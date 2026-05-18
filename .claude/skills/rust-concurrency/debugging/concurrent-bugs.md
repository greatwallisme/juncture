# 并发Bug调试技术

## 当使用此专题

- 调试复杂的并发程序
- 定位竞态条件和死锁
- 分析性能瓶颈
- 验证并发正确性

## 核心功能

### 并发bug分析
```
Analyze concurrent bugs: target=src/, detect_races=true, detect_deadlocks=true, check_memory_safety=true
```
分析代码中的并发问题

### 死锁检测
```
Detect deadlocks: code=src/, lock_graph=true, suggest_fixes=true, visual=true
```
检测和分析潜在的死锁场景

### 性能瓶颈分析
```
Profile concurrency: target=src/, identify_bottlenecks=true, timeline_analysis=true, flamegraph=true
```
分析并发程序的性能瓶颈

## 常见并发Bug类型

### 1. 竞态条件 (Race Conditions)

#### 识别模式
```rust
// ❌ 竞态条件示例
static mut SHARED_DATA: Vec<i32> = vec![];

fn bad_concurrent_access() {
    for i in 0..1000 {
        // 可能的竞态条件
        SHARED_DATA.push(i);
    }
}

// ✅ 正确的并发访问
use std::sync::Mutex;
static SHARED_DATA_SAFE: Mutex<Vec<i32>> = Mutex::new(vec![]);

fn safe_concurrent_access() {
    for i in 0..1000 {
        SHARED_DATA_SAFE.lock().unwrap().push(i);
    }
}
```

#### 调试技巧
```rust
// 使用thread local存储避免竞态
thread_local! {
    static THREAD_DATA: std::cell::RefCell<Vec<i32>> = std::cell::RefCell::new(vec![]);
}

fn debug_race_condition() {
    // 在调试模式下启用额外检查
    #[cfg(debug_assertions)]
    {
        // 验证不变量
        assert_eq!(SHARED_DATA_SAFE.lock().unwrap().len(), expected_length);
    }

    // 使用有界集合进行调试
    let bounded_queue = std::sync::Arc::new(std::sync::Mutex::new(
        std::collections::VecDeque::with_capacity(100)
    ));
}
```

### 2. 死锁 (Deadlocks)

#### 识别模式
```rust
use std::sync::{Arc, Mutex, Condvar};

struct DeadlockExample {
    mutex1: Arc<Mutex<()>>,
    mutex2: Arc<Mutex<()>>,
    condvar: Condvar,
}

impl DeadlockExample {
    fn new() -> Self {
        Self {
            mutex1: Arc::new(Mutex::new(())),
            mutex2: Arc::new(Mutex::new(())),
            condvar: Condvar::new(),
        }
    }

    // ❌ 可能导致死锁
    fn potential_deadlock(&self) {
        let _lock1 = self.mutex1.lock().unwrap();
        thread::sleep(std::time::Duration::from_millis(100));
        let _lock2 = self.mutex2.lock().unwrap(); // 可能死锁
    }

    // ✅ 固定锁顺序
    fn fixed_lock_order(&self) {
        let lock1 = self.mutex1.lock().unwrap();
        let _lock2 = self.mutex2.lock().unwrap(); // 固定顺序
    }

    // ✅ 使用超时
    fn timeout_safe_locks(&self) -> Result<(), std::sync::PoisonError<MutexGuard<()>>> {
        let timeout = std::time::Duration::from_secs(5);
        let lock1 = std::sync::Mutex::try_lock_for_duration(&self.mutex1, timeout)?;
        let _lock2 = std::sync::Mutex::try_lock_for_duration(&self.mutex2, timeout)?;
        Ok(())
    }
}
```

