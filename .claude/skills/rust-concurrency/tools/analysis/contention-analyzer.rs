// Lock Contention Analyzer
// Based on "深入理解Rust并发编程" Chapter 9: Performance Optimization

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use std::fs;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct ContentionMetrics {
    pub lock_name: String,
    pub acquisition_count: u64,
    pub total_wait_time: Duration,
    pub max_wait_time: Duration,
    pub avg_wait_time: Duration,
    pub contention_ratio: f64,
    pub timestamp: Instant,
}

impl ContentionMetrics {
    pub fn new(lock_name: String) -> Self {
        Self {
            lock_name,
            acquisition_count: 0,
            total_wait_time: Duration::ZERO,
            max_wait_time: Duration::ZERO,
            avg_wait_time: Duration::ZERO,
            contention_ratio: 0.0,
            timestamp: Instant::now(),
        }
    }

    pub fn update(&mut self, wait_time: Duration) {
        self.acquisition_count += 1;
        self.total_wait_time += wait_time;
        self.max_wait_time = self.max_wait_time.max(wait_time);

        if self.acquisition_count > 0 {
            self.avg_wait_time = self.total_wait_time / self.acquisition_count as u32;
        }
    }

    pub fn calculate_contention_ratio(&mut self, total_acquisitions: u64) {
        if total_acquisitions > 0 {
            self.contention_ratio = (self.acquisition_count as f64) / (total_acquisitions as f64);
        }
    }
}

pub struct ContentionAnalyzer {
    metrics: Arc<Mutex<HashMap<String, ContentionMetrics>>>,
    total_acquisitions: Arc<Mutex<u64>>,
    analysis_duration: Duration,
}

impl ContentionAnalyzer {
    pub fn new(analysis_duration: Duration) -> Self {
        Self {
            metrics: Arc::new(Mutex::new(HashMap::new())),
            total_acquisitions: Arc::new(Mutex::new(0)),
            analysis_duration,
        }
    }

    /// Start monitoring lock contention in the current process
    pub fn start_monitoring(&self) -> ContentionMonitor {
        ContentionMonitor {
            metrics: Arc::clone(&self.metrics),
            total_acquisitions: Arc::clone(&self.total_acquisitions),
            start_time: Instant::now(),
        }
    }

    /// Analyze lock patterns from source code
    pub fn analyze_source_code<P: AsRef<Path>>(&self, source_path: P) -> Result<Vec<ContentionMetrics>, Box<dyn std::error::Error>> {
        let content = fs::read_to_string(source_path)?;
        let mut metrics = Vec::new();

        // Analyze different lock patterns
        self.analyze_mutex_patterns(&content, &mut metrics)?;
        self.analyze_rwlock_patterns(&content, &mut metrics)?;
        self.analyze_atomic_patterns(&content, &mut metrics)?;

        Ok(metrics)
    }

    /// Detect potential contention hotspots
    pub fn detect_contention_hotspots(&self) -> Vec<(&str, f64)> {
        let metrics = self.metrics.lock().unwrap();
        let mut hotspots: Vec<(&str, f64)> = metrics
            .values()
            .map(|m| (&m.lock_name[..], m.contention_ratio))
            .collect();

        // Sort by contention ratio (descending)
        hotspots.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());

