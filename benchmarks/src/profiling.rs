//! CPU and memory profiling utilities for benchmarks.
//!
//! Provides functions to measure CPU time, wall-clock time, and peak memory
//! usage during graph execution. Each profiling run produces a `ProfileResult`
//! with per-scenario metrics.

use std::io::Write;
use std::time::{Duration, Instant};

/// Linux clock ticks per second (typically 100, verified via `getconf CLK_TCK`).
const CLK_TCK: u64 = 100;

/// Result of a profiled execution run (multiple iterations).
///
/// Stores both aggregated metrics and raw per-iteration wall-clock samples
/// so that JSON output matches the Python benchmark schema (`times_ms`,
/// `mean_ms`, `min_ms`, `max_ms`).
#[derive(Debug, Clone)]
pub struct ProfileResult {
    /// Scenario name (e.g., `sequential_100`)
    pub scenario: String,
    /// Number of nodes executed per iteration
    pub node_count: usize,
    /// Number of timed iterations
    pub iterations: usize,
    /// Raw per-iteration wall-clock times in milliseconds
    pub times_ms: Vec<f64>,
    /// Aggregated CPU time (user + system) across all iterations
    pub cpu_time: Duration,
    /// Peak resident set size in bytes observed across all iterations
    pub peak_rss_bytes: u64,
}

impl ProfileResult {
    /// Mean wall-clock time in milliseconds across all iterations.
    #[must_use]
    pub fn mean_ms(&self) -> f64 {
        if self.times_ms.is_empty() {
            return 0.0;
        }
        self.times_ms.iter().sum::<f64>()
            / f64::from(u32::try_from(self.times_ms.len()).unwrap_or(1))
    }

    /// Minimum wall-clock time in milliseconds across all iterations.
    #[must_use]
    pub fn min_ms(&self) -> f64 {
        self.times_ms.iter().copied().fold(f64::INFINITY, f64::min)
    }

    /// Maximum wall-clock time in milliseconds across all iterations.
    #[must_use]
    pub fn max_ms(&self) -> f64 {
        self.times_ms
            .iter()
            .copied()
            .fold(f64::NEG_INFINITY, f64::max)
    }

    /// Alias for [`mean_ms`] -- kept for backward compatibility with `compare.py`.
    #[must_use]
    pub fn wall_ms(&self) -> f64 {
        self.mean_ms()
    }

    /// CPU time in milliseconds (aggregated across all iterations).
    #[must_use]
    pub fn cpu_ms(&self) -> f64 {
        self.cpu_time.as_secs_f64() * 1000.0
    }

    /// CPU utilization ratio (`cpu_time` / total wall time), capped at 1.0.
    #[must_use]
    pub fn cpu_utilization(&self) -> f64 {
        let mean_wall = self.mean_ms();
        if mean_wall <= 0.0 {
            return 0.0;
        }
        let total_wall_ms = mean_wall * f64::from(u32::try_from(self.iterations).unwrap_or(1));
        let ratio = self.cpu_ms() / total_wall_ms;
        ratio.min(1.0)
    }

    /// Peak RSS in megabytes.
    #[must_use]
    pub fn peak_rss_mb(&self) -> f64 {
        f64::from(u32::try_from(self.peak_rss_bytes).unwrap_or(u32::MAX)) / (1024.0 * 1024.0)
    }

    /// Per-node wall-clock time in microseconds (based on mean wall time).
    #[must_use]
    pub fn per_node_wall_us(&self) -> f64 {
        if self.node_count == 0 {
            return 0.0;
        }
        (self.mean_ms() * 1000.0) / f64::from(u32::try_from(self.node_count).unwrap_or(u32::MAX))
    }

    /// Per-node CPU time in microseconds (based on total CPU time / iterations / nodes).
    #[must_use]
    pub fn per_node_cpu_us(&self) -> f64 {
        if self.node_count == 0 || self.iterations == 0 {
            return 0.0;
        }
        let cpu_per_iter_us = self.cpu_time.as_secs_f64() * 1_000_000.0
            / f64::from(u32::try_from(self.iterations).unwrap_or(1));
        cpu_per_iter_us / f64::from(u32::try_from(self.node_count).unwrap_or(u32::MAX))
    }
}

