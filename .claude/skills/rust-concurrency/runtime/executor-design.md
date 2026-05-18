# 异步执行器设计模式

## 当使用此专题

- 设计自定义异步执行器
- 理解任务调度算法
- 构建高性能运行时
- 优化执行器性能

## 执行器架构设计

### 基础执行器接口
```rust
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

/// 执行器核心trait
trait Executor: Send + Sync {
    /// 提交任务到执行器
    fn spawn<F>(&self, future: F) -> TaskId
    where
        F: Future<Output = ()> + Send + 'static;

    /// 阻塞直到所有任务完成
    fn block_on<F>(&self, future: F) -> F::Output
    where
        F: Future<Output = ()>;

    /// 获取执行器统计信息
    fn stats(&self) -> ExecutorStats;
}

/// 任务ID
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct TaskId(u64);

/// 执行器统计信息
#[derive(Debug, Default)]
struct ExecutorStats {
    tasks_submitted: u64,
    tasks_completed: u64,
    total_wait_time: std::time::Duration,
    avg_task_duration: std::time::Duration,
}
```

### 单线程执行器
```rust
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// 单线程执行器
pub struct SingleThreadExecutor {
    task_queue: VecDeque<Task>,
    task_counter: AtomicU64,
    running: AtomicBool,
    stats: ExecutorStats,
}

struct Task {
    id: TaskId,
    future: Pin<Box<dyn Future<Output = ()>>>,
    created_at: std::time::Instant,
}

impl SingleThreadExecutor {
    pub fn new() -> Self {
        Self {
            task_queue: VecDeque::new(),
            task_counter: AtomicU64::new(0),
            running: AtomicBool::new(false),
            stats: ExecutorStats::default(),
        }
    }

    fn run_ready_tasks(&mut self) {
        while let Some(mut task) = self.task_queue.pop_front() {
            let waker = waker_fn(move || {
                // 简单的唤醒：将任务重新加入队列
                println!("Waking task {}", task.id.0);
            });

            let mut cx = Context::from_waker(&waker);

            match task.future.as_mut().poll(&mut cx) {
                Poll::Ready(()) => {
                    let duration = task.created_at.elapsed();
                    self.stats.tasks_completed += 1;
                    self.stats.total_wait_time += duration;
                    self.stats.avg_task_duration =
                        self.stats.total_wait_time / self.stats.tasks_completed as u32;
                }
                Poll::Pending => {
                    // 任务还没完成，重新加入队列末尾
                    self.task_queue.push_back(task);
                }
            }
        }
    }
}

impl Executor for SingleThreadExecutor {
    fn spawn<F>(&self, future: F) -> TaskId
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let id = TaskId(self.task_counter.fetch_add(1, Ordering::Relaxed));
        let task = Task {
            id,
            future: Box::pin(future),
            created_at: std::time::Instant::now(),
        };

        // 线程安全地添加任务到队列
        unsafe {
            let executor_ptr = self as *const Self as *mut Self;
            (*executor_ptr).task_queue.push_back(task);
        }

        self.stats.tasks_submitted += 1;
        id
    }

    fn block_on<F>(&self, future: F) -> F::Output
    where
        F: Future<Output = ()>,
    {
        self.running.store(true, Ordering::Relaxed);

        // 运行传入的future
        pin_utils::pin_mut!(future);
        let waker = waker_fn(|| {});
        let mut cx = Context::from_waker(&waker);

        match future.poll(&mut cx) {
            Poll::Ready(()) => {},
            Poll::Pending => {
                // 运行队列中的任务
                unsafe {
                    let executor_ptr = self as *const Self as *mut Self;
                    while self.running.load(Ordering::Relaxed) {
                        (*executor_ptr).run_ready_tasks();
                        if (*executor_ptr).task_queue.is_empty() {
                            break;
                        }
                    }
                }
            }
        }
    }

    fn stats(&self) -> ExecutorStats {
        self.stats.clone()
    }
}
```