#### 死锁检测工具
```rust
use std::collections::HashSet;
use std::thread;
use std::sync::{Arc, Mutex};

pub struct DeadlockDetector {
    lock_graph: Arc<Mutex<std::collections::HashMap<String, HashSet<String>>>>,
    current_held_locks: std::sync::atomic::AtomicU64,
}

impl DeadlockDetector {
    pub fn new() -> Self {
        Self {
            lock_graph: Arc::new(Mutex::new(HashMap::new())),
            current_held_locks: std::sync::atomic::AtomicU64::new(0),
        }
    }

    pub fn acquire_lock(&self, lock_id: &str) -> DeadlockGuard {
        let thread_id = get_thread_id();
        let lock_graph = Arc::clone(&self.lock_graph);

        DeadlockGuard {
            lock_id: lock_id.to_string(),
            thread_id,
            lock_graph,
            detector: self,
            _guard: std::marker::PhantomPinned,
        }
    }

    fn add_edge(&self, from: &str, to: &str) {
        let mut graph = self.lock_graph.lock().unwrap();
        graph.entry(from.to_string()).or_insert_with(HashSet::new()).insert(to.to_string());
    }

    fn check_for_cycle(&self) -> bool {
        let graph = self.lock_graph.lock().unwrap();

        let mut visited = HashSet::new();
        let recursion_stack = Vec::new();

        for start_node in graph.keys() {
            if !visited.contains(start_node) {
                if self.dfs_check(start_node, &graph, &mut visited, &mut recursion_stack) {
                    return true;
                }
            }
        }

        false
    }

    fn dfs_check(
        &self,
        node: &str,
        graph: &HashMap<String, HashSet<String>>,
        visited: &mut HashSet<String>,
        stack: &mut Vec<String>,
    ) -> bool {
        if visited.contains(node) {
            return true; // 检测到循环
        }

        visited.insert(node.to_string());
        stack.push(node.to_string());

        if let Some(neighbors) = graph.get(node) {
            for neighbor in neighbors {
                if self.dfs_check(neighbor, graph, visited, stack) {
                    return true;
                }
            }
        }

        stack.pop();
        false
    }
}

pub struct DeadlockGuard {
    lock_id: String,
    thread_id: u64,
    lock_graph: Arc<Mutex<std::collections::HashMap<String, HashSet<String>>>>,
    detector: *const DeadlockDetector,
    _guard: std::marker::PhantomPinned,
}

impl Drop for DeadlockGuard {
    fn drop(&mut self) {
        self.detector.current_held_locks.fetch_sub(1, std::sync::atomic::Ordering::Release);
    }
}
```

### 3. 活锁 (Livelock)

#### 识别模式
```rust
use std::sync::Arc;
use std::thread;
use std::time::Duration;

struct LiveLockExample {
    workers: Vec<Arc<Mutex<bool>>,
    should_continue: Arc<Mutex<bool>>,
}

impl LiveLockExample {
    fn new(worker_count: usize) -> Self {
        let mut workers = Vec::new();
        let should_continue = Arc::new(Mutex::new(true));

        for _ in 0..worker_count {
            workers.push(Arc::new(Mutex::new(false)));
        }

        Self {
            workers,
            should_continue,
        }
    }

    // ❌ 活锁示例
    fn busy_wait(&self) {
        loop {
            if *self.should_continue.lock().unwrap() {
                thread::sleep(Duration::from_millis(1));
                continue;
            } else {
                break;
            }
        }
    }

    // ✅ 使用条件变量
    fn condition_wait(&self) {
        let condvar = std::sync::Condvar::new();
        let should_continue = self.should_continue.clone();
        let mut should_continue_guard = should_continue.lock().unwrap();

        while !*should_continue_guard {
            should_continue_guard = condvar.wait(should_continue_guard).unwrap();
        }
    }
}
```

### 4. 悬空指针和内存安全问题

#### 识别模式
```rust
// ❌ 悬空指针风险
struct UnsafeNode {
    next: *mut UnsafeNode,
    data: Option<String>,
}

impl UnsafeNode {
    fn use_data(&self) -> Option<&String> {
        unsafe { &self.data } }
    }

    fn set_data(&mut self, data: String) {
        unsafe { self.data = Some(data); }
    }
}

// ✅ 使用Option和借用检查器
struct SafeNode {
    next: Option<Box<SafeNode>>,
    data: String,
}

impl SafeNode {
    fn get_data(&self) -> &String {
        &self.data
    }

    fn next_node(&self) -> Option<&SafeNode> {
        self.next.as_ref().map(|node| node.as_ref())
    }
}
```