/// Read current process CPU time (user + system) from `/proc/self/stat`.
/// Returns `Duration::ZERO` if unavailable (non-Linux platforms).
fn read_process_cpu_time() -> Duration {
    let Ok(stat) = std::fs::read_to_string("/proc/self/stat") else {
        return Duration::ZERO;
    };

    // Format: pid (comm) state ppid ... utime stime ...
    // Fields are space-separated, but comm may contain spaces and parens.
    // Find the last ')' to skip past comm, then count fields.
    let Some(pos) = stat.rfind(')') else {
        return Duration::ZERO;
    };
    let fields: Vec<&str> = stat[pos + 2..].split_whitespace().collect();

    // After (comm), field indices are 0-based:
    // 0: state, 1: ppid, 2: pgrp, 3: session, 4: tty_nr, 5: tpgid,
    // 6: flags, 7: minflt, 8: cminflt, 9: majflt, 10: cmajflt,
    // 11: utime, 12: stime
    if fields.len() < 13 {
        return Duration::ZERO;
    }

    let utime_ticks: u64 = fields[11].parse().unwrap_or(0);
    let stime_ticks: u64 = fields[12].parse().unwrap_or(0);

    let total_ticks = utime_ticks + stime_ticks;
    let secs = total_ticks / CLK_TCK;
    let nanos = ((total_ticks % CLK_TCK) * 1_000_000_000) / CLK_TCK;

    Duration::new(secs, u32::try_from(nanos).unwrap_or(u32::MAX))
}

/// Read peak RSS from `/proc/self/status`.
/// Returns 0 if unavailable.
fn read_peak_rss() -> u64 {
    let Ok(status) = std::fs::read_to_string("/proc/self/status") else {
        return 0;
    };

    for line in status.lines() {
        if let Some(value) = line.strip_prefix("VmHWM:") {
            let trimmed = value.trim();
            // Format: "12345 kB"
            let kb_str = trimmed.strip_suffix(" kB").unwrap_or(trimmed);
            return kb_str.trim().parse::<u64>().unwrap_or(0) * 1024;
        }
    }

    0
}

/// Snapshot of process resource usage at a point in time.
struct ResourceSnapshot {
    cpu_time: Duration,
    peak_rss: u64,
}

impl ResourceSnapshot {
    fn capture() -> Self {
        Self {
            cpu_time: read_process_cpu_time(),
            peak_rss: read_peak_rss(),
        }
    }
}

/// Profile a closure that executes a graph invocation.
///
/// Measures wall-clock time, CPU time, and peak RSS before and after execution.
/// The closure is executed `iterations` times. Raw per-iteration wall times are
/// preserved in `times_ms` for JSON output that matches the Python benchmark schema.
pub fn profile_execution<F, R>(
    scenario: &str,
    node_count: usize,
    iterations: usize,
    mut execute: F,
) -> ProfileResult
where
    F: FnMut() -> R,
{
    let mut times_ms = Vec::with_capacity(iterations);
    let mut total_cpu = Duration::ZERO;
    let mut max_peak_rss = 0u64;

    for _ in 0..iterations {
        let cpu_before = ResourceSnapshot::capture();
        let wall_start = Instant::now();

        let _ = execute();

        let wall_elapsed = wall_start.elapsed();
        let cpu_after = ResourceSnapshot::capture();

        times_ms.push(wall_elapsed.as_secs_f64() * 1000.0);
        let cpu_delta = cpu_after.cpu_time.saturating_sub(cpu_before.cpu_time);
        total_cpu += cpu_delta;

        if cpu_after.peak_rss > max_peak_rss {
            max_peak_rss = cpu_after.peak_rss;
        }
    }

    ProfileResult {
        scenario: scenario.to_string(),
        node_count,
        iterations,
        times_ms,
        cpu_time: total_cpu,
        peak_rss_bytes: max_peak_rss,
    }
}

