# 工作窃取算法实现

## 当使用此专题

- 实现高性能CPU密集型任务调度
- 优化多核系统负载均衡
- 设计自适应任务分配系统
- 减少线程空闲时间

## 工作窃取算法详解

### 核心原理

工作窃取算法基于以下概念：
1. 每个线程有自己的本地任务队列（通常是双端队列/deque）
2. 线程优先从本地队列头部获取任务
3. 当本地队列为空时，从其他线程队列尾部"窃取"任务
4. 使用随机化避免活锁

### 基础实现
```rust
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

type Job = Box<dyn FnOnce() + Send>;

struct WorkStealingWorker {
    id: usize,
    local_queue: Arc<Mutex<VecDeque<Job>>>,
    all_queues: Vec<Arc<Mutex<VecDeque<Job>>>>,
    steal_attempts: u32,
    max_steal_attempts: u32,
}

impl WorkStealingWorker {
    fn new(
        id: usize,
        local_queue: Arc<Mutex<VecDeque<Job>>>,
        all_queues: Vec<Arc<Mutex<VecDeque<Job>>>>,
    ) -> Self {
        Self {
            id,
            local_queue,
            all_queues,
            steal_attempts: 0,
            max_steal_attempts: 1000,
        }
    }

    fn run(&mut self) {
        loop {
            // 1. 尝试从本地队列获取任务
            if let Some(job) = self.try_local_job() {
                job;
                self.steal_attempts = 0; // 重置窃取计数
                continue;
            }

            // 2. 本地队列为空，尝试窃取任务
            if let Some(job) = self.try_steal_job() {
                job;
                self.steal_attempts = 0;
                continue;
            }

            // 3. 没有任务可做，短暂休眠
            if self.steal_attempts > self.max_steal_attempts {
                thread::sleep(Duration::from_micros(100));
                self.steal_attempts = 0;
            } else {
                thread::yield_now();
                self.steal_attempts += 1;
            }
        }
    }

    fn try_local_job(&self) -> Option<Job> {
        let mut queue = self.local_queue.lock().unwrap();
        queue.pop_front() // 从头部获取
    }

    fn try_steal_job(&mut self) -> Option<Job> {
        // 随机选择其他队列进行窃取
        let num_workers = self.all_queues.len();
        let start = (self.id + 1) % num_workers;

        for i in 0..num_workers {
            let target_id = (start + i) % num_workers;
            if target_id == self.id {
                continue; // 不从自己队列窃取
            }

            if let Some(job) = self.steal_from_queue(target_id) {
                return Some(job);
            }
        }

        None
    }

    fn steal_from_queue(&self, target_id: usize) -> Option<Job> {
        let target_queue = &self.all_queues[target_id];
        let mut queue = target_queue.lock().unwrap();

        queue.pop_back() // 从尾部窃取
    }
}

pub struct WorkStealingPool {
    workers: Vec<WorkStealingWorker>,
    global_queue: Arc<Mutex<VecDeque<Job>>>,
    handles: Vec<thread::JoinHandle<()>>,
}

impl WorkStealingPool {
    pub fn new(worker_count: usize) -> Self {
        let mut queues = Vec::new();
        for _ in 0..worker_count {
            queues.push(Arc::new(Mutex::new(VecDeque::new())));
        }

        let mut workers = Vec::new();

        for id in 0..worker_count {
            let local_queue = Arc::clone(&queues[id]);
            let all_queues = queues.clone();

            workers.push(WorkStealingWorker::new(id, local_queue, all_queues));
        }

        WorkStealingPool {
            workers,
            global_queue: Arc::new(Mutex::new(VecDeque::new())),
            handles: Vec::new(),
        }
    }

    pub fn start(&mut self) {
        for (id, worker) in self.workers.drain(..).enumerate() {
            let global_queue = Arc::clone(&self.global_queue);

            let handle = thread::spawn(move || {
                let mut worker = worker;

                loop {
                    // 尝试本地任务
                    if let Some(job) = worker.try_local_job() {
                        job;
                        continue;
                    }

                    // 尝试从全局队列获取任务
                    if let Some(job) = Self::try_global_job(&global_queue) {
                        job;
                        continue;
                    }

                    // 尝试窃取任务
                    if let Some(job) = worker.try_steal_job() {
                        job;
                        continue;
                    }

                    // 没有任务，短暂休眠
                    thread::sleep(Duration::from_micros(10));
                }
            });

            self.handles.push(handle);
        }
    }

    fn try_global_job(global_queue: &Arc<Mutex<VecDeque<Job>>>) -> Option<Job> {
        let mut queue = global_queue.lock().unwrap();
        queue.pop_front()
    }

    pub fn submit(&self, job: Job) {
        // 简化版本：总是提交到全局队列
        // 实际实现中可以根据负载情况选择本地或全局队列
        self.global_queue.lock().unwrap().push_back(job);
    }
}
```

