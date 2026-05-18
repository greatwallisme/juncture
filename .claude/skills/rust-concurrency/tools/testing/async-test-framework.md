# 异步测试框架

## 当使用此专题

- 测试异步代码正确性
- 验证并发安全
- 性能基准测试
- 压力测试和稳定性验证

## 测试框架设计

### 异步测试宏
```rust
#[macro_export]
macro_rules! async_test {
    ($name:ident, $future:expr) => {
        #[test]
        fn $name() {
            let mut executor = $crate::testing::AsyncTestExecutor::new();
            executor.block_on(async move {
                $future.await
            });
        }
    };
}

// 使用示例
async_test!(test_simple_async, async {
    let result = compute_something().await;
    assert_eq!(result, 42);
});
```

### 超时测试
```rust
use std::time::Duration;

/// 带超时的异步测试
pub struct TimeoutTest {
    duration: Duration,
}

impl TimeoutTest {
    pub fn new(duration: Duration) -> Self {
        Self { duration }
    }

    pub async fn run<F, T>(&self, future: F) -> Result<T, TimeoutError>
    where
        F: Future<Output = T>,
    {
        let timeout_future = async {
            tokio::time::sleep(self.duration).await;
            TimeoutError
        };

        match select(future, timeout_future).await {
            Either::Left(result) => Ok(result),
            Either::Right(TimeoutError) => Err(TimeoutError),
        }
    }
}

#[derive(Debug)]
pub struct TimeoutError;

/// 超时测试宏
#[macro_export]
macro_rules! timeout_test {
    ($duration:expr, $future:expr) => {
        let timeout_test = $crate::testing::TimeoutTest::new($duration);
        match timeout_test.run($future).await {
            Ok(result) => result,
            Err($crate::testing::TimeoutError) => panic!("Test timed out after {:?}", $duration),
        }
    };
}

// 使用示例
timeout_test!(Duration::from_secs(5), async {
    let result = slow_operation().await;
    assert_eq!(result, expected);
});
```

## 并发测试工具

### 竞态条件检测
```rust
use std::sync::Arc;
use std::thread;
use std::sync::atomic::{AtomicUsize, Ordering};

/// 并发测试运行器
pub struct ConcurrentTestRunner {
    thread_count: usize,
    iterations: usize,
}

impl ConcurrentTestRunner {
    pub fn new(thread_count: usize, iterations: usize) -> Self {
        Self {
            thread_count,
            iterations,
        }
    }

    /// 运行并发测试
    pub fn run<F, R>(&self, test_fn: F) -> Vec<R>
    where
        F: Fn(usize) -> R + Send + Sync + 'static,
        R: Send + 'static,
    {
        let test_fn = Arc::new(test_fn);
        let mut handles = Vec::new();

        for thread_id in 0..self.thread_count {
            let test_fn = Arc::clone(&test_fn);
            let iterations = self.iterations;

            let handle = thread::spawn(move || {
                let mut results = Vec::new();
                for i in 0..iterations {
                    results.push(test_fn(thread_id * 1000 + i));
                }
                results
            });

            handles.push(handle);
        }

        let mut all_results = Vec::new();
        for handle in handles {
            let results = handle.join().unwrap();
            all_results.extend(results);
        }

        all_results
    }

    /// 测试数据结构的并发安全性
    pub fn test_concurrent_safety<T, F>(&self, create_container: F) -> ConcurrentTestResult
    where
        T: Send + Sync,
        F: Fn() -> T + Send + Sync + 'static,
    {
        let container = Arc::new(create_container());
        let counter = Arc::new(AtomicUsize::new(0));
        let error_count = Arc::new(AtomicUsize::new(0));

        let mut handles = Vec::new();

        // 生产者线程
        for i in 0..self.thread_count / 2 {
            let container = Arc::clone(&container);
            let counter = Arc::clone(&counter);
            let error_count = Arc::clone(&error_count);

            let handle = thread::spawn(move || {
                for j in 0..self.iterations {
                    let value = i * 1000 + j;
                    if let Err(_) = container.push(value) {
                        error_count.fetch_add(1, Ordering::Relaxed);
                    }
                    counter.fetch_add(1, Ordering::Relaxed);
                }
            });

            handles.push(handle);
        }

        // 消费者线程
        for _ in self.thread_count / 2..self.thread_count {
            let container = Arc::clone(&container);
            let counter = Arc::clone(&counter);

            let handle = thread::spawn(move || {
                let mut local_count = 0;
                for _ in 0..self.iterations {
                    if let Some(_) = container.pop() {
                        local_count += 1;
                    }
                }
                local_count
            });

            handles.push(handle);
        }

        // 等待所有线程完成
        let mut consumer_counts = Vec::new();
        for handle in handles {
            if let Ok(count) = handle.join() {
                consumer_counts.push(count);
            }
        }

        let total_produced = counter.load(Ordering::Relaxed);
        let total_consumed: usize = consumer_counts.iter().sum();
        let errors = error_count.load(Ordering::Relaxed);

        ConcurrentTestResult {
            total_produced,
            total_consumed,
            errors,
            thread_count: self.thread_count,
            iterations: self.iterations,
        }
    }
}

#[derive(Debug)]
pub struct ConcurrentTestResult {
    pub total_produced: usize,
    pub total_consumed: usize,
    pub errors: usize,
    pub thread_count: usize,
    pub iterations: usize,
}

impl ConcurrentTestResult {
    pub fn is_consistent(&self) -> bool {
        self.errors == 0 && (self.total_produced == self.total_consumed ||
            (self.total_produced > self.total_consumed && self.total_produced - self.total_consumed <= self.thread_count))
    }

    pub fn print_summary(&self) {
        println!("Concurrent Test Results:");
        println!("  Threads: {}", self.thread_count);
        println!("  Iterations per thread: {}", self.iterations);
        println!("  Total produced: {}", self.total_produced);
        println!("  Total consumed: {}", self.total_consumed);
        println!("  Errors: {}", self.errors);
        println!("  Consistent: {}", self.is_consistent());
    }
}
```