## 多线程执行器

### 工作窃取执行器
```rust
use std::sync::{Arc, Mutex};
use std::thread;
use std::collections::VecDeque;
use crossbeam::deque::{Injector, Stealer, Worker};

/// 工作窃取执行器
pub struct WorkStealingExecutor {
    global_queue: Arc<Injector<Task>>,
    workers: Vec<WorkerThread>,
    task_counter: AtomicU64,
    shutdown: AtomicBool,
}

struct WorkerThread {
    id: usize,
    local_queue: Worker<Task>,
    stealer: Stealer<Task>,
    global_queue: Arc<Injector<Task>>,
    stealers: Vec<Stealer<Task>>,
    handle: thread::JoinHandle<()>,
}

impl WorkStealingExecutor {
    pub fn new(num_workers: usize) -> Self {
        let global_queue = Arc::new(Injector::new());
        let mut stealers = Vec::new();
        let mut workers = Vec::new();

        // 创建工作线程
        for worker_id in 0..num_workers {
            let local_queue = Worker::new_fifo();
            let stealer = local_queue.stealer();
            stealers.push(stealer.clone());

            let worker = WorkerThread {
                id: worker_id,
                local_queue,
                stealer,
                global_queue: Arc::clone(&global_queue),
                stealers: stealers.clone(),
                handle: thread::spawn(|| {}),
            };
            workers.push(worker);
        }

        Self {
            global_queue,
            workers,
            task_counter: AtomicU64::new(0),
            shutdown: AtomicBool::new(false),
        }
    }

    pub fn start(&mut self) {
        for worker in &mut self.workers {
            let local_queue = std::mem::replace(&mut worker.local_queue, Worker::new_fifo());
            let stealer = local_queue.stealer();
            let global_queue = Arc::clone(&worker.global_queue);
            let stealers = worker.stealers.clone();
            let worker_id = worker.id;
            let shutdown = Arc::new(AtomicBool::new(false));

            let shutdown_clone = Arc::clone(&shutdown);
            worker.handle = thread::spawn(move || {
                Self::worker_loop(
                    worker_id,
                    local_queue,
                    stealer,
                    global_queue,
                    stealers,
                    shutdown_clone,
                );
            });
        }
    }

    fn worker_loop(
        worker_id: usize,
        local_queue: Worker<Task>,
        stealer: Stealer<Task>,
        global_queue: Arc<Injector<Task>>,
        stealers: Vec<Stealer<Task>>,
        shutdown: Arc<AtomicBool>,
    ) {
        println!("Worker {} started", worker_id);

        while !shutdown.load(Ordering::Relaxed) {
            // 尝试从本地队列获取任务
            if let Some(task) = local_queue.pop() {
                Self::execute_task(task, worker_id, &local_queue, &global_queue);
                continue;
            }

            // 尝试从全局队列获取任务
            if let Some(task) = global_queue.steal() {
                Self::execute_task(task, worker_id, &local_queue, &global_queue);
                continue;
            }

            // 尝试从其他工作线程窃取任务
            if let Some(task) = Self::steal_task(&stealers, worker_id) {
                Self::execute_task(task, worker_id, &local_queue, &global_queue);
                continue;
            }

            // 没有任务，短暂休眠
            thread::sleep(std::time::Duration::from_millis(1));
        }

        println!("Worker {} shutdown", worker_id);
    }

    fn execute_task(
        mut task: Task,
        worker_id: usize,
        local_queue: &Worker<Task>,
        global_queue: &Injector<Task>,
    ) {
        let waker = waker_fn(move || {
            // 将任务重新加入本地队列
            local_queue.push(task.clone());
        });

        let mut cx = Context::from_waker(&waker);

        match task.future.as_mut().poll(&mut cx) {
            Poll::Ready(()) => {
                println!("Worker {} completed task {}", worker_id, task.id.0);
            }
            Poll::Pending => {
                // 任务还没完成，重新加入队列
                local_queue.push(task);
            }
        }
    }

    fn steal_task(stealers: &[Stealer<Task>], self_id: usize) -> Option<Task> {
        // 从其他工作线程窃取任务
        for (i, stealer) in stealers.iter().enumerate() {
            if i == self_id {
                continue; // 不从自己这里窃取
            }

            if let Ok(task) = stealer.steal() {
                return Some(task);
            }
        }
        None
    }
}

impl Executor for WorkStealingExecutor {
    fn spawn<F>(&self, future: F) -> TaskId
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let id = TaskId(self.task_counter.fetch_add(1, Ordering::Relaxed));
        let task = Task {
            id,
            future: Box::pin(future),
            created_at: std::time::Instant::now(),
        };

        // 随机选择工作线程或全局队列
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        id.0.hash(&mut hasher);
        let worker_index = (hasher.finish() as usize) % self.workers.len();

        // 尝试推送到选定的工作线程
        if let Some(worker) = self.workers.get(worker_index) {
            if worker.local_queue.push(task).is_ok() {
                return id;
            }
        }

        // 推送到全局队列
        self.global_queue.push(task);
        id
    }

    fn block_on<F>(&self, future: F) -> F::Output
    where
        F: Future<Output = ()>,
    {
        // 实现block_on逻辑
        pin_utils::pin_mut!(future);
        let waker = waker_fn(|| {});
        let mut cx = Context::from_waker(&waker);

        match future.poll(&mut cx) {
            Poll::Ready(output) => output,
            Poll::Pending => {
                // 运行事件循环
                while self.global_queue.steal().is_some() {
                    thread::yield_now();
                }
                // 简化实现
                ()
            }
        }
    }

    fn stats(&self) -> ExecutorStats {
        // 收集统计信息
        ExecutorStats::default()
    }
}
```