### 优化版本 - NUMA感知
```rust
use std::sync::atomic::{AtomicUsize, Ordering};

struct NUMAWorkStealingPool {
    workers_per_node: Vec<Vec<WorkStealingWorker>>,
    current_cpu: AtomicUsize,
    total_workers: usize,
}

impl NUMAWorkStealingPool {
    pub fn new(num_nodes: usize, workers_per_node: usize) -> Self {
        let mut workers_per_node_list = Vec::new();

        for node_id in 0..num_nodes {
            let mut node_workers = Vec::new();

            for worker_id in 0..workers_per_node {
                // 创建属于特定NUMA节点的worker
                let worker = create_numa_aware_worker(node_id, worker_id);
                node_workers.push(worker);
            }

            workers_per_node_list.push(node_workers);
        }

        Self {
            workers_per_node: workers_per_node_list,
            current_cpu: AtomicUsize::new(0),
            total_workers: num_nodes * workers_per_node,
        }
    }

    pub fn submit_numa_local(&self, job: Job) {
        // 提交到当前CPU所在的NUMA节点
        let current_cpu = self.current_cpu.load(Ordering::Relaxed);
        let node_id = get_numa_node_for_cpu(current_cpu);

        if let Some(node_workers) = self.workers_per_node.get(node_id) {
            // 选择负载最轻的worker
            let target_worker = self.find_least_loaded_worker(node_workers);
            target_worker.submit_local_job(job);
        } else {
            // 回退到全局提交
            self.submit_global(job);
        }
    }

    fn find_least_loaded_worker(&self, workers: &[WorkStealingWorker]) -> &WorkStealingWorker {
        // 简化实现：随机选择一个worker
        // 实际实现中应该监控队列长度
        let index = fastrand::usize(..) % workers.len();
        &workers[index]
    }
}

fn create_numa_aware_worker(node_id: usize, worker_id: usize) -> WorkStealingWorker {
    // 设置CPU亲和性到特定NUMA节点
    if cfg!(target_os = "linux") {
        let cpu_id = node_id * get_cpus_per_node() + worker_id;
        unsafe {
            libc::cpu_set_t mut cpuset;
            libc::CPU_ZERO(&mut cpuset);
            libc::CPU_SET(cpu_id, &mut cpuset);

            // 设置线程亲和性
            if let Err(_) = std::thread::spawn(move || {
                let result = libc::sched_setaffinity(0,
                    std::mem::size_of::<libc::cpu_set_t>(),
                    &cpuset);
                if result != 0 {
                    eprintln!("Failed to set CPU affinity: {}", std::io::Error::last_os_error());
                }
            }).join() {
                // 处理错误
            }
        }
    }

    // 创建worker...
    WorkStealingWorker::new(node_id, worker_id, vec![])
}
```