### 内存泄漏检测
```rust
use std::collections::HashMap;
use std::sync::Mutex;

/// 内存泄漏检测器
pub struct MemoryLeakDetector {
    allocations: Arc<Mutex<HashMap<usize, AllocationInfo>>>,
    deallocations: Arc<Mutex<HashMap<usize, DeallocationInfo>>>,
}

#[derive(Debug)]
struct AllocationInfo {
    size: usize,
    location: String,
    timestamp: Instant,
    backtrace: Option<Vec<String>>,
}

#[derive(Debug)]
struct DeallocationInfo {
    size: usize,
    timestamp: Instant,
}

impl MemoryLeakDetector {
    pub fn new() -> Self {
        Self {
            allocations: Arc::new(Mutex::new(HashMap::new())),
            deallocations: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn track_allocation(&self, ptr: usize, size: usize, location: &str) {
        let info = AllocationInfo {
            size,
            location: location.to_string(),
            timestamp: Instant::now(),
            backtrace: self.capture_backtrace(),
        };

        let mut allocations = self.allocations.lock().unwrap();
        allocations.insert(ptr, info);

        // 同时移除对应的释放记录（如果有）
        let mut deallocations = self.deallocations.lock().unwrap();
        deallocations.remove(&ptr);
    }

    pub fn track_deallocation(&self, ptr: usize, size: usize) {
        let info = DeallocationInfo {
            size,
            timestamp: Instant::now(),
        };

        let mut deallocations = self.deallocations.lock().unwrap();
        deallocations.insert(ptr, info);

        // 检查是否存在内存泄漏
        if self.has_leaks() {
            let leaks = self.get_leaks();
            if !leaks.is_empty() {
                println!("Memory leaks detected:");
                for leak in leaks {
                    println!("  {} bytes at {} (allocated {:?})",
                        leak.size, leak.location, leak.timestamp.elapsed());
                }
            }
        }
    }

    pub fn has_leaks(&self) -> bool {
        let allocations = self.allocations.lock().unwrap();
        let deallocations = self.deallocations.lock().unwrap();

        !allocations.keys().all(|ptr| deallocations.contains_key(ptr))
    }

    pub fn get_leaks(&self) -> Vec<AllocationInfo> {
        let allocations = self.allocations.lock().unwrap();
        let deallocations = self.deallocations.lock().unwrap();

        allocations.values()
            .filter(|info| !deallocations.contains_key(&(info.location.as_ptr() as usize)))
            .cloned()
            .collect()
    }

    fn capture_backtrace(&self) -> Option<Vec<String>> {
        #[cfg(feature = "backtrace")]
        {
            use std::backtrace::{Backtrace, BacktraceFrame};
            let backtrace = Backtrace::new();
            Some(backtrace.frames().iter().enumerate().map(|(i, frame)| {
                format!("{}: {:?}", i, frame)
            }).collect())
        }
        #[cfg(not(feature = "backtrace"))]
        {
            None
        }
    }
}

impl Drop for MemoryLeakDetector {
    fn drop(&mut self) {
        if self.has_leaks() {
            println!("Final memory leak check:");
            for leak in self.get_leaks() {
                println!("  {} bytes at {} (allocated {:?})",
                    leak.size, leak.location, leak.timestamp.elapsed());
            }
        }
    }
}

/// 内存泄漏检测宏
#[macro_export]
macro_rules! track_allocation {
    ($ptr:expr, $size:expr) => {
        if let Some(detector) = $crate::testing::LEAK_DETECTOR.get() {
            detector.track_allocation($ptr as usize, $size, concat!("(", file!(), ":", line!(), ")"));
        }
    };
}

#[macro_export]
macro_rules! track_deallocation {
    ($ptr:expr, $size:expr) => {
        if let Some(detector) = $crate::testing::LEAK_DETECTOR.get() {
            detector.track_deallocation($ptr as usize, $size);
        }
    };
}
```