        hotspots
    }

    /// Generate optimization suggestions
    pub fn generate_optimization_suggestions(&self) -> Vec<String> {
        let metrics = self.metrics.lock().unwrap();
        let mut suggestions = Vec::new();

        for metric in metrics.values() {
            if metric.contention_ratio > 0.1 {
                suggestions.push(format!(
                    "High contention detected on '{}': {:.1}% of acquisitions had contention. Consider using lock sharding or lock-free algorithms.",
                    metric.lock_name, metric.contention_ratio * 100.0
                ));
            }

            if metric.avg_wait_time > Duration::from_millis(10) {
                suggestions.push(format!(
                    "High average wait time on '{}': {:?}. Consider reducing critical section size or using read-write locks.",
                    metric.lock_name, metric.avg_wait_time
                ));
            }

            if metric.max_wait_time > Duration::from_millis(100) {
                suggestions.push(format!(
                    "Very long wait times on '{}': {:?}. Check for potential deadlocks or I/O operations in critical sections.",
                    metric.lock_name, metric.max_wait_time
                ));
            }
        }

        suggestions
    }

    fn analyze_mutex_patterns(&self, content: &str, metrics: &mut Vec<ContentionMetrics>) -> Result<(), Box<dyn std::error::Error>> {
        use regex::Regex;

        // Find Mutex<T> usage patterns
        let mutex_regex = Regex::new(r"Mutex<([^>]+)>\s*([a-zA-Z_][a-zA-Z0-9_]*)")?;

        for caps in mutex_regex.captures_iter(content) {
            let type_name = caps.get(1).unwrap().as_str();
            let var_name = caps.get(2).unwrap().as_str();

            // Check for nested locks
            if content.contains(&format!("{}.lock()", var_name)) &&
               content.contains(&format!("{}.lock()", var_name).replace(var_name, "other")) {
                let metric = ContentionMetrics::new(format!("Mutex<{}>", type_name));
                metrics.push(metric);
            }

            // Check for long-held locks
            let long_held_pattern = format!("let.*_.*= {}.lock\\(\\).*;.*\\.unwrap\\(\\);.*;", var_name);
            if Regex::new(&long_held_pattern)?.is_match(content) {
                let metric = ContentionMetrics::new(format!("LongHeldMutex<{}>", type_name));
                metrics.push(metric);
            }
        }

        Ok(())
    }

    fn analyze_rwlock_patterns(&self, content: &str, metrics: &mut Vec<ContentionMetrics>) -> Result<(), Box<dyn std::error::Error>> {
        use regex::Regex;

        let rwlock_regex = Regex::new(r"RwLock<([^>]+)>\s*([a-zA-Z_][a-zA-Z0-9_]*)")?;

        for caps in rwlock_regex.captures_iter(content) {
            let type_name = caps.get(1).unwrap().as_str();
            let var_name = caps.get(2).unwrap().as_str();

            // Check for write-heavy patterns
            let write_pattern = format!("{}.write\\(", var_name);
            let read_pattern = format!("{}.read\\(", var_name);

            let write_count = Regex::new(&write_pattern)?.find_iter(content).count();
            let read_count = Regex::new(&read_pattern)?.find_iter(content).count();

            if write_count > read_count && write_count > 5 {
                let metric = ContentionMetrics::new(format!("WriteHeavyRwLock<{}>", type_name));
                metrics.push(metric);
            }

            // Check for repeated read-write cycles
            let cycle_pattern = format!("let.*= {}.read\\(\\).*;.*\\.drop\\(.*\\);.*\\.write\\(", var_name);
            if Regex::new(&cycle_pattern)?.find_iter(content).count() > 3 {
                let metric = ContentionMetrics::new(format!("RwLockCycle<{}>", type_name));
                metrics.push(metric);
            }
        }

        Ok(())
    }

    fn analyze_atomic_patterns(&self, content: &str, metrics: &mut Vec<ContentionMetrics>) -> Result<(), Box<dyn std::error::Error>> {
        use regex::Regex;

        let atomic_regex = Regex::new(r"Atomic([A-Za-z0-9_]+)")?;

        for caps in atomic_regex.captures_iter(content) {
            let atomic_type = caps.get(1).unwrap().as_str();

            // Check for spin patterns
            let spin_pattern = format!("while.*{}.compare_and", atomic_type);
            if Regex::new(&spin_pattern)?.is_match(content) {
                let metric = ContentionMetrics::new(format!("AtomicSpin<{}>", atomic_type));
                metrics.push(metric);
            }

            // Check for high-frequency updates
            let update_pattern = format!("{}.fetch_", atomic_type);
            let update_count = Regex::new(&update_pattern)?.find_iter(content).count();

            if update_count > 50 {
                let metric = ContentionMetrics::new(format!("HighFreqAtomic<{}>", atomic_type));
                metrics.push(metric);
            }
        }

        Ok(())
    }
}

pub struct ContentionMonitor {
    metrics: Arc<Mutex<HashMap<String, ContentionMetrics>>>,
    total_acquisitions: Arc<Mutex<u64>>,
    start_time: Instant,
}

