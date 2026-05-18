# 压力测试与模糊测试

## 当使用此专题

- 进行大规模并发压力测试
- 实现智能模糊测试
- 发现并发系统的边界条件
- 验证系统在高负载下的稳定性

## 压力测试框架

### 并发压力测试器
```rust
use std::sync::{Arc, Barrier, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use std::collections::HashMap;
use rand::Rng;

/// 压力测试配置
#[derive(Debug, Clone)]
pub struct StressTestConfig {
    /// 并发线程数
    pub concurrent_threads: usize,
    /// 测试持续时间
    pub duration: Duration,
    /// 每线程操作数
    pub operations_per_thread: usize,
    /// 工作负载类型
    pub workload_type: WorkloadType,
    /// 资源限制
    pub resource_limits: ResourceLimits,
    /// 监控间隔
    pub monitoring_interval: Duration,
}

#[derive(Debug, Clone)]
pub enum WorkloadType {
    /// CPU密集型
    CpuIntensive { iterations: usize },
    /// 内存密集型
    MemoryIntensive { allocation_size: usize },
    /// I/O密集型
    IoIntensive { file_operations: bool, network_operations: bool },
    /// 混合负载
    Mixed { cpu_ratio: f64, memory_ratio: f64, io_ratio: f64 },
    /// 锁竞争
    LockContention { critical_section_time: Duration },
}

#[derive(Debug, Clone)]
pub struct ResourceLimits {
    pub max_memory_mb: usize,
    pub max_cpu_percent: f64,
    pub max_file_descriptors: usize,
}

impl Default for StressTestConfig {
    fn default() -> Self {
        Self {
            concurrent_threads: num_cpus::get() * 2,
            duration: Duration::from_secs(60),
            operations_per_thread: 10000,
            workload_type: WorkloadType::Mixed {
                cpu_ratio: 0.4,
                memory_ratio: 0.3,
                io_ratio: 0.3,
            },
            resource_limits: ResourceLimits {
                max_memory_mb: 1024,
                max_cpu_percent: 90.0,
                max_file_descriptors: 1000,
            },
            monitoring_interval: Duration::from_millis(500),
        }
    }
}

/// 压力测试结果
#[derive(Debug)]
pub struct StressTestResult {
    pub test_duration: Duration,
    pub total_operations: u64,
    pub successful_operations: u64,
    pub failed_operations: u64,
    pub operations_per_second: f64,
    pub average_latency: Duration,
    pub p95_latency: Duration,
    pub p99_latency: Duration,
    pub max_latency: Duration,
    pub memory_usage_mb: f64,
    pub cpu_usage_percent: f64,
    pub error_details: Vec<String>,
    pub thread_performance: Vec<ThreadPerformance>,
}

#[derive(Debug)]
pub struct ThreadPerformance {
    pub thread_id: usize,
    pub operations: u64,
    pub errors: u64,
    pub average_latency_ns: u64,
    pub peak_memory_mb: f64,
}

impl StressTestResult {
    pub fn print_summary(&self) {
        println!("=== Stress Test Results ===");
        println!("Test Duration: {:?}", self.test_duration);
        println!("Total Operations: {}", self.total_operations);
        println!("Successful Operations: {}", self.successful_operations);
        println!("Failed Operations: {}", self.failed_operations);
        println!("Success Rate: {:.2}%", (self.successful_operations as f64 / self.total_operations as f64) * 100.0);
        println!("Throughput: {:.2} ops/sec", self.operations_per_second);
        println!("Average Latency: {:?}", self.average_latency);
        println!("P95 Latency: {:?}", self.p95_latency);
        println!("P99 Latency: {:?}", self.p99_latency);
        println!("Max Latency: {:?}", self.max_latency);
        println!("Memory Usage: {:.2} MB", self.memory_usage_mb);
        println!("CPU Usage: {:.1}%", self.cpu_usage_percent);

        if !self.error_details.is_empty() {
            println!("\nErrors:");
            for error in &self.error_details {
                println!("  - {}", error);
            }
        }
    }
}

/// 压力测试器
pub struct StressTester {
    config: StressTestConfig,
}

impl StressTester {
    pub fn new(config: StressTestConfig) -> Self {
        Self { config }
    }

    /// 运行压力测试
    pub fn run(&self) -> StressTestResult {
        println!("Starting stress test with {} threads for {:?}",
               self.config.concurrent_threads, self.config.duration);

        let barrier = Arc::new(Barrier::new(self.config.concurrent_threads + 1));
        let shutdown = Arc::new(AtomicBool::new(false));
        let results = Arc::new(Mutex::new(Vec::new()));
        let mut handles = Vec::new();

        let start_time = Instant::now();

        // 启动工作线程
        for thread_id in 0..self.config.concurrent_threads {
            let barrier_clone = Arc::clone(&barrier);
            let shutdown_clone = Arc::clone(&shutdown);
            let results_clone = Arc::clone(&results);
            let config = self.config.clone();

            let handle = thread::spawn(move || {
                Self::worker_thread(thread_id, config, barrier_clone, shutdown_clone, results_clone)
            });

            handles.push(handle);
        }

        // 启动监控线程
        let monitoring_interval = self.config.monitoring_interval;
        let start_time_clone = start_time;
        let monitor_handle = thread::spawn(move || {
            let mut last_check = start_time_clone;

            while start_time_clone.elapsed() < self.config.duration {
                thread::sleep(monitoring_interval);

                let elapsed = last_check.elapsed();
                let total_elapsed = start_time_clone.elapsed();

                // 简单的资源监控
                let memory_usage = Self::get_memory_usage_mb();
                let cpu_usage = Self::get_cpu_usage_percent();

                println!("[MONITOR] Elapsed: {:?}, Memory: {:.1}MB, CPU: {:.1}%",
                       total_elapsed, memory_usage, cpu_usage);

                last_check = Instant::now();
            }
        });

        // 等待所有线程就绪，然后开始测试
        barrier.wait();
        println!("All threads started. Test running...");

        // 运行指定时间
        thread::sleep(self.config.duration);

        // 发送关闭信号
        shutdown.store(true, Ordering::Relaxed);
        println!("Stopping stress test...");

        // 等待所有线程完成
        for handle in handles {
            handle.join().unwrap();
        }

        // 等待监控线程
        monitor_handle.join().unwrap();

        let test_duration = start_time.elapsed();

        // 收集结果
        let mut all_results = Vec::new();
        let mut total_ops = 0u64;
        let mut total_errors = 0u64;
        let mut latencies = Vec::new();

        let results_guard = results.lock().unwrap();
        for thread_result in results_guard.iter() {
            all_results.push(thread_result.clone());
            total_ops += thread_result.operations;
            total_errors += thread_result.errors;
            latencies.push(thread_result.average_latency_ns);
        }

        // 计算统计信息
        let total_operations = total_ops as u64;
        let successful_operations = total_operations - total_errors;
        let operations_per_second = total_operations as f64 / test_duration.as_secs_f64();

        // 计算延迟统计
        latencies.sort_unstable();
        let average_latency = Duration::from_nanos(
            latencies.iter().sum::<u64>() / latencies.len() as u64
        );
        let p95_latency = Duration::from_nanos(
            latencies[(latencies.len() as f64 * 0.95) as usize]
        );
        let p99_latency = Duration::from_nanos(
            latencies[(latencies.len() as f64 * 0.99) as usize]
        );
        let max_latency = Duration::from_nanos(*latencies.last().unwrap_or(&0));

        StressTestResult {
            test_duration,
            total_operations,
            successful_operations,
            failed_operations: total_errors,
            operations_per_second,
            average_latency,
            p95_latency,
            p99_latency,
            max_latency,
            memory_usage_mb: Self::get_memory_usage_mb(),
            cpu_usage_percent: Self::get_cpu_usage_percent(),
            error_details: Vec::new(), // 简化实现
            thread_performance: all_results,
        }
    }

    /// 工作线程实现
    fn worker_thread(
        thread_id: usize,
        config: StressTestConfig,
        barrier: Arc<Barrier>,
        shutdown: Arc<AtomicBool>,
        results: Arc<Mutex<Vec<ThreadPerformance>>>,
    ) {
        // 等待开始信号
        barrier.wait();

        let mut operations = 0u64;
        let mut errors = 0u64;
        let mut latencies = Vec::new();
        let mut peak_memory = 0.0f64;

        let start_time = Instant::now();

        while !shutdown.load(Ordering::Relaxed) &&
              operations < config.operations_per_thread as u64 {
            let operation_start = Instant::now();

            // 根据工作负载类型执行操作
            let result = match &config.workload_type {
                WorkloadType::CpuIntensive { iterations } => {
                    Self::cpu_intensive_work(*iterations)
                }
                WorkloadType::MemoryIntensive { allocation_size } => {
                    Self::memory_intensive_work(*allocation_size)
                }
                WorkloadType::IoIntensive { file_operations, network_operations } => {
                    Self::io_intensive_work(*file_operations, *network_operations)
                }
                WorkloadType::Mixed { cpu_ratio, memory_ratio, io_ratio } => {
                    let random = rand::random::<f64>();
                    if random < *cpu_ratio {
                        Self::cpu_intensive_work(1000)
                    } else if random < *cpu_ratio + *memory_ratio {
                        Self::memory_intensive_work(1024)
                    } else {
                        Self::io_intensive_work(true, true)
                    }
                }
                WorkloadType::LockContention { critical_section_time } => {
                    Self::lock_contention_work(*critical_section_time)
                }
            };

            let latency = operation_start.elapsed();
            latencies.push(latency.as_nanos());

            match result {
                Ok(_) => operations += 1,
                Err(_) => errors += 1,
            }

            // 更新内存使用统计
            if operations % 1000 == 0 {
                let current_memory = Self::get_memory_usage_mb();
                if current_memory > peak_memory {
                    peak_memory = current_memory;
                }
            }
        }

        let average_latency_ns = if latencies.is_empty() {
            0
        } else {
            latencies.iter().sum::<u64>() / latencies.len() as u64
        };

        let thread_result = ThreadPerformance {
            thread_id,
            operations,
            errors,
            average_latency_ns,
            peak_memory_mb: peak_memory,
        };

        results.lock().unwrap().push(thread_result);
    }

    /// CPU密集型工作
    fn cpu_intensive_work(iterations: usize) -> Result<(), String> {
        let mut result = 0u64;
        for i in 0..iterations {
            result = result.wrapping_add((i * i) as u64);
            result = result.wrapping_mul(result + 1);
        }
        Ok(())
    }

    /// 内存密集型工作
    fn memory_intensive_work(allocation_size: usize) -> Result<(), String> {
        let data: Vec<u8> = vec![0; allocation_size];
        // 简单的内存操作
        let sum: usize = data.iter().map(|&x| x as usize).sum();
        drop(data);
        Ok(())
    }

    /// I/O密集型工作
    fn io_intensive_work(file_operations: bool, network_operations: bool) -> Result<(), String> {
        if file_operations {
            let temp_file = "/tmp/stress_test_temp.txt";
            let content = "Stress test data ".repeat(100);

            if let Err(e) = std::fs::write(temp_file, content) {
                return Err(format!("File write error: {}", e));
            }

            if let Err(e) = std::fs::read_to_string(temp_file) {
                return Err(format!("File read error: {}", e));
            }

            let _ = std::fs::remove_file(temp_file);
        }

        if network_operations {
            // 简单的网络操作（localhost连接）
            use std::net::TcpStream;
            use std::time::Duration;

            if let Ok(_) = TcpStream::connect_timeout("127.0.0.1:80", Duration::from_millis(100)) {
                // 连接成功，但没有实际数据传输
            }
            // 忽略连接错误，因为目标可能不可用
        }

        Ok(())
    }

    /// 锁竞争工作
    fn lock_contention_work(critical_section_time: Duration) -> Result<(), String> {
        use std::sync::Mutex;
        static GLOBAL_LOCK: Mutex<()> = Mutex::new(());

        let _guard = GLOBAL_LOCK.lock().unwrap();
        thread::sleep(critical_section_time);
        Ok(())
    }

    /// 获取内存使用量（MB）
    fn get_memory_usage_mb() -> f64 {
        #[cfg(target_os = "linux")]
        {
            use std::fs;
            if let Ok(status) = fs::read_to_string("/proc/self/status") {
                for line in status.lines() {
                    if line.starts_with("VmRSS:") {
                        if let Some(kb_str) = line.split_whitespace().nth(1) {
                            if let Ok(kb) = kb_str.parse::<f64>() {
                                return kb / 1024.0; // 转换为MB
                            }
                        }
                    }
                }
            }
        }

        // 默认值或非Linux系统
        0.0
    }

    /// 获取CPU使用率（简化实现）
    fn get_cpu_usage_percent() -> f64 {
        // 这是一个简化的实现，实际应该使用系统调用获取精确的CPU使用率
        rand::random::<f64>() * 20.0 + 40.0 // 模拟40-60%的CPU使用率
    }
}

/// 压力测试使用示例
fn stress_test_example() {
    let config = StressTestConfig {
        concurrent_threads: 8,
        duration: Duration::from_secs(30),
        operations_per_thread: 5000,
        workload_type: WorkloadType::Mixed {
            cpu_ratio: 0.5,
            memory_ratio: 0.2,
            io_ratio: 0.3,
        },
        resource_limits: ResourceLimits {
            max_memory_mb: 512,
            max_cpu_percent: 95.0,
            max_file_descriptors: 500,
        },
        monitoring_interval: Duration::from_millis(1000),
    };

    let tester = StressTester::new(config);
    let result = tester.run();

    result.print_summary();

    // 验证结果合理性
    assert!(result.total_operations > 0);
    assert!(result.operations_per_second > 0.0);
    assert!(result.success_rate() > 0.5); // 至少50%成功率

    println!("Stress test completed successfully!");
}
```