## 性能测试框架

### 基准测试工具
```rust
use std::time::{Duration, Instant};

/// 性能基准测试
pub struct Benchmark {
    name: String,
    iterations: usize,
    warmup_iterations: usize,
}

impl Benchmark {
    pub fn new(name: &str, iterations: usize) -> Self {
        Self {
            name: name.to_string(),
            iterations,
            warmup_iterations: iterations / 10,
        }
    }

    /// 运行同步基准测试
    pub fn run<F, R>(&self, test_fn: F) -> BenchmarkResult
    where
        F: Fn() -> R,
    {
        // 预热
        for _ in 0..self.warmup_iterations {
            test_fn();
        }

        let start_time = Instant::now();

        // 正式测试
        for _ in 0..self.iterations {
            test_fn();
        }

        let duration = start_time.elapsed();

        BenchmarkResult {
            name: self.name.clone(),
            iterations: self.iterations,
            total_duration: duration,
            avg_duration: duration / self.iterations as u32,
            throughput: self.iterations as f64 / duration.as_secs_f64(),
        }
    }

    /// 运行异步基准测试
    pub async fn run_async<F, R, Fut>(&self, test_fn: F) -> BenchmarkResult
    where
        F: Fn() -> Fut,
        Fut: Future<Output = R>,
    {
        // 预热
        for _ in 0..self.warmup_iterations {
            test_fn().await;
        }

        let start_time = Instant::now();

        // 正式测试
        for _ in 0..self.iterations {
            test_fn().await;
        }

        let duration = start_time.elapsed();

        BenchmarkResult {
            name: self.name.clone(),
            iterations: self.iterations,
            total_duration: duration,
            avg_duration: duration / self.iterations as u32,
            throughput: self.iterations as f64 / duration.as_secs_f64(),
        }
    }

    /// 运行并发基准测试
    pub fn run_concurrent<F, R>(&self, thread_count: usize, test_fn: F) -> ConcurrentBenchmarkResult
    where
        F: Fn(usize) -> R + Send + Sync + 'static,
        R: Send + 'static,
    {
        let test_fn = Arc::new(test_fn);
        let mut handles = Vec::new();

        let start_time = Instant::now();

        for thread_id in 0..thread_count {
            let test_fn = Arc::clone(&test_fn);
            let iterations_per_thread = self.iterations / thread_count;

            let handle = thread::spawn(move || {
                let thread_start = Instant::now();

                for i in 0..iterations_per_thread {
                    test_fn(thread_id * 1000 + i);
                }

                thread_start.elapsed()
            });

            handles.push(handle);
        }

        let mut thread_durations = Vec::new();
        for handle in handles {
            thread_durations.push(handle.join().unwrap());
        }

        let total_duration = start_time.elapsed();
        let max_thread_duration = thread_durations.iter().max().unwrap();
        let min_thread_duration = thread_durations.iter().min().unwrap();

        ConcurrentBenchmarkResult {
            name: self.name.clone(),
            thread_count,
            iterations: self.iterations,
            total_duration,
            max_thread_duration: *max_thread_duration,
            min_thread_duration: *min_thread_duration,
            throughput: self.iterations as f64 / total_duration.as_secs_f64(),
            parallel_efficiency: (thread_durations.iter().sum::<Duration>() / thread_count as u32).as_secs_f64() / max_thread_duration.as_secs_f64(),
        }
    }
}

#[derive(Debug)]
pub struct BenchmarkResult {
    pub name: String,
    pub iterations: usize,
    pub total_duration: Duration,
    pub avg_duration: Duration,
    pub throughput: f64,
}

#[derive(Debug)]
pub struct ConcurrentBenchmarkResult {
    pub name: String,
    pub thread_count: usize,
    pub iterations: usize,
    pub total_duration: Duration,
    pub max_thread_duration: Duration,
    pub min_thread_duration: Duration,
    pub throughput: f64,
    pub parallel_efficiency: f64,
}

impl BenchmarkResult {
    pub fn print_summary(&self) {
        println!("Benchmark: {}", self.name);
        println!("  Iterations: {}", self.iterations);
        println!("  Total time: {:?}", self.total_duration);
        println!("  Average time: {:?}", self.avg_duration);
        println!("  Throughput: {:.2} ops/sec", self.throughput);
    }
}

impl ConcurrentBenchmarkResult {
    pub fn print_summary(&self) {
        println!("Concurrent Benchmark: {}", self.name);
        println!("  Threads: {}", self.thread_count);
        println!("  Iterations: {}", self.iterations);
        println!("  Total time: {:?}", self.total_duration);
        println!("  Max thread time: {:?}", self.max_thread_duration);
        println!("  Min thread time: {:?}", self.min_thread_duration);
        println!("  Throughput: {:.2} ops/sec", self.throughput);
        println!("  Parallel efficiency: {:.2}%", self.parallel_efficiency * 100.0);
    }
}
```