## 🔧 调试工具和技术

### 1. 日志记录
```rust
use std::sync::{Arc, Mutex};
use std::time::Instant;
use std::fs::{OpenOptions, File};

pub struct ThreadLogger {
    log_file: Arc<Mutex<File>>,
    start_time: std::time::Instant,
}

impl ThreadLogger {
    pub fn new(filename: &str) -> std::io::Result<Self> {
        let file = File::create(filename)?;
        Ok(Self {
            log_file: Arc::new(Mutex::new(file)),
            start_time: Instant::now(),
        })
    }

    pub fn log(&self, thread_id: u64, event: &str) {
        let timestamp = self.start_time.elapsed().as_millis();
        let log_line = format!("[{}ms] [{}] {}\n", timestamp, thread_id, event);

        if let Ok(mut file) = self.log_file.lock().write(log_line.as_bytes()) {
            // 日志写入成功
        }
    }

    pub fn log_lock_operation(&self, thread_id: u64, lock_name: &str, operation: &str) {
        self.log(thread_id, &format!("Lock {} on {}", operation, operation, lock_name));
    }

    pub fn log_data_race(&self, thread_id: u64, variable: &str, action: &str) {
        self.log(thread_id, &format!("Data race: {} on {}", action, variable));
    }
}

thread_local! {
    static THREAD_LOGGER: std::cell::OnceCell<ThreadLogger> = std::cell::OnceCell::new();
}

pub fn log_event(event: &str) {
    THREAD_LOGGER.with(|logger| {
        logger.log(get_thread_id(), event);
    });
}
```

### 2. 断言检查
```rust
use std::sync::{Arc, Mutex};

pub struct AssertionChecker {
    invariants: Vec<Box<dyn Fn() -> bool + Send + Sync>>,
}

impl AssertionChecker {
    pub fn new() -> Self {
        Self {
            invariants: Vec::new(),
        }
    }

    pub fn add_invariant<F>(&mut self, invariant: F)
    where
        F: Fn() -> bool + Send + Sync + 'static,
    {
        self.invariants.push(Box::new(invariant));
    }

    pub fn check_all(&self) {
        for invariant in &self.invariants {
            let is_valid = invariant();
            if !is_valid {
                panic!("Invariant check failed!");
            }
        }
    }
}

pub struct CheckedData<T> {
    data: T,
    checker: Arc<AssertionChecker>,
}

impl<T> CheckedData<T> {
    pub fn new(data: T, checker: Arc<AssertionChecker>) -> Self {
        checker.check_all();
        Self { data, checker }
    }

    pub fn get(&self) -> &T {
        self.checker.check_all();
        &self.data
    }

    pub fn update<F>(&mut self, updater: F) -> T
    where
        F: FnOnce(&mut T) -> T,
    {
        self.checker.check_all();
        let result = updater(&mut self.data);
        self.checker.check_all();
        result
    }
}
```