### 优先级执行器
```rust
use std::cmp::Ordering as CmpOrdering;
use std::collections::BinaryHeap;

/// 任务优先级
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Priority {
    High = 3,
    Normal = 2,
    Low = 1,
    Background = 0,
}

/// 优先级任务
struct PriorityTask {
    task: Task,
    priority: Priority,
}

impl PartialEq for PriorityTask {
    fn eq(&self, other: &Self) -> bool {
        self.priority == other.priority
    }
}

impl Eq for PriorityTask {}

impl PartialOrd for PriorityTask {
    fn partial_cmp(&self, other: &Self) -> Option<CmpOrdering> {
        Some(self.cmp(other))
    }
}

impl Ord for PriorityTask {
    fn cmp(&self, other: &Self) -> CmpOrdering {
        // 反向排序，最大堆中优先级最高的在前
        other.priority.cmp(&self.priority)
    }
}

/// 优先级执行器
pub struct PriorityExecutor {
    task_heap: BinaryHeap<PriorityTask>,
    task_counter: AtomicU64,
    running: AtomicBool,
}

impl PriorityExecutor {
    pub fn new() -> Self {
        Self {
            task_heap: BinaryHeap::new(),
            task_counter: AtomicU64::new(0),
            running: AtomicBool::new(false),
        }
    }

    pub fn spawn_with_priority<F>(&self, future: F, priority: Priority) -> TaskId
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let id = TaskId(self.task_counter.fetch_add(1, Ordering::Relaxed));
        let task = Task {
            id,
            future: Box::pin(future),
            created_at: std::time::Instant::now(),
        };

        let priority_task = PriorityTask { task, priority };
        self.task_heap.push(priority_task);
        id
    }

    fn run_priority_tasks(&mut self) {
        while let Some(mut priority_task) = self.task_heap.pop() {
            let waker = waker_fn(move || {
                // 重新加入队列
                println!("Waking priority task");
            });

            let mut cx = Context::from_waker(&waker);

            match priority_task.task.future.as_mut().poll(&mut cx) {
                Poll::Ready(()) => {
                    println!("Priority task {:?} completed", priority_task.priority);
                }
                Poll::Pending => {
                    // 根据优先级决定是否重新加入队列
                    if priority_task.priority >= Priority::Normal {
                        self.task_heap.push(priority_task);
                    } else {
                        // 低优先级任务可能被延迟
                        thread::sleep(std::time::Duration::from_millis(10));
                        self.task_heap.push(priority_task);
                    }
                }
            }
        }
    }
}

impl Executor for PriorityExecutor {
    fn spawn<F>(&self, future: F) -> TaskId
    where
        F: Future<Output = ()> + Send + 'static,
    {
        self.spawn_with_priority(future, Priority::Normal)
    }

    fn block_on<F>(&self, future: F) -> F::Output
    where
        F: Future<Output = ()>,
    {
        // 实现block_on
        ()
    }

    fn stats(&self) -> ExecutorStats {
        ExecutorStats::default()
    }
}
```