/// Print a formatted profiling report table.
pub fn print_profiling_report(results: &[ProfileResult]) {
    let stdout = std::io::stdout();
    let mut lock = stdout.lock();

    writeln!(lock).unwrap_or(());
    writeln!(lock, "{}", "=".repeat(80)).unwrap_or(());
    writeln!(lock, "PROFILING REPORT (averaged over iterations)").unwrap_or(());
    writeln!(lock, "{}", "=".repeat(80)).unwrap_or(());

    writeln!(
        lock,
        "{:<25} {:>10} {:>10} {:>10} {:>8} {:>10} {:>12}",
        "Scenario", "Wall(ms)", "CPU(ms)", "PerNode(us)", "CPU%", "RSS(MB)", "Nodes"
    )
    .unwrap_or(());
    writeln!(lock, "{}", "-".repeat(85)).unwrap_or(());

    for r in results {
        writeln!(
            lock,
            "{:<25} {:>10.3} {:>10.3} {:>10.1} {:>7.1}% {:>10.1} {:>12}",
            r.scenario,
            r.wall_ms(),
            r.cpu_ms(),
            r.per_node_wall_us(),
            r.cpu_utilization() * 100.0,
            r.peak_rss_mb(),
            r.node_count,
        )
        .unwrap_or(());
    }

    writeln!(lock).unwrap_or(());
}

/// Print a CSV-formatted profiling report for machine consumption.
pub fn print_profiling_csv(results: &[ProfileResult]) {
    let stdout = std::io::stdout();
    let mut lock = stdout.lock();

    writeln!(
        lock,
        "scenario,wall_ms,cpu_ms,per_node_wall_us,per_node_cpu_us,cpu_pct,peak_rss_mb,node_count"
    )
    .unwrap_or(());

    for r in results {
        writeln!(
            lock,
            "{},{:.4},{:.4},{:.2},{:.2},{:.1},{:.2},{}",
            r.scenario,
            r.wall_ms(),
            r.cpu_ms(),
            r.per_node_wall_us(),
            r.per_node_cpu_us(),
            r.cpu_utilization() * 100.0,
            r.peak_rss_mb(),
            r.node_count,
        )
        .unwrap_or(());
    }
}

/// Serialize profiling results to a JSON file.
///
/// The output format matches the Python benchmark JSON schema so the
/// comparison script (`scripts/compare.py`) can consume both sides.
/// Each entry includes:
/// - Python-compatible fields: `scenario`, `num_nodes`, `iterations`,
///   `times_ms`, `mean_ms`, `min_ms`, `max_ms`
/// - Rust-specific fields: `cpu_ms`, `per_node_wall_us`, `per_node_cpu_us`,
///   `cpu_pct`, `peak_rss_mb`
///
/// # Errors
///
/// Returns `std::io::Error` if the file cannot be created or JSON serialization fails.
pub fn save_json(path: &std::path::Path, results: &[ProfileResult]) -> std::io::Result<()> {
    let benchmarks: Vec<serde_json::Value> = results
        .iter()
        .map(|r| {
            serde_json::json!({
                // Python-compatible fields
                "scenario": r.scenario,
                "num_nodes": r.node_count,
                "iterations": r.iterations,
                "times_ms": r.times_ms,
                "mean_ms": r.mean_ms(),
                "min_ms": r.min_ms(),
                "max_ms": r.max_ms(),
                // Rust-specific fields
                "cpu_ms": r.cpu_ms(),
                "per_node_wall_us": r.per_node_wall_us(),
                "per_node_cpu_us": r.per_node_cpu_us(),
                "cpu_pct": r.cpu_utilization() * 100.0,
                "peak_rss_mb": r.peak_rss_mb(),
            })
        })
        .collect();

    let output = serde_json::json!({
        "benchmarks": benchmarks,
    });

    let file = std::fs::File::create(path)?;
    serde_json::to_writer_pretty(file, &output)?;
    Ok(())
}

// Rust guideline compliant 2026-05-25
