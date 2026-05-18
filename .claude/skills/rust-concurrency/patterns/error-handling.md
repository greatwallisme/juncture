# 并发错误处理

## 当使用此专题

- 构健的并发系统错误处理
- 实现错误恢复和故障转移
- 处理级联故障和系统级错误
- 设计高可用性并发架构

## 核心功能

### 错误处理策略分析
```
Analyze error handling: failure_rate=0.01, recovery_time=5s, cascade_prevention=true, retry_policy=exponential
```
分析错误处理策略的有效性

### 故障恢复验证
```
Verify fault tolerance: single_node_failure=true, network_partition=true, data_consistency=strong, recovery_time=30s
```
验证系统容错能力

## 并发错误处理基础

### 错误类型分类
```rust
use std::fmt;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// 并发系统错误类型
#[derive(Debug, Clone)]
pub enum ConcurrencyError {
    /// 超时错误
    Timeout {
        operation: String,
        duration: Duration,
    },
    /// 资源竞争错误
    ResourceContention {
        resource: String,
        wait_time: Duration,
        max_wait_time: Duration,
    },
    /// 死锁错误
    Deadlock {
        threads: Vec<usize>,
        resources: Vec<String>,
    },
    /// 数据竞争错误
    DataRace {
        variable: String,
        thread_id: usize,
        operation: String,
    },
    /// 内存错误
    MemoryError {
        error_type: String,
        allocation_size: usize,
    },
    /// I/O错误
    IoError {
        operation: String,
        source: String,
    },
    /// 网络错误
    NetworkError {
        operation: String,
        peer: String,
        error_code: Option<i32>,
    },
    /// 系统错误
    SystemError {
        component: String,
        error_code: i32,
        description: String,
    },
    /// 用户定义错误
    Custom {
        error_code: u32,
        message: String,
        context: String,
    },
}

impl fmt::Display for ConcurrencyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConcurrencyError::Timeout { operation, duration } => {
                write!(f, "Timeout in operation '{}' after {:?}", operation, duration)
            }
            ConcurrencyError::ResourceContention { resource, wait_time, max_wait_time } => {
                write!(f, "Resource '{}' contention after {:?} (max: {:?})",
                       resource, wait_time, max_wait_time)
            }
            ConcurrencyError::Deadlock { threads, resources } => {
                write!(f, "Deadlock detected involving threads {:?} and resources {:?}",
                       threads, resources)
            }
            ConcurrencyError::DataRace { variable, thread_id, operation } => {
                write!(f, "Data race on variable '{}' in thread {} during '{}'",
                       variable, thread_id, operation)
            }
            ConcurrencyError::MemoryError { error_type, allocation_size } => {
                write!(f, "Memory error '{}' for allocation of {} bytes",
                       error_type, allocation_size)
            }
            ConcurrencyError::IoError { operation, source } => {
                write!(f, "I/O error in operation '{}': {}", operation, source)
            }
            ConcurrencyError::NetworkError { operation, peer, error_code } => {
                write!(f, "Network error '{}' with peer {} (code: {:?})",
                       operation, peer, error_code)
            }
            ConcurrencyError::SystemError { component, error_code, description } => {
                write!(f, "System error in component '{}' (code: {}): {}",
                       component, error_code, description)
            }
            ConcurrencyError::Custom { error_code, message, context } => {
                write!(f, "Custom error {} (code: {}): {} - {}",
                       error_code, message, context)
            }
        }
    }
}

impl std::error::Error for ConcurrencyError {}

/// 错误严重程度
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum ErrorSeverity {
    Info = 0,
    Warning = 1,
    Error = 2,
    Critical = 3,
    Fatal = 4,
}

/// 错误事件
#[derive(Debug)]
pub struct ErrorEvent {
    pub error: ConcurrencyError,
    pub severity: ErrorSeverity,
    pub timestamp: Instant,
    pub thread_id: usize,
    pub context: String,
    pub recoverable: bool,
}

impl ErrorEvent {
    pub fn new(error: ConcurrencyError, severity: ErrorSeverity, thread_id: usize, context: String) -> Self {
        Self {
            error,
            severity,
            timestamp: Instant::now(),
            thread_id,
            context,
            recoverable: Self::is_recoverable(&error, &severity),
        }
    }

    fn is_recoverable(error: &ConcurrencyError, severity: &ErrorSeverity) -> bool {
        match (error, severity) {
            (ConcurrencyError::Timeout { .. }, ErrorSeverity::Error) => true,
            (ConcurrencyError::ResourceContention { .. }, ErrorSeverity::Error) => true,
            (ConcurrencyError::NetworkError { .. }, ErrorSeverity::Error) => true,
            (ConcurrencyError::IoError { .. }, ErrorSeverity::Error) => true,
            (ConcurrencyError::Deadlock { .. }, _) => false,
            (ConcurrencyError::DataRace { .. }, _) => false,
            (ConcurrencyError::Fatal, _) => false,
            _ => *severity <= ErrorSeverity::Warning,
        }
    }
}
```