### 3. 内存泄漏检测
```rust
use std::sync::Arc;
use std::collections::HashMap;

pub struct LeakDetector {
    allocations: Arc<Mutex<HashMap<usize, AllocationInfo>>>,
    deallocations: Arc<Mutex<HashSet<usize>>>,
}

struct AllocationInfo {
    size: usize,
    location: String,
    timestamp: std::time::Instant,
}

impl LeakDetector {
    pub fn new() -> Self {
        Self {
            allocations: Arc::new(Mutex::new(HashMap::new())),
            deallocations: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    pub fn track_allocation(&self, ptr: usize, size: usize, location: &str) {
        let info = AllocationInfo {
            size,
            location: location.to_string(),
            timestamp: std::time::Instant::now(),
        };

        let mut allocations = self.allocations.lock().unwrap();
        allocations.insert(ptr, info);

        // 同时移除对应的释放记录
        self.deallocations.lock().unwrap().remove(&ptr);
    }

    pub fn track_deallocation(&self, ptr: usize) {
        let mut deallocations = self.deallocations.lock().unwrap();
        deallocations.insert(ptr);

        // 检查是否泄漏
        if self.has_leaks() {
            let leaks = self.get_leaks();
            if !leaks.is_empty() {
                println!("Memory leaks detected:");
                for leak in leaks {
                    println!("  {} bytes at {}", leak.size, leak.location);
                }
            }
        }
    }

    pub fn has_leaks(&self) -> bool {
        let allocations = self.allocations.lock().unwrap();
        let deallocations = self.deallocations.lock().unwrap();

        !allocations.keys().all(|ptr| deallocations.contains(ptr))
    }

    pub fn get_leaks(&self) -> Vec<AllocationInfo> {
        let allocations = self.allocations.lock().unwrap();
        let deallocations = self.deallocations.lock().unwrap();

        allocations.values()
            .filter(|info| !deallocations.contains(&info.location.as_ptr() as usize))
            .cloned()
            .collect()
    }
}

impl Drop for LeakDetector {
    fn drop(&mut self) {
        if self.has_leaks() {
            println!("Final memory leak check:");
            for leak in self.get_leaks() {
                println!("  {} bytes at {} (allocated {:?})",
                    leak.size, leak.location,
                    leak.timestamp.elapsed());
            }
        }
    }
}
```

### 4. 性能分析
```rust
use std::time::{Duration, Instant};
use std::collections::VecDeque;

pub struct PerformanceProfiler {
    measurements: VecDeque<Measurement>,
    start_time: Instant,
}

#[derive(Debug, Clone)]
pub struct Measurement {
    timestamp: Instant,
    duration: Duration,
    event: String,
    thread_id: u64,
}

impl PerformanceProfiler {
    pub fn new() -> Self {
        Self {
            measurements: VecDeque::new(),
            start_time: Instant::now(),
        }
    }

    pub fn start_measurement(&self, event: &str) {
        self.measurements.push(Measurement {
            timestamp: Instant::now(),
            duration: Duration::ZERO,
            event: event.to_string(),
            thread_id: get_thread_id(),
        });
    }

    pub fn end_measurement(&self, event: &str) {
        if let Some(measurement) = self.measurements.iter_mut()
            .find(|m| m.event == event && m.duration == Duration::ZERO) {
            measurement.duration = measurement.timestamp.elapsed();
            measurement.timestamp = Instant::now();
        }
    }

    pub fn get_report(&self) -> PerformanceReport {
        let mut report = PerformanceReport::new();

        for measurement in &self.measurements {
            if measurement.duration > Duration::ZERO {
                report.total_time += measurement.duration;
                report.events.push(measurement.clone());
            }
        }

        report
    }
}

#[derive(Debug)]
pub struct PerformanceReport {
    pub total_time: Duration,
    pub events: Vec<Measurement>,
}

impl PerformanceReport {
    pub fn new() -> Self {
        Self {
            total_time: Duration::ZERO,
            events: Vec::new(),
        }
    }

    pub fn print_summary(&self) {
        println!("Performance Summary:");
        println!("  Total time: {:?}", self.total_time);
        println!("  Event count: {}", self.events.len());

        // 按持续时间排序
        let mut sorted_events = self.events.clone();
        sorted_events.sort_by(|a, b| b.duration.cmp(&a.duration));

        for measurement in sorted_events.iter().take(10) {
            println!("  {}: {:?} ({})", measurement.event, measurement.duration, measurement.thread_id);
        }
    }
}

// 使用RAII包装器来自动跟踪资源
pub struct RAIIProfiler<T> {
    resource: T,
    profiler: Arc<PerformanceProfiler>,
    name: String,
    start_time: Instant,
}

impl<T> RAIIProfiler<T> {
    pub fn new(resource: T, profiler: Arc<PerformanceProfiler>, name: &str) -> Self {
        let profiler_clone = Arc::clone(&profiler);
        profiler.start_measurement(&format!("create_{}", name));

        Self {
            resource,
            profiler: profiler_clone,
            name: name.to_string(),
            start_time: Instant::now(),
        }
    }
}

impl<T> std::ops::Deref for RAIIProfiler<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.resource
    }
}

impl<T> Drop for RAIIProfiler<T> {
    fn drop(&mut self) {
        self.profiler.end_measurement(&format!("drop_{}", self.name));
        let duration = self.start_time.elapsed();
        println!("Resource '{}' lived for {:?}", self.name, duration);
    }
}
```