### 高级特性 - 自适应窃取
```rust
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

struct AdaptiveWorkStealingPool {
    workers: Vec<AdaptiveWorker>,
    steal_statistics: Vec<AtomicU64>,
    adaptive_enabled: AtomicBool,
    steal_threshold: AtomicU64,
}

struct AdaptiveWorker {
    base: WorkStealingWorker,
    last_steal_time: AtomicU64,
    steal_success_rate: AtomicU64,
    preferred_victims: Vec<usize>,
}

impl AdaptiveWorkStealingPool {
    pub fn new(worker_count: usize) -> Self {
        let mut workers = Vec::new();
        let mut statistics = Vec::new();

        for id in 0..worker_count {
            workers.push(AdaptiveWorker {
                base: WorkStealingWorker::new(id, vec![]),
                last_steal_time: AtomicU64::new(0),
                steal_success_rate: AtomicU64::new(0),
                preferred_victims: Vec::new(),
            });

            statistics.push(AtomicU64::new(0));
        }

        Self {
            workers,
            steal_statistics: statistics,
            adaptive_enabled: AtomicBool::new(true),
            steal_threshold: AtomicU64::new(100),
        }
    }

    pub fn update_steal_statistics(&self, worker_id: usize, success: bool) {
        if !self.adaptive_enabled.load(Ordering::Relaxed) {
            return;
        }

        let worker = &self.workers[worker_id];
        let now = current_time_nanos();

        if success {
            // 更新成功统计
            let current_rate = worker.steal_success_rate.load(Ordering::Relaxed);
            worker.steal_success_rate.store(
                (current_rate * 9 + 100) / 10, // 移动平均
                Ordering::Relaxed
            );

            // 更新首选受害者列表
            self.update_preferred_victims(worker_id);
        }

        worker.last_steal_time.store(now, Ordering::Relaxed);
    }

    fn update_preferred_victims(&self, worker_id: usize) {
        let worker = &self.workers[worker_id];
        let mut victims = worker.preferred_victims.clone();

        // 简化实现：随机选择一些受害者作为首选
        victims.clear();
        let num_workers = self.workers.len();
        let num_victims = (num_workers / 4).max(1); // 选择1/4的worker作为首选

        for _ in 0..num_victims {
            let victim = fastrand::usize(..(num_workers.saturating_sub(1)));
            if victim >= worker_id {
                let victim = victim + 1; // 避免选择自己
            }
            if victim < num_workers && !victims.contains(&victim) {
                victims.push(victim);
            }
        }
    }

    pub fn smart_steal_job(&mut self, worker_id: usize) -> Option<Job> {
        let worker = &mut self.workers[worker_id];

        // 优先从首选受害者窃取
        for &victim_id in &worker.preferred_victims {
            if let Some(job) = worker.steal_from_specific_queue(victim_id) {
                return Some(job);
            }
        }

        // 回退到随机窃取
        worker.try_steal_job()
    }
}
```

## 性能优化技巧

### 1. 缓存行对齐
```rust
use std::mem;

#[repr(align(64))] // 64字节对齐，避免伪共享
struct CacheAlignedDeque<T> {
    data: VecDeque<T>,
    _padding: [u8; 64 - std::mem::size_of::<VecDeque<T>>() % 64],
}
```

### 2. 批量窃取
```rust
impl WorkStealingWorker {
    fn batch_steal(&mut self, batch_size: usize) -> Vec<Job> {
        let mut stolen = Vec::with_capacity(batch_size);

        for _ in 0..batch_size {
            if let Some(job) = self.try_steal_job() {
                stolen.push(job);
            } else {
                break;
            }
        }

        stolen
    }
}
```

### 3. 工作窃取 vs 工作共享
```rust
enum WorkDistribution {
    Stealing,    // 工作窃取模式
    Sharing,     // 工作共享模式
    Hybrid {      // 混合模式
        steal_threshold: usize,
        share_threshold: usize,
    },
}

struct AdaptiveWorkPool {
    distribution: WorkDistribution,
    local_load: AtomicUsize,
    global_load: AtomicUsize,
}

impl AdaptiveWorkPool {
    fn should_share(&self) -> bool {
        match self.distribution {
            WorkDistribution::Sharing => true,
            WorkDistribution::Stealing => false,
            WorkDistribution::Hybrid { share_threshold, .. } => {
                self.local_load.load(Ordering::Relaxed) > share_threshold
            }
        }
    }
}
```

## 性能基准

基于书中测试数据：
- **任务调度延迟**: < 1ms
- **CPU利用率**: 95%+ (在8核系统)
- **负载均衡度**: 标准差 < 5%
- **窃取开销**: < 2% 的总执行时间

## 实现模板

### 基础工作窃取
- `../templates/thread-pools/work-stealing-basic.rs` - 基础实现

### NUMA优化版本
- `../templates/thread-pools/work-stealing-numa.rs` - NUMA感知实现

### 自适应版本
- `../templates/thread-pools/work-stealing-adaptive.rs` - 自适应窃取

## 常见陷阱

### 1. 死锁问题
```rust
// ❌ 可能死锁 - 所有线程同时窃取
fn naive_steal() {
    while let Some(job) = try_steal_from_any() {
        job;
    }
}

// ✅ 使用随机化和超时
fn smart_steal() {
    let attempts = 0;
    while attempts < MAX_STEAL_ATTEMPTS {
        if let Some(job) = try_steal_random() {
            job;
            break;
        }
        attempts += 1;
        thread::yield_now();
    }
}
```

### 2. 级联窃取
```rust
// ❌ 级联窃取可能导致雪崩
fn cascade_steal() {
    for target in 0..num_workers {
        if let Some(job) = steal_from(target) {
            return job;
        }
    }
}

// ✅ 限制窃取范围
fn limited_steal() {
    let max_victims = num_workers / 2;
    for _ in 0..max_victims {
        let target = random_victim();
        if let Some(job) = steal_from(target) {
            return job;
        }
    }
}
```