## 性能优化技术

### 缓存友好的任务调度
```rust
use std::sync::atomic::{AtomicUsize, Ordering};

/// NUMA感知的任务调度器
pub struct NUMAAwareScheduler {
    numa_nodes: Vec<NUMANode>,
    current_node: AtomicUsize,
}

struct NUMANode {
    node_id: usize,
    local_workers: Vec<WorkerThread>,
    task_queue: Injector<Task>,
    cpu_mask: Vec<usize>,
}

impl NUMAAwareScheduler {
    pub fn new(numa_topology: &[Vec<usize>]) -> Self {
        let mut numa_nodes = Vec::new();

        for (node_id, cpus) in numa_topology.iter().enumerate() {
            let node = NUMANode {
                node_id,
                local_workers: Vec::new(),
                task_queue: Injector::new(),
                cpu_mask: cpus.clone(),
            };
            numa_nodes.push(node);
        }

        Self {
            numa_nodes,
            current_node: AtomicUsize::new(0),
        }
    }

    pub fn schedule_task<F>(&self, future: F, affinity_hint: Option<usize>) -> TaskId
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let target_node = affinity_hint
            .and_then(|cpu| self.find_numa_node_for_cpu(cpu))
            .unwrap_or_else(|| self.next_round_robin_node());

        let task_id = TaskId(self.task_counter.fetch_add(1, Ordering::Relaxed));

        // 创建任务并调度到指定NUMA节点
        let task = Task {
            id: task_id,
            future: Box::pin(future),
            created_at: std::time::Instant::now(),
        };

        self.numa_nodes[target_node].task_queue.push(task);
        task_id
    }

    fn find_numa_node_for_cpu(&self, cpu: usize) -> Option<usize> {
        self.numa_nodes.iter()
            .position(|node| node.cpu_mask.contains(&cpu))
    }

    fn next_round_robin_node(&self) -> usize {
        let current = self.current_node.fetch_add(1, Ordering::Relaxed);
        current % self.numa_nodes.len()
    }
}
```