## 错误恢复策略

### 重试机制
```rust
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// 重试策略
#[derive(Debug, Clone)]
pub enum RetryStrategy {
    /// 固定间隔重试
    Fixed {
        interval: Duration,
        max_attempts: u32,
    },
    /// 指数退避重试
    Exponential {
        base_interval: Duration,
        max_interval: Duration,
        max_attempts: u32,
        multiplier: f64,
    },
    /// 线性退避重试
    Linear {
        base_interval: Duration,
        max_interval: Duration,
        max_attempts: u32,
        increment: Duration,
    },
    /// 自适应重试
    Adaptive {
        initial_interval: Duration,
        max_interval: Duration,
        max_attempts: u32,
        success_threshold: f64,
    },
}

/// 重试配置
#[derive(Debug, Clone)]
pub struct RetryConfig {
    pub strategy: RetryStrategy,
    pub jitter: bool,
    pub on_retry: Option<Arc<dyn Fn(u32, &Duration, &any::Error) + Send + Sync>>,
}

impl RetryConfig {
    /// 默认重试配置
    pub fn default() -> Self {
        Self {
            strategy: RetryStrategy::Exponential {
                base_interval: Duration::from_millis(100),
                max_interval: Duration::from_secs(30),
                max_attempts: 3,
                multiplier: 2.0,
            },
            jitter: true,
            on_retry: None,
        }
    }

    /// 计算重试延迟
    pub fn calculate_delay(&self, attempt: u32) -> Duration {
        let base_delay = match &self.strategy {
            RetryStrategy::Fixed { interval, .. } => *interval,
            RetryStrategy::Exponential { base_interval, max_interval, max_attempts, multiplier } => {
                if attempt == 0 {
                    *base_interval
                } else {
                    let delay = base_interval.as_secs_f64() * multiplier.powi(attempt as i32 - 1);
                    let max_delay = max_interval.as_secs_f64();
                    Duration::from_secs_f64(delay.min(max_delay))
                }
            }
            RetryStrategy::Linear { base_interval, max_interval, increment, .. } => {
                let delay = base_interval + *increment * (attempt as u32);
                std::cmp::min(delay, *max_interval)
            }
            RetryStrategy::Adaptive { initial_interval, max_interval, .. } => {
                // 简化的自适应策略，基于成功率调整
                *initial_interval
            }
        };

        // 添加抖动
        if self.jitter {
            let jitter_range = base_delay.as_secs_f64() * 0.1; // 10%的抖动
            let jitter = (rand::random::<f64>() - 0.5) * 2.0 * jitter_range;
            let final_delay = base_delay.as_secs_f64() + jitter;
            Duration::from_secs_f64(final_delay.max(0.0))
        } else {
            base_delay
        }
    }

    /// 判断是否应该重试
    pub fn should_retry(&self, attempt: u32, error: &any::Error) -> bool {
        if attempt == 0 {
            return true;
        }

        let max_attempts = match &self.strategy {
            RetryStrategy::Fixed { max_attempts, .. } => *max_attempts,
            RetryStrategy::Exponential { max_attempts, .. } => *max_attempts,
            RetryStrategy::Linear { max_attempts, .. } => *max_attempts,
            RetryStrategy::Adaptive { max_attempts, .. } => *max_attempts,
        };

        if attempt >= max_attempts {
            return false;
        }

        // 检查错误类型是否可重试
        if let Some(concurrency_error) = error.downcast_ref::<ConcurrencyError>() {
            match concurrency_error {
                ConcurrencyError::Timeout { .. } => true,
                ConcurrencyError::ResourceContention { .. } => true,
                ConcurrencyError::NetworkError { .. } => true,
                ConcurrencyError::IoError { .. } => true,
                ConcurrencyError::Deadlock { .. } => false,
                ConcurrencyError::DataRace { .. } => false,
                ConcurrencyError::MemoryError { .. } => false,
                ConcurrencyError::SystemError { .. } => false,
                ConcurrencyError::Custom { .. } => true, // 默认可重试
            }
        } else {
            true // 默认可重试未知错误
        }
    }
}

/// 重试执行器
pub struct RetryExecutor {
    config: RetryConfig,
    stats: Arc<RetryStats>,
}

#[derive(Debug, Default)]
pub struct RetryStats {
    pub total_attempts: AtomicU64,
    pub successful_retries: AtomicU64,
    pub failed_retries: AtomicU64,
    pub total_delay: AtomicU64, // 毫秒
}

impl RetryExecutor {
    pub fn new(config: RetryConfig) -> Self {
        Self {
            config,
            stats: Arc::new(RetryStats::default()),
        }
    }

    /// 执行带重试的操作
    pub async fn execute<F, T, E>(&self, operation: F) -> Result<T, E>
    where
        F: Fn() -> Result<T, E>,
        E: std::error::Error + Send + Sync + 'static,
        T: Send,
    {
        let mut attempt = 0;

        loop {
            self.stats.total_attempts.fetch_add(1, Ordering::Relaxed);

            match operation() {
                Ok(result) => {
                    if attempt > 0 {
                        self.stats.successful_retries.fetch_add(1, Ordering::Relaxed);
                    }
                    return Ok(result);
                }
                Err(error) => {
                    let error_any: &dyn std::error::Error = &error;

                    // 调用重试回调
                    if let Some(ref callback) = self.config.on_retry {
                        let delay = self.config.calculate_delay(attempt);
                        callback(attempt, &delay, error_any);
                    }

                    if !self.config.should_retry(attempt, error_any) {
                        self.stats.failed_retries.fetch_add(1, Ordering::Relaxed);
                        return Err(error);
                    }

                    let delay = self.config.calculate_delay(attempt);
                    self.stats.total_delay.fetch_add(delay.as_millis() as u64, Ordering::Relaxed);

                    tokio::time::sleep(delay).await;
                    attempt += 1;
                }
            }
        }
    }

    /// 获取重试统计
    pub fn get_stats(&self) -> RetryStatsSnapshot {
        RetryStatsSnapshot {
            total_attempts: self.stats.total_attempts.load(Ordering::Relaxed),
            successful_retries: self.stats.successful_retries.load(Ordering::Relaxed),
            failed_retries: self.stats.failed_retries.load(Ordering::Relaxed),
            total_delay_ms: self.stats.total_delay.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug)]
pub struct RetryStatsSnapshot {
    pub total_attempts: u64,
    pub successful_retries: u64,
    pub failed_retries: u64,
    pub total_delay_ms: u64,
}

impl RetryStatsSnapshot {
    pub fn success_rate(&self) -> f64 {
        if self.total_attempts == 0 {
            0.0
        } else {
            (self.total_attempts - self.failed_retries) as f64 / self.total_attempts as f64
        }
    }

    pub fn average_delay_ms(&self) -> f64 {
        if self.total_attempts == 0 {
            0.0
        } else {
            self.total_delay_ms as f64 / self.total_attempts as f64
        }
    }
}
```