## 测试报告生成

### 测试结果聚合
```rust
use std::collections::HashMap;

/// 测试套件
pub struct TestSuite {
    name: String,
    tests: HashMap<String, TestResult>,
    benchmarks: HashMap<String, BenchmarkResult>,
    start_time: Instant,
}

impl TestSuite {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            tests: HashMap::new(),
            benchmarks: HashMap::new(),
            start_time: Instant::now(),
        }
    }

    pub fn add_test_result(&mut self, name: &str, result: TestResult) {
        self.tests.insert(name.to_string(), result);
    }

    pub fn add_benchmark_result(&mut self, name: &str, result: BenchmarkResult) {
        self.benchmarks.insert(name.to_string(), result);
    }

    pub fn generate_report(&self) -> TestReport {
        let duration = self.start_time.elapsed();
        let passed_tests = self.tests.values().filter(|r| r.passed).count();
        let failed_tests = self.tests.len() - passed_tests;

        TestReport {
            name: self.name.clone(),
            duration,
            total_tests: self.tests.len(),
            passed_tests,
            failed_tests,
            test_results: self.tests.clone(),
            benchmarks: self.benchmarks.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct TestResult {
    pub name: String,
    pub passed: bool,
    pub duration: Duration,
    pub error_message: Option<String>,
}

#[derive(Debug)]
pub struct TestReport {
    pub name: String,
    pub duration: Duration,
    pub total_tests: usize,
    pub passed_tests: usize,
    pub failed_tests: usize,
    pub test_results: HashMap<String, TestResult>,
    pub benchmarks: HashMap<String, BenchmarkResult>,
}

impl TestReport {
    pub fn print_summary(&self) {
        println!("\n=== Test Suite: {} ===", self.name);
        println!("Duration: {:?}", self.duration);
        println!("Tests: {}/{} passed", self.passed_tests, self.total_tests);

        if self.failed_tests > 0 {
            println!("FAILED TESTS:");
            for (name, result) in &self.test_results {
                if !result.passed {
                    println!("  ❌ {}: {:?}", name, result.error_message);
                }
            }
        }

        if !self.benchmarks.is_empty() {
            println!("\nBENCHMARKS:");
            for (_, benchmark) in &self.benchmarks {
                benchmark.print_summary();
            }
        }
    }

    pub fn save_to_file<P: AsRef<std::path::Path>>(&self, path: P) -> std::io::Result<()> {
        use std::fs::File;
        use std::io::Write;

        let mut file = File::create(path)?;

        writeln!(file, "# Test Report: {}", self.name)?;
        writeln!(file, "## Summary")?;
        writeln!(file, "- Duration: {:?}", self.duration)?;
        writeln!(file, "- Tests: {}/{} passed", self.passed_tests, self.total_tests)?;

        if !self.test_results.is_empty() {
            writeln!(file, "\n## Test Results")?;
            for (name, result) in &self.test_results {
                let status = if result.passed { "✅" } else { "❌" };
                writeln!(file, "- {} {}: {:?} {}", status, name, result.duration,
                    if let Some(err) = &result.error_message { err } else { "" })?;
            }
        }

        if !self.benchmarks.is_empty() {
            writeln!(file, "\n## Benchmarks")?;
            for (name, benchmark) in &self.benchmarks {
                writeln!(file, "### {}", name)?;
                writeln!(file, "- Iterations: {}", benchmark.iterations)?;
                writeln!(file, "- Throughput: {:.2} ops/sec", benchmark.throughput)?;
                writeln!(file, "- Average time: {:?}", benchmark.avg_duration)?;
            }
        }

        Ok(())
    }
}
```

## 相关专题

- `../debugging/concurrent-bugs.md` - 并发Bug调试
- `../tools/analysis/contention-analyzer.rs` - 竞争分析工具