## 📁 调试命令

### 竞争条件检测
```bash
# 使用ThreadSanitizer检测竞态条件
cargo test --target x86_64-unknown-linux-gnu -- -- -Z sanitizer=thread

# 使用Helgrind检测内存错误
valgrind --tool=helgrind ./target/debug/myapp

# 使用AddressSanitizer检测地址错误
cargo test --target x86_64-unknown-linux-gnu -- -- -Z sanitizer=address
```

### 死锁检测
```bash
# 运行程序并等待死锁
timeout 10s ./target/debug/myapp

# 使用gdb调试死锁
gdb -batch deadlock-debugger.gdb ./target/debug/myapp
```

### 性能分析
```bash
# 使用perf进行性能分析
perf record -g ./target/release/myapp
perf report

# 使用flamegraph生成调用图
cargo flamegraph --bin target/release/myapp

# 使用Intel VTune分析缓存性能
vtune ./target/release/myapp
```

## 最佳实践

### 1. 防御性编程
```rust
// 使用Rust的类型系统预防内存错误
struct SafeLinkedList<T> {
    head: Option<Box<Node<T>>>,
}

struct Node<T> {
    data: T,
    next: Option<Box<Node<T>>>,
}

impl<T> SafeLinkedList<T> {
    pub fn push(&mut self, data: T) {
        let new_node = Node {
            data,
            next: None,
        };

        match self.head {
            None => {
                self.head = Some(Box::new(new_node));
            }
            Some(head) => {
                // 创建新的头节点，避免修改已有节点
                let new_head = Node {
                    data,
                    next: Some(head),
                };
                self.head = Some(Box::new(new_head));
            }
        }
    }
}
```

### 2. 单元测试并发代码
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    #[test]
    fn test_concurrent_access() {
        let data = Arc::new(Mutex::new(vec![]));
        let mut handles = vec![];

        for i in 0..10 {
            let data_clone = Arc::clone(&data);
            handles.push(thread::spawn(move || {
                data_clone.lock().unwrap().push(i);
            }));
        }

        for handle in handles {
            handle.join().unwrap();
        }

        let final_data = data.lock().unwrap();
        assert_eq!(final_data.len(), 10);

        // 验证数据顺序
        for (i, &item) in final_data.iter().enumerate() {
            assert_eq!(item, i as i32);
        }
    }

    #[test]
    fn test_deadlock_prevention() {
        let mutex1 = Arc::new(Mutex::new(()));
        let mutex2 = Arc::new(Mutex::new(()));

        let handle1 = thread::spawn({
            let m1 = mutex1.clone();
            let m2 = mutex2.clone();
            move || {
                let _lock1 = m1.lock().unwrap();
                thread::sleep(Duration::from_millis(100));
                let _lock2 = m2.lock().unwrap();
                // 按固定顺序获取锁
            }
        });

        let handle2 = thread::spawn({
            let m1 = mutex1.clone();
            let m2 = mutex2.clone();
            move || {
                let _lock2 = m2.lock().unwrap();
                let _lock1 = m1.lock().unwrap();
                // 相反的顺序，但通过固定顺序实现
            }
        });

        handle1.join().unwrap();
        handle2.join().unwrap();
    }
}
```