## 断路器模式

### 熔断器实现
```rust
use std::sync::atomic::{AtomicU64, AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// 断路器状态
#[derive(Debug, Clone, PartialEq)]
pub enum CircuitBreakerState {
    Closed,      // 关闭状态，正常工作
    Open,        // 打开状态，停止调用
    HalfOpen,    // 半开状态，允许少量测试调用
}

/// 断路器配置
#[derive(Debug, Clone)]
pub struct CircuitBreakerConfig {
    /// 失败阈值
    pub failure_threshold: u64,
    /// 成功阈值（用于从半开状态恢复）
    pub success_threshold: u64,
    /// 超时时间
    pub timeout: Duration,
    /// 断开时间
    pub recovery_timeout: Duration,
    /// 最小调用次数
    pub min_calls: u64,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            failure_threshold: 5,
            success_threshold: 3,
            timeout: Duration::from_secs(5),
            recovery_timeout: Duration::from_secs(30),
            min_calls: 10,
        }
    }
}

/// 断路器统计
#[derive(Debug, Default)]
pub struct CircuitBreakerStats {
    pub total_calls: AtomicU64,
    pub successful_calls: AtomicU64,
    pub failed_calls: AtomicU64,
    pub timeout_calls: AtomicU64,
    pub rejected_calls: AtomicU64,
}

/// 断路器
pub struct CircuitBreaker {
    config: CircuitBreakerConfig,
    state: Arc<AtomicU64>, // 使用AtomicU64存储状态枚举
    last_failure_time: Arc<AtomicU64>, // 存储时间戳
    failure_count: Arc<AtomicU64>,
    success_count: AtomicU64,
    stats: Arc<CircuitBreakerStats>,
}

impl CircuitBreaker {
    pub fn new(config: CircuitBreakerConfig) -> Self {
        Self {
            config,
            state: Arc::new(AtomicU64::new(CircuitBreakerState::Closed as u64)),
            last_failure_time: Arc::new(AtomicU64::new(0)),
            failure_count: Arc::new(AtomicU64::new(0)),
            success_count: AtomicU64::new(0),
            stats: Arc::new(CircuitBreakerStats::default()),
        }
    }

    /// 获取当前状态
    pub fn state(&self) -> CircuitBreakerState {
        let state_val = self.state.load(Ordering::Acquire);
        match state_val {
            0 => CircuitBreakerState::Closed,
            1 => CircuitBreakerState::Open,
            2 => CircuitBreakerState::HalfOpen,
            _ => CircuitBreakerState::Closed,
        }
    }

    /// 设置状态
    fn set_state(&self, new_state: CircuitBreakerState) {
        let state_val = match new_state {
            CircuitBreakerState::Closed => 0,
            CircuitBreakerState::Open => 1,
            CircuitBreakerState::HalfOpen => 2,
        };
        self.state.store(state_val, Ordering::Release);
    }

    /// 检查是否允许调用
    pub fn allow_call(&self) -> bool {
        match self.state() {
            CircuitBreakerState::Closed => true,
            CircuitBreakerState::Open => {
                let last_failure = self.last_failure_time.load(Ordering::Relaxed);
                let elapsed = Instant::now().duration_since(
                    std::time::UNIX_EPOCH + Duration::from_millis(last_failure)
                );

                if elapsed >= self.config.recovery_timeout {
                    self.set_state(CircuitBreakerState::HalfOpen);
                    self.success_count.store(0, Ordering::Relaxed);
                    true
                } else {
                    false
                }
            }
            CircuitBreakerState::HalfOpen => {
                let success_count = self.success_count.load(Ordering::Relaxed);
                success_count < self.config.success_threshold
            }
        }
    }

    /// 记录成功调用
    pub fn record_success(&self) {
        self.stats.successful_calls.fetch_add(1, Ordering::Relaxed);

        match self.state() {
            CircuitBreakerState::HalfOpen => {
                let success_count = self.success_count.fetch_add(1, Ordering::Relaxed) + 1;
                if success_count >= self.config.success_threshold {
                    self.set_state(CircuitBreakerState::Closed);
                    self.failure_count.store(0, Ordering::Relaxed);
                }
            }
            CircuitBreakerState::Open => {
                // 不应该发生，但为了安全处理
                self.set_state(CircuitBreakerState::Closed);
                self.failure_count.store(0, Ordering::Relaxed);
            }
            CircuitBreakerState::Closed => {
                // 重置失败计数（如果需要）
                if self.failure_count.load(Ordering::Relaxed) > 0 {
                    self.failure_count.store(0, Ordering::Relaxed);
                }
            }
        }
    }

    /// 记录失败调用
    pub fn record_failure(&self) {
        self.stats.failed_calls.fetch_add(1, Ordering::Relaxed);

        match self.state() {
            CircuitBreakerState::Closed => {
                let failure_count = self.failure_count.fetch_add(1, Ordering::Relaxed) + 1;

                let total_calls = self.stats.total_calls.load(Ordering::Relaxed);
                if total_calls >= self.config.min_calls &&
                   failure_count >= self.config.failure_threshold {
                    self.set_state(CircuitBreakerState::Open);
                    let now = Instant::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .as_millis() as u64;
                    self.last_failure_time.store(now, Ordering::Relaxed);
                }
            }
            CircuitBreakerState::HalfOpen => {
                self.set_state(CircuitBreakerState::Open);
                let now = Instant::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .as_millis() as u64;
                self.last_failure_time.store(now, Ordering::Relaxed);
            }
            CircuitBreakerState::Open => {
                // 已经是打开状态
            }
        }
    }

    /// 记录超时调用
    pub fn record_timeout(&self) {
        self.stats.timeout_calls.fetch_add(1, Ordering::Relaxed);
        self.record_failure(); // 超时视为失败
    }

    /// 执行带断路器的操作
    pub async fn execute<F, T, E>(&self, operation: F) -> Result<T, E>
    where
        F: FnOnce() -> Result<T, E>,
        E: std::error::Error + Send + Sync + 'static,
        T: Send,
    {
        self.stats.total_calls.fetch_add(1, Ordering::Relaxed);

        if !self.allow_call() {
            self.stats.rejected_calls.fetch_add(1, Ordering::Relaxed);
            return Err(ConcurrentError::Custom {
                error_code: 5000,
                message: "Circuit breaker is open".to_string(),
                context: "Call rejected by circuit breaker".to_string(),
            }.into());
        }

        let start_time = Instant::now();

        // 设置超时
        let result = tokio::time::timeout(self.config.timeout, async {
            operation()
        }).await;

        match result {
            Ok(Ok(value)) => {
                self.record_success();
                Ok(value)
            }
            Ok(Err(error)) => {
                self.record_failure();
                Err(error)
            }
            Err(_) => {
                self.record_timeout();
                Err(ConcurrentError::Timeout {
                    operation: "CircuitBreaker operation".to_string(),
                    duration: self.config.timeout,
                }.into())
            }
        }
    }

    /// 获取统计信息
    pub fn get_stats(&self) -> CircuitBreakerStatsSnapshot {
        let total_calls = self.stats.total_calls.load(Ordering::Relaxed);
        let successful_calls = self.stats.successful_calls.load(Ordering::Relaxed);
        let failed_calls = self.stats.failed_calls.load(Ordering::Relaxed);
        let timeout_calls = self.stats.timeout_calls.load(Ordering::Relaxed);
        let rejected_calls = self.stats.rejected_calls.load(Ordering::Relaxed);

        CircuitBreakerStatsSnapshot {
            state: self.state(),
            total_calls,
            successful_calls,
            failed_calls,
            timeout_calls,
            rejected_calls,
            failure_count: self.failure_count.load(Ordering::Relaxed),
            success_count: self.success_count.load(Ordering::Relaxed),
        }
    }

    /// 重置断路器
    pub fn reset(&self) {
        self.set_state(CircuitBreakerState::Closed);
        self.failure_count.store(0, Ordering::Relaxed);
        self.success_count.store(0, Ordering::Relaxed);

        // 重置统计
        self.stats.total_calls.store(0, Ordering::Relaxed);
        self.stats.successful_calls.store(0, Ordering::Relaxed);
        self.stats.failed_calls.store(0, Ordering::Relaxed);
        self.stats.timeout_calls.store(0, Ordering::Relaxed);
        self.stats.rejected_calls.store(0, Ordering::Relaxed);
    }
}

#[derive(Debug)]
pub struct CircuitBreakerStatsSnapshot {
    pub state: CircuitBreakerState,
    pub total_calls: u64,
    pub successful_calls: u64,
    pub failed_calls: u64,
    pub timeout_calls: u64,
    pub rejected_calls: u64,
    pub failure_count: u64,
    pub success_count: u64,
}

impl CircuitBreakerStatsSnapshot {
    pub fn success_rate(&self) -> f64 {
        if self.total_calls == 0 {
            0.0
        } else {
            self.successful_calls as f64 / self.total_calls as f64
        }
    }

    pub fn failure_rate(&self) -> f64 {
        if self.total_calls == 0 {
            0.0
        } else {
            (self.failed_calls + self.timeout_calls) as f64 / self.total_calls as f64
        }
    }
}

/// 断路器使用示例
async fn circuit_breaker_example() {
    let config = CircuitBreakerConfig {
        failure_threshold: 3,
        success_threshold: 2,
        timeout: Duration::from_secs(2),
        recovery_timeout: Duration::from_secs(10),
        min_calls: 5,
    };

    let circuit_breaker = Arc::new(CircuitBreaker::new(config));

    // 模拟外部服务调用
    let external_service = || {
        // 模拟70%的成功率
        if rand::random::<f64>() < 0.7 {
            Ok("Service response".to_string())
        } else {
            Err(ConcurrentError::NetworkError {
                operation: "external_api_call".to_string(),
                peer: "external.service.com".to_string(),
                error_code: Some(500),
            })
        }
    };

    // 测试断路器
    for i in 0..20 {
        println!("Attempt {}", i + 1);

        match circuit_breaker.execute(external_service).await {
            Ok(response) => println!("Success: {}", response),
            Err(e) => println!("Error: {}", e),
        }

        let stats = circuit_breaker.get_stats();
        println!("Circuit Breaker State: {:?}", stats.state);
        println!("Success Rate: {:.2}%", stats.success_rate() * 100.0);
        println!("Failure Rate: {:.2}%", stats.failure_rate() * 100.0);
        println!("---");

        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}
```

## 相关专题

- `../sync/mutex-rwlock.md` - 同步原语错误处理
- `../async-io/async-file-operations.md` - 异步I/O错误处理
- `../debugging/concurrent-bugs.md` - 并发Bug调试
- `../patterns/concurrent-patterns.md` - 并发设计模式