## 模糊测试框架

### 并发模糊测试器
```rust
use std::sync::{Arc, Mutex};
use std::collections::HashSet;
use std::thread;
use std::time::{Duration, Instant};
use rand::Rng;

/// 模糊测试配置
#[derive(Debug, Clone)]
pub struct FuzzTestConfig {
    /// 测试迭代次数
    pub iterations: usize,
    /// 并发测试器数量
    pub concurrent_fuzzers: usize,
    /// 输入大小范围
    pub input_size_range: (usize, usize),
    /// 变异率
    pub mutation_rate: f64,
    /// 语料库大小
    pub corpus_size: usize,
    /// 超时时间
    pub timeout: Duration,
    /// 目标覆盖率
    pub target_coverage: f64,
}

impl Default for FuzzTestConfig {
    fn default() -> Self {
        Self {
            iterations: 10000,
            concurrent_fuzzers: 4,
            input_size_range: (1, 1024),
            mutation_rate: 0.1,
            corpus_size: 1000,
            timeout: Duration::from_secs(5),
            target_coverage: 80.0,
        }
    }
}

/// 模糊测试结果
#[derive(Debug)]
pub struct FuzzTestResult {
    pub total_iterations: usize,
    pub successful_iterations: usize,
    pub failed_iterations: usize,
    pub timeouts: usize,
    pub crashes: usize,
    pub unique_crashes: HashSet<String>,
    pub coverage_percentage: f64,
    pub execution_time: Duration,
    pub average_execution_time: Duration,
    pub test_cases_generated: usize,
    pub mutations_performed: usize,
}

impl FuzzTestResult {
    pub fn print_summary(&self) {
        println!("=== Fuzz Test Results ===");
        println!("Total Iterations: {}", self.total_iterations);
        println!("Successful: {}", self.successful_iterations);
        println!("Failed: {}", self.failed_iterations);
        println!("Timeouts: {}", self.timeouts);
        println!("Crashes: {}", self.crashes);
        println!("Unique Crashes: {}", self.unique_crashes.len());
        println!("Coverage: {:.1}%", self.coverage_percentage);
        println!("Execution Time: {:?}", self.execution_time);
        println!("Average Execution Time: {:?}", self.average_execution_time);
        println!("Test Cases Generated: {}", self.test_cases_generated);
        println!("Mutations Performed: {}", self.mutations_performed);

        if !self.unique_crashes.is_empty() {
            println!("\nUnique Crash Signatures:");
            for crash in &self.unique_crashes {
                println!("  - {}", crash);
            }
        }
    }

    pub fn success_rate(&self) -> f64 {
        if self.total_iterations == 0 {
            0.0
        } else {
            self.successful_iterations as f64 / self.total_iterations as f64 * 100.0
        }
    }

    pub fn crash_rate(&self) -> f64 {
        if self.total_iterations == 0 {
            0.0
        } else {
            self.crashes as f64 / self.total_iterations as f64 * 100.0
        }
    }
}

/// 测试目标
pub trait FuzzTarget {
    /// 执行测试
    fn execute(&mut self, input: &[u8]) -> Result<(), TestError>;
    /// 获取代码覆盖率（简化实现）
    fn get_coverage(&self) -> f64;
    /// 重置目标状态
    fn reset(&mut self);
}

/// 测试错误
#[derive(Debug, Clone)]
pub enum TestError {
    Panic(String),
    AssertionError(String),
    Timeout,
    MemoryError(String),
    IoError(String),
    NetworkError(String),
    Unknown(String),
}

impl std::fmt::Display for TestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TestError::Panic(msg) => write!(f, "Panic: {}", msg),
            TestError::AssertionError(msg) => write!(f, "Assertion failed: {}", msg),
            TestError::Timeout => write!(f, "Test timeout"),
            TestError::MemoryError(msg) => write!(f, "Memory error: {}", msg),
            TestError::IoError(msg) => write!(f, "I/O error: {}", msg),
            TestError::NetworkError(msg) => write!(f, "Network error: {}", msg),
            TestError::Unknown(msg) => write!(f, "Unknown error: {}", msg),
        }
    }
}

impl std::error::Error for TestError {}

/// 模糊测试器
pub struct FuzzTester<T>
where
    T: FuzzTarget + Send + Clone,
{
    config: FuzzTestConfig,
    corpus: Arc<Mutex<Vec<Vec<u8>>>>,
    coverage_tracker: Arc<Mutex<HashSet<usize>>>,
}

impl<T> FuzzTester<T>
where
    T: FuzzTarget + Send + Clone + 'static,
{
    pub fn new(config: FuzzTestConfig) -> Self {
        Self {
            config,
            corpus: Arc::new(Mutex::new(Vec::new())),
            coverage_tracker: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    /// 初始化语料库
    pub fn initialize_corpus(&self) {
        let mut corpus = self.corpus.lock().unwrap();

        // 生成一些初始测试用例
        let initial_inputs = vec![
            b"Hello, World!".to_vec(),
            vec![0; 1024], // 全零
            vec![255; 1024], // 全255
            (0..256).collect(), // 0-255
            b"GET / HTTP/1.1\r\nHost: example.com\r\n\r\n".to_vec(),
            b"POST /api/data HTTP/1.1\r\nContent-Type: application/json\r\n\r\n{}".to_vec(),
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZ".to_vec(),
            b"abcdefghijklmnopqrstuvwxyz".to_vec(),
            b"0123456789".to_vec(),
        ];

        for input in initial_inputs {
            if corpus.len() < self.config.corpus_size {
                corpus.push(input);
            }
        }

        // 随机生成更多测试用例
        let mut rng = rand::thread_rng();
        while corpus.len() < self.config.corpus_size {
            let size = rng.gen_range(self.config.input_size_range.0..=self.config.input_size_range.1);
            let input: Vec<u8> = (0..size).map(|_| rng.gen()).collect();
            corpus.push(input);
        }
    }

    /// 运行模糊测试
    pub fn run(&mut self, mut target: T) -> FuzzTestResult {
        println!("Starting fuzz test with {} iterations", self.config.iterations);

        self.initialize_corpus();

        let start_time = Instant::now();
        let mut result = FuzzTestResult {
            total_iterations: 0,
            successful_iterations: 0,
            failed_iterations: 0,
            timeouts: 0,
            crashes: 0,
            unique_crashes: HashSet::new(),
            coverage_percentage: 0.0,
            execution_time: Duration::ZERO,
            average_execution_time: Duration::ZERO,
            test_cases_generated: 0,
            mutations_performed: 0,
        };

        let execution_times = Arc::new(Mutex::new(Vec::new()));
        let shutdown = Arc::new(AtomicBool::new(false));

        // 启动多个并发模糊测试器
        let mut handles = Vec::new();
        for fuzzer_id in 0..self.config.concurrent_fuzzers {
            let fuzzer = Fuzzer::new(
                fuzzer_id,
                self.config.clone(),
                Arc::clone(&self.corpus),
                Arc::clone(&self.coverage_tracker),
                Arc::clone(&execution_times),
                Arc::clone(&shutdown),
            );

            let handle = thread::spawn(move || fuzzer.run(target.clone()));
            handles.push(handle);
        }

        // 运行指定次数的迭代
        for iteration in 0..self.config.iterations {
            if iteration % 1000 == 0 {
                println!("Fuzz iteration {} / {}", iteration, self.config.iterations);
            }

            thread::sleep(Duration::from_millis(1));
        }

        // 停止所有模糊测试器
        shutdown.store(true, Ordering::Relaxed);

        // 收集结果
        let mut all_times = Vec::new();
        for handle in handles {
            let (times, crashes, timeouts, mutations) = handle.join().unwrap();
            all_times.extend(times);
            result.crashes += crashes;
            result.timeouts += timeouts;
            result.mutations_performed += mutations;
        }

        // 计算统计信息
        result.execution_time = start_time.elapsed();
        result.total_iterations = all_times.len();
        result.successful_iterations = result.total_iterations - result.timeouts - result.crashes;
        result.failed_iterations = result.timeouts + result.crashes;

        if !all_times.is_empty() {
            let total_time: Duration = all_times.iter().sum();
            result.average_execution_time = total_time / all_times.len() as u32;
        }

        result.test_cases_generated = self.corpus.lock().unwrap().len();
        result.coverage_percentage = target.get_coverage();

        result.print_summary();

        result
    }
}

/// 单个模糊测试器
struct Fuzzer<T>
where
    T: FuzzTarget,
{
    id: usize,
    config: FuzzTestConfig,
    corpus: Arc<Mutex<Vec<Vec<u8>>>>,
    coverage_tracker: Arc<Mutex<HashSet<usize>>>,
    execution_times: Arc<Mutex<Vec<Duration>>>,
    shutdown: Arc<AtomicBool>,
}

impl<T> Fuzzer<T>
where
    T: FuzzTarget + Send + Clone,
{
    fn new(
        id: usize,
        config: FuzzTestConfig,
        corpus: Arc<Mutex<Vec<Vec<u8>>>>,
        coverage_tracker: Arc<Mutex<HashSet<usize>>>,
        execution_times: Arc<Mutex<Vec<Duration>>>,
        shutdown: Arc<AtomicBool>,
    ) -> Self {
        Self {
            id,
            config,
            corpus,
            coverage_tracker,
            execution_times,
            shutdown,
        }
    }

    fn run(self, mut target: T) -> (Vec<Duration>, u32, u32, u32) {
        let mut rng = rand::thread_rng();
        let mut execution_times = Vec::new();
        let mut crashes = 0u32;
        let mut timeouts = 0u32;
        let mut mutations = 0u32;

        while !self.shutdown.load(Ordering::Relaxed) {
            // 选择或生成测试输入
            let input = self.select_or_generate_input(&mut rng);

            // 变异输入
            let mutated_input = if rng.gen::<f64>() < self.config.mutation_rate {
                mutations += 1;
                self.mutate_input(&input, &mut rng)
            } else {
                input.clone()
            };

            // 执行测试
            let start_time = Instant::now();
            let execution_result = tokio::time::timeout(
                self.config.timeout,
                std::thread::spawn({
                    let input = mutated_input.clone();
                    move || target.execute(&input)
                })
            ).await;

            let execution_time = start_time.elapsed();
            execution_times.push(execution_time);

            match execution_result {
                Ok(Ok(_)) => {
                    // 测试成功
                }
                Ok(Err(error)) => {
                    crashes += 1;
                    println!("Fuzzer {} crash: {}", self.id, error);

                    // 记录崩溃签名
                    let crash_signature = self.generate_crash_signature(&mutated_input, &error);
                    if let Some(signature) = crash_signature {
                        println!("Crash signature: {}", signature);
                    }
                }
                Err(_) => {
                    timeouts += 1;
                }
            }

            // 重置目标状态
            target.reset();

            // 随机休眠，避免过于频繁的执行
            thread::sleep(Duration::from_micros(1));
        }

        (execution_times, crashes, timeouts, mutations)
    }

    fn select_or_generate_input(&self, rng: &mut impl rand::Rng) -> Vec<u8> {
        let corpus = self.corpus.lock().unwrap();

        if corpus.is_empty() {
            // 生成随机输入
            let size = rng.gen_range(self.config.input_size_range.0..=self.config.input_size_range.1);
            (0..size).map(|_| rng.gen()).collect()
        } else {
            // 从语料库中随机选择
            corpus[rng.gen_range(0..corpus.len())].clone()
        }
    }

    fn mutate_input(&self, input: &[u8], rng: &mut impl rand::Rng) -> Vec<u8> {
        let mut mutated = input.to_vec();

        match rng.gen_range(0..5) {
            0 => {
                // 位翻转
                if !mutated.is_empty() {
                    let index = rng.gen_range(0..mutated.len());
                    mutated[index] ^= 1 << rng.gen_range(0..8);
                }
            }
            1 => {
                // 字节替换
                if !mutated.is_empty() {
                    let index = rng.gen_range(0..mutated.len());
                    mutated[index] = rng.gen();
                }
            }
            2 => {
                // 字节删除
                if !mutated.is_empty() {
                    let index = rng.gen_range(0..mutated.len());
                    mutated.remove(index);
                }
            }
            3 => {
                // 字节插入
                if mutated.len() < self.config.input_size_range.1 {
                    let index = rng.gen_range(0..=mutated.len());
                    mutated.insert(index, rng.gen());
                }
            }
            4 => {
                // 块翻转
                if mutated.len() >= 4 {
                    let start = rng.gen_range(0..mutated.len() - 3);
                    let end = (start + 4).min(mutated.len());
                    for i in start..end {
                        mutated[i] = !mutated[i];
                    }
                }
            }
            _ => {}
        }

        mutated
    }

    fn generate_crash_signature(&self, input: &[u8], error: &TestError) -> Option<String> {
        // 生成崩溃签名的简化实现
        let hash = {
            use std::collections::hash_map::DefaultHasher;
            use std::hash::{Hash, Hasher};

            let mut hasher = DefaultHasher::new();
            input.hash(&mut hasher);
            format!("{:x}", hasher.finish())
        };

        Some(format!("Error: {} | Input: {} | Size: {}",
                     error, hash, input.len()))
    }
}

/// 示例模糊测试目标
#[derive(Debug, Clone)]
pub struct ExampleConcurrentTarget {
    shared_data: Arc<Mutex<Vec<i32>>>,
}

impl ExampleConcurrentTarget {
    pub fn new() -> Self {
        Self {
            shared_data: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl FuzzTarget for ExampleConcurrentTarget {
    fn execute(&mut self, input: &[u8]) -> Result<(), TestError> {
        // 解析输入为操作序列
        for (i, &byte) in input.iter().enumerate() {
            let operation = byte % 4;

            match operation {
                0 => {
                    // 插入操作
                    let mut data = self.shared_data.lock().unwrap();
                    data.push(i as i32);
                    if data.len() > 1000 {
                        return Err(TestError::Panic("Data too large".to_string()));
                    }
                }
                1 => {
                    // 删除操作
                    let mut data = self.shared_data.lock().unwrap();
                    if !data.is_empty() {
                        data.pop();
                    }
                }
                2 => {
                    // 查找操作
                    let data = self.shared_data.lock().unwrap();
                    let _ = data.iter().find(|&&x| x == (byte as i32));
                }
                3 => {
                    // 清空操作
                    let mut data = self.shared_data.lock().unwrap();
                    data.clear();
                }
                _ => {}
            }

            // 模拟一些计算工作
            let mut sum = 0u64;
            for j in 0..100 {
                sum = sum.wrapping_add((i * 100 + j) as u64);
            }

            // 偶性检查
            if input.len() > 100 && sum % 1000 == 0 {
                return Err(TestError::AssertionError("Sum check failed".to_string()));
            }
        }

        Ok(())
    }

    fn get_coverage(&self) -> f64 {
        // 简化的覆盖率计算
        let data = self.shared_data.lock().unwrap();
        let basic_coverage = if !data.is_empty() { 50.0 } else { 0.0 };

        // 模拟更复杂的覆盖率分析
        basic_coverage + rand::random::<f64>() * 30.0
    }

    fn reset(&mut self) {
        let mut data = self.shared_data.lock().unwrap();
        data.clear();
    }
}

/// 模糊测试使用示例
fn fuzz_test_example() {
    let config = FuzzTestConfig {
        iterations: 10000,
        concurrent_fuzzers: 4,
        input_size_range: (1, 512),
        mutation_rate: 0.15,
        corpus_size: 500,
        timeout: Duration::from_millis(500),
        target_coverage: 75.0,
    };

    let mut tester = FuzzTester::new(config);
    let target = ExampleConcurrentTarget::new();
    let result = tester.run(target);

    println!("Fuzz testing completed!");

    // 验证结果
    assert!(result.total_iterations > 0);
    assert!(result.success_rate() > 80.0); // 至少80%成功率

    if result.crash_rate() > 5.0 {
        println!("Warning: High crash rate detected: {:.1}%", result.crash_rate());
    }
}
```

## 相关专题

- `../debugging/concurrent-bugs.md` - 并发Bug调试
- `../tools/testing/race-condition-detector.rs` - 竞态条件检测
- `../patterns/error-handling.md` - 错误处理策略