use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use std::sync::atomic::{AtomicU64, Ordering};

pub struct ConcurrencyBenchmark {
    name: String,
    iterations: usize,
    threads: usize,
}

impl ConcurrencyBenchmark {
    pub fn new(name: &str, iterations: usize, threads: usize) -> Self {
        Self {
            name: name.to_string(),
            iterations,
            threads,
        }
    }

    pub fn benchmark_atomic_counter(&self) -> BenchmarkResult {
        let counter = Arc::new(AtomicU64::new(0));
        let start = Instant::now();

        let handles: Vec<_> = (0..self.threads)
            .map(|_| {
                let counter = Arc::clone(&counter);
                thread::spawn(move || {
                    for _ in 0..self.iterations {
                        counter.fetch_add(1, Ordering::Relaxed);
                    }
                })
            })
            .collect();

        for handle in handles {
            handle.join().unwrap();
        }

        let duration = start.elapsed();
        let total_ops = self.threads * self.iterations;
        let ops_per_sec = total_ops as f64 / duration.as_secs_f64();

        BenchmarkResult {
            name: format!("{}_atomic_counter", self.name),
            total_operations: total_ops,
            duration,
            ops_per_second: ops_per_sec,
            threads_used: self.threads,
        }
    }

    pub fn benchmark_mutex_counter(&self) -> BenchmarkResult {
        let counter = Arc::new(Mutex::new(0u64));
        let start = Instant::now();

        let handles: Vec<_> = (0..self.threads)
            .map(|_| {
                let counter = Arc::clone(&counter);
                thread::spawn(move || {
                    for _ in 0..self.iterations {
                        let mut num = counter.lock().unwrap();
                        *num += 1;
                    }
                })
            })
            .collect();

        for handle in handles {
            handle.join().unwrap();
        }

        let duration = start.elapsed();
        let total_ops = self.threads * self.iterations;
        let ops_per_sec = total_ops as f64 / duration.as_secs_f64();

        BenchmarkResult {
            name: format!("{}_mutex_counter", self.name),
            total_operations: total_ops,
            duration,
            ops_per_second: ops_per_sec,
            threads_used: self.threads,
        }
    }

    pub fn benchmark_channel_throughput(&self) -> BenchmarkResult {
        use std::sync::mpsc;

        let (tx, rx) = mpsc::channel();
        let start = Instant::now();

        // Consumer thread
        let consumer = thread::spawn(move || {
            let mut count = 0;
            while let Ok(_) = rx.recv() {
                count += 1;
            }
            count
        });

        // Producer threads
        let handles: Vec<_> = (0..self.threads)
            .map(|_| {
                let tx = tx.clone();
                thread::spawn(move || {
                    for _ in 0..self.iterations {
                        tx.send(1).unwrap();
                    }
                })
            })
            .collect();

        // Wait for producers
        for handle in handles {
            handle.join().unwrap();
        }

        // Signal consumer to stop
        drop(tx);

        let received_count = consumer.join().unwrap();
        let duration = start.elapsed();
        let ops_per_sec = received_count as f64 / duration.as_secs_f64();

        BenchmarkResult {
            name: format!("{}_channel_throughput", self.name),
            total_operations: received_count,
            duration,
            ops_per_second: ops_per_sec,
            threads_used: self.threads,
        }
    }
}

#[derive(Debug)]
pub struct BenchmarkResult {
    pub name: String,
    pub total_operations: usize,
    pub duration: Duration,
    pub ops_per_second: f64,
    pub threads_used: usize,
}

impl BenchmarkResult {
    pub fn print(&self) {
        println!(
            "{}: {:.2} ops/sec ({} ops in {:?}, {} threads)",
            self.name, self.ops_per_second, self.total_operations, self.duration, self.threads_used
        );
    }
}

pub fn run_comprehensive_benchmark() {
    println!("Running comprehensive concurrency benchmarks...\n");

    let configs = vec![
        (4, 100_000),
        (8, 100_000),
        (16, 100_000),
    ];

    for (threads, iterations) in configs {
        println!("=== {} threads, {} iterations ===", threads, iterations);

        let benchmark = ConcurrencyBenchmark::new("test", iterations, threads);

        let atomic_result = benchmark.benchmark_atomic_counter();
        let mutex_result = benchmark.benchmark_mutex_counter();
        let channel_result = benchmark.benchmark_channel_throughput();

        atomic_result.print();
        mutex_result.print();
        channel_result.print();

        let speedup = atomic_result.ops_per_second / mutex_result.ops_per_second;
        println!("Atomic vs Mutex speedup: {:.2}x\n", speedup);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_atomic_counter_benchmark() {
        let benchmark = ConcurrencyBenchmark::new("test", 1000, 4);
        let result = benchmark.benchmark_atomic_counter();
        assert_eq!(result.total_operations, 4000);
        assert!(result.ops_per_second > 0.0);
    }
}