### 自适应调度策略
```rust
use std::time::{Duration, Instant};

/// 自适应调度器
pub struct AdaptiveScheduler {
    workers: Vec<AdaptiveWorker>,
    load_balancer: LoadBalancer,
    performance_monitor: PerformanceMonitor,
}

struct AdaptiveWorker {
    id: usize,
    current_load: f64,
    throughput: f64,
    last_adjustment: Instant,
}

impl AdaptiveWorker {
    fn new(id: usize) -> Self {
        Self {
            id,
            current_load: 0.0,
            throughput: 0.0,
            last_adjustment: Instant::now(),
        }
    }

    fn adjust_workload(&mut self) {
        let now = Instant::now();
        let since_last = now.duration_since(self.last_adjustment);

        if since_last >= Duration::from_secs(1) {
            // 根据当前负载调整工作策略
            if self.current_load > 0.8 && self.throughput < 1000.0 {
                // 高负载低吞吐，可能需要更多CPU时间
                println!("Worker {} needs more CPU time", self.id);
            } else if self.current_load < 0.2 {
                // 低负载，可以接受更多任务
                println!("Worker {} can accept more tasks", self.id);
            }

            self.last_adjustment = now;
        }
    }
}

struct PerformanceMonitor {
    metrics: Vec<PerformanceMetric>,
}

struct PerformanceMetric {
    timestamp: Instant,
    worker_id: usize,
    task_count: u64,
    avg_latency: Duration,
    cpu_utilization: f64,
}

impl PerformanceMonitor {
    fn new() -> Self {
        Self {
            metrics: Vec::new(),
        }
    }

    fn record_metric(&mut self, metric: PerformanceMetric) {
        self.metrics.push(metric);

        // 保持最近1000个指标
        if self.metrics.len() > 1000 {
            self.metrics.remove(0);
        }
    }

    fn get_optimal_worker_count(&self) -> usize {
        if self.metrics.len() < 10 {
            return 4; // 默认值
        }

        // 分析最近性能指标
        let recent_metrics: Vec<_> = self.metrics.iter()
            .filter(|m| m.timestamp.elapsed() < Duration::from_secs(60))
            .collect();

        if recent_metrics.is_empty() {
            return 4;
        }

        let avg_latency: Duration = recent_metrics.iter()
            .map(|m| m.avg_latency)
            .sum::<Duration>() / recent_metrics.len() as u32;

        let avg_cpu: f64 = recent_metrics.iter()
            .map(|m| m.cpu_utilization)
            .sum::<f64>() / recent_metrics.len() as f64;

        // 根据延迟和CPU利用率调整worker数量
        if avg_latency > Duration::from_millis(100) && avg_cpu < 0.8 {
            // 延迟高，CPU利用率低，增加worker
            recent_metrics.len() + 1
        } else if avg_latency < Duration::from_millis(10) && avg_cpu > 0.9 {
            // 延迟低，CPU利用率高，可以减少worker
            (recent_metrics.len() / 2).max(1)
        } else {
            recent_metrics.len()
        }
    }
}
```

## 性能基准和测试

### 吞吐量测试
```rust
/// 吞吐量基准测试
pub fn throughput_benchmark<E: Executor>(executor: &E, task_count: usize) -> BenchmarkResult {
    let start_time = Instant::now();
    let barrier = Arc::new(std::sync::Barrier::new(task_count + 1));

    // 生成任务
    for i in 0..task_count {
        let barrier_clone = Arc::clone(&barrier);
        executor.spawn(async move {
            // 模拟工作负载
            let mut sum = 0u64;
            for j in 0..1000 {
                sum += j as u64;
            }

            barrier_clone.wait();
            println!("Task {} completed, sum: {}", i, sum);
        });
    }

    barrier.wait();
    let duration = start_time.elapsed();

    BenchmarkResult {
        task_count,
        duration,
        throughput: task_count as f64 / duration.as_secs_f64(),
    }
}

#[derive(Debug)]
struct BenchmarkResult {
    task_count: usize,
    duration: Duration,
    throughput: f64,
}
```

### 延迟测试
```rust
/// 延迟测试
pub fn latency_benchmark<E: Executor>(executor: &E, iterations: usize) -> Vec<Duration> {
    let mut latencies = Vec::with_capacity(iterations);

    for _ in 0..iterations {
        let start_time = Instant::now();
        let completion_time = Arc::new(Mutex::new(None));

        let completion_time_clone = Arc::clone(&completion_time);
        executor.spawn(async move {
            *completion_time_clone.lock().unwrap() = Some(Instant::now());
        });

        // 等待任务完成
        loop {
            if let Some(time) = *completion_time.lock().unwrap() {
                latencies.push(time - start_time);
                break;
            }
            thread::sleep(Duration::from_micros(1));
        }
    }

    latencies
}
```

## 相关专题

- `../async/future-trait.md` - Future Trait详解
- `../debugging/concurrent-debugging.md` - 并发调试技术