impl ContentionMonitor {
    pub fn record_acquisition(&self, lock_name: &str, wait_time: Duration) {
        let mut metrics = self.metrics.lock().unwrap();
        let metric = metrics.entry(lock_name.to_string()).or_insert_with(|| ContentionMetrics::new(lock_name.to_string()));
        metric.update(wait_time);

        let mut total = self.total_acquisitions.lock().unwrap();
        *total += 1;
    }

    pub fn stop_and_report(&self) -> Vec<ContentionMetrics> {
        let end_time = Instant::now();
        let duration = end_time - self.start_time;

        let mut metrics = self.metrics.lock().unwrap();
        let total_acquisitions = *self.total_acquisitions.lock().unwrap();

        // Update final statistics
        for metric in metrics.values_mut() {
            metric.calculate_contention_ratio(total_acquisitions);
        }

        metrics.values().cloned().collect()
    }
}

/// Profile lock contention in a function
#[macro_export]
macro_rules! profile_lock {
    ($lock_name:expr, $code:block) => {
        {
            let start = std::time::Instant::now();
            let _guard = $lock_name.lock().unwrap();
            let wait_time = start.elapsed();

            // Record the acquisition wait time
            // In a real implementation, this would be stored in a contention monitor
            println!("Lock '{}' waited: {:?}", $lock_name, wait_time);

            $code
        }
    };
}

/// Profile RwLock contention
#[macro_export]
macro_rules! profile_rwlock_read {
    ($rwlock:expr, $code:block) => {
        {
            let start = std::time::Instant::now();
            let _guard = $rwlock.read().unwrap();
            let wait_time = start.elapsed();

            println!("RwLock read waited: {:?}", wait_time);

            $code
        }
    };
}

#[macro_export]
macro_rules! profile_rwlock_write {
    ($rwlock:expr, $code:block) => {
        {
            let start = std::time::Instant::now();
            let _guard = $rwlock.write().unwrap();
            let wait_time = start.elapsed();

            println!("RwLock write waited: {:?}", wait_time);

            $code
        }
    };
}

/// Profile atomic operation contention
#[macro_export]
macro_rules! profile_atomic_operation {
    ($atomic:expr, $operation:expr) => {
        {
            let start = std::time::Instant::now();
            let _result = $operation;
            let wait_time = start.elapsed();

            println!("Atomic operation took: {:?}", wait_time);

            _result
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use std::thread;

    #[test]
    fn test_contention_analyzer() {
        let analyzer = ContentionAnalyzer::new(Duration::from_secs(10));

        // Simulate lock contention
        let mutex = Arc::new(Mutex::new(0));
        let handles: Vec<_> = (0..10).map(|_| {
            let mutex = Arc::clone(&mutex);
            thread::spawn(move || {
                for _ in 0..100 {
                    profile_lock!(mutex, {
                        let _guard = mutex.lock().unwrap();
                        *_guard += 1;
                        thread::sleep(Duration::from_millis(1));
                    });
                }
            })
        }).collect();

        for handle in handles {
            handle.join().unwrap();
        }
    }

    #[test]
    fn test_source_code_analysis() {
        let code = r#"
        use std::sync::{Mutex, RwLock, atomic::AtomicU64};

        struct Data {
            mutex1: Mutex<u64>,
            mutex2: Mutex<String>,
            rwlock: RwLock<Vec<i32>>,
            counter: AtomicU64,
        }

        impl Data {
            fn process(&self) {
                let _guard = self.mutex1.lock().unwrap();
                let _guard2 = self.mutex2.lock().unwrap();

                while self.counter.compare_exchange(0, 1, Ordering::SeqCst, Ordering::Relaxed).is_err() {
                    // Spin wait
                }

                let read_guard = self.rwlock.read().unwrap();
                drop(read_guard);

                let write_guard = self.rwlock.write().unwrap();
                drop(write_guard);
            }
        }
        "#;

        let analyzer = ContentionAnalyzer::new(Duration::from_secs(1));
        let metrics = analyzer.analyze_source_code(code.as_bytes()).unwrap();

        // Should detect potential contention issues
        assert!(!metrics.is_empty());
    }
}