# NUMA架构编程

## 当使用此专题

- 针对NUMA架构优化并发程序
- 理解内存局部性和CPU缓存层次
- 实现NUMA感知的数据结构
- 优化跨NUMA节点的通信性能

## NUMA架构基础

### NUMA拓扑检测
```rust
use std::fs;
use std::collections::HashMap;
use std::path::Path;

/// NUMA节点信息
#[derive(Debug, Clone)]
pub struct NumaNode {
    pub id: usize,
    pub cpus: Vec<usize>,
    pub memory_size: usize,
    pub distance: HashMap<usize, usize>,
    pub active: bool,
}

/// NUMA拓扑
#[derive(Debug)]
pub struct NumaTopology {
    pub nodes: Vec<NumaNode>,
    pub num_nodes: usize,
    pub total_memory: usize,
    pub is_numa_enabled: bool,
}

impl NumaTopology {
    /// 检测系统NUMA拓扑
    pub fn detect() -> Result<Self, String> {
        // 检查是否启用NUMA
        if !Path::new("/sys/devices/system/node").exists() {
            return Ok(NumaTopology {
                nodes: vec![NumaNode {
                    id: 0,
                    cpus: Self::detect_cpus()?,
                    memory_size: Self::detect_total_memory()?,
                    distance: HashMap::new(),
                    active: true,
                }],
                num_nodes: 1,
                total_memory: Self::detect_total_memory()?,
                is_numa_enabled: false,
            });
        }

        let mut nodes = Vec::new();
        let mut total_memory = 0;

        // 遍历所有NUMA节点
        for entry in fs::read_dir("/sys/devices/system/node")
            .map_err(|e| e.to_string())?
        {
            let entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path();

            if path.is_dir() && path.file_name().unwrap().to_str().unwrap().starts_with("node") {
                let node_id: usize = path.file_name()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .strip_prefix("node")
                    .unwrap()
                    .parse()
                    .map_err(|_| "Invalid node id".to_string())?;

                let node = Self::read_numa_node(&path, node_id)?;
                total_memory += node.memory_size;
                nodes.push(node);
            }
        }

        // 读取节点间距离矩阵
        Self::read_distance_matrix(&mut nodes)?;

        Ok(NumaTopology {
            num_nodes: nodes.len(),
            total_memory,
            nodes,
            is_numa_enabled: true,
        })
    }

    /// 读取NUMA节点信息
    fn read_numa_node(path: &Path, node_id: usize) -> Result<NumaNode, String> {
        // 读取CPU列表
        let cpu_path = path.join("cpulist");
        let cpu_list = fs::read_to_string(&cpu_path)
            .map_err(|e| format!("Failed to read cpulist: {}", e))?;

        let cpus = Self::parse_cpu_list(&cpu_list)?;

        // 读取内存大小
        let mem_path = path.join("meminfo");
        let mem_info = fs::read_to_string(&mem_path)
            .map_err(|e| format!("Failed to read meminfo: {}", e))?;

        let memory_size = Self::parse_memory_size(&mem_info)?;

        Ok(NumaNode {
            id: node_id,
            cpus,
            memory_size,
            distance: HashMap::new(),
            active: true,
        })
    }

    /// 解析CPU列表
    fn parse_cpu_list(cpu_list: &str) -> Result<Vec<usize>, String> {
        let mut cpus = Vec::new();

        for part in cpu_list.trim().split(',') {
            if part.contains('-') {
                let range: Vec<&str> = part.split('-').collect();
                if range.len() != 2 {
                    return Err("Invalid CPU range".to_string());
                }

                let start: usize = range[0].parse()
                    .map_err(|_| "Invalid start CPU".to_string())?;
                let end: usize = range[1].parse()
                    .map_err(|_| "Invalid end CPU".to_string())?;

                cpus.extend(start..=end);
            } else {
                cpus.push(part.parse()
                    .map_err(|_| "Invalid CPU ID".to_string())?);
            }
        }

        Ok(cpus)
    }

    /// 解析内存大小
    fn parse_memory_size(mem_info: &str) -> Result<usize, String> {
        for line in mem_info.lines() {
            if line.starts_with("Node ") && line.contains("MemTotal:") {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 4 {
                    let size_kb: usize = parts[3].parse()
                        .map_err(|_| "Invalid memory size".to_string())?;
                    return Ok(size_kb * 1024); // 转换为字节
                }
            }
        }
        Err("Memory size not found".to_string())
    }

    /// 读取节点间距离矩阵
    fn read_distance_matrix(nodes: &mut [NumaNode]) -> Result<(), String> {
        for node in nodes.iter_mut() {
            let distance_path = format!("/sys/devices/system/node/node{}/distance", node.id);

            if Path::new(&distance_path).exists() {
                let distance_str = fs::read_to_string(&distance_path)
                    .map_err(|e| format!("Failed to read distance: {}", e))?;

                let distances: Vec<usize> = distance_str
                    .trim()
                    .split_whitespace()
                    .map(|d| d.parse().map_err(|_| "Invalid distance".to_string()))
                    .collect::<Result<Vec<_>, _>>()?;

                for (i, &distance) in distances.iter().enumerate() {
                    node.distance.insert(i, distance);
                }
            }
        }

        Ok(())
    }

    /// 检测系统CPU列表
    fn detect_cpus() -> Result<Vec<usize>, String> {
        let cpu_str = fs::read_to_string("/sys/devices/system/cpu/online")
            .map_err(|_| "Failed to read CPU online list".to_string())?;
        Self::parse_cpu_list(&cpu_str)
    }

    /// 检测总内存大小
    fn detect_total_memory() -> Result<usize, String> {
        let mem_info = fs::read_to_string("/proc/meminfo")
            .map_err(|_| "Failed to read /proc/meminfo".to_string())?;

        for line in mem_info.lines() {
            if line.starts_with("MemTotal:") {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 2 {
                    let size_kb: usize = parts[1].parse()
                        .map_err(|_| "Invalid total memory".to_string())?;
                    return Ok(size_kb * 1024);
                }
            }
        }

        Err("Total memory not found".to_string())
    }

    /// 打印NUMA拓扑信息
    pub fn print_info(&self) {
        println!("NUMA Topology Information:");
        println!("  NUMA enabled: {}", self.is_numa_enabled);
        println!("  Total nodes: {}", self.num_nodes);
        println!("  Total memory: {} MB", self.total_memory / (1024 * 1024));

        for node in &self.nodes {
            println!("Node {}:", node.id);
            println!("  CPUs: {:?}", node.cpus);
            println!("  Memory: {} MB", node.memory_size / (1024 * 1024));
            println!("  Active: {}", node.active);

            if !node.distance.is_empty() {
                println!("  Distances: {:?}", node.distance);
            }
        }
    }

    /// 获取最优节点
    pub fn get_optimal_node(&self, thread_id: usize) -> usize {
        if !self.is_numa_enabled {
            return 0;
        }

        // 简单的轮询策略
        thread_id % self.num_nodes
    }
}

/// NUMA拓扑检测示例
fn numa_detection_example() {
    match NumaTopology::detect() {
        Ok(topology) => {
            topology.print_info();

            let optimal_node = topology.get_optimal_node(5);
            println!("Optimal node for thread 5: {}", optimal_node);
        }
        Err(e) => {
            println!("Failed to detect NUMA topology: {}", e);
        }
    }
}
```

## NUMA感知编程

### NUMA感知的线程池
```rust
use std::sync::{Arc, Barrier, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// NUMA感知的线程池
pub struct NumaThreadPool {
    workers: Vec<NumaWorker>,
    task_queue: Arc<Mutex<Vec<Box<dyn FnOnce() + Send>>>>,
    barrier: Arc<Barrier>,
    topology: NumaTopology,
}

struct NumaWorker {
    id: usize,
    numa_node: usize,
    cpus: Vec<usize>,
    handle: Option<thread::JoinHandle<()>>,
}

impl NumaThreadPool {
    /// 创建NUMA感知线程池
    pub fn new(workers_per_node: usize) -> Result<Self, String> {
        let topology = NumaTopology::detect()?;
        let mut workers = Vec::new();

        for (node_id, node) in topology.nodes.iter().enumerate() {
            if node.cpus.len() < workers_per_node {
                return Err(format!(
                    "Node {} has {} CPUs, but {} workers requested",
                    node_id, node.cpus.len(), workers_per_node
                ));
            }

            for i in 0..workers_per_node {
                let worker_cpus: Vec<usize> = node.cpus
                    .chunks(node.cpus.len() / workers_per_node)
                    .nth(i)
                    .unwrap_or(&[])
                    .to_vec();

                let worker = NumaWorker {
                    id: workers.len(),
                    numa_node: node_id,
                    cpus: worker_cpus,
                    handle: None,
                };
                workers.push(worker);
            }
        }

        Ok(NumaThreadPool {
            workers,
            task_queue: Arc::new(Mutex::new(Vec::new())),
            barrier: Arc::new(Barrier::new(workers.len() + 1)),
            topology,
        })
    }

    /// 启动线程池
    pub fn start(&mut self) {
        let task_queue = Arc::clone(&self.task_queue);
        let barrier = Arc::clone(&self.barrier);

        for worker in &mut self.workers {
            let task_queue_clone = Arc::clone(&task_queue);
            let barrier_clone = Arc::clone(&barrier);
            let numa_node = worker.numa_node;
            let cpus = worker.cpus.clone();

            let handle = thread::spawn(move || {
                // 设置NUMA亲和性
                Self::set_numa_affinity(numa_node, &cpus).unwrap();

                // 等待所有线程就绪
                barrier_clone.wait();

                // 工作循环
                loop {
                    let task = {
                        let mut queue = task_queue_clone.lock().unwrap();
                        queue.pop()
                    };

                    match task {
                        Some(task) => task(),
                        None => {
                            // 没有任务时短暂休眠
                            thread::sleep(Duration::from_millis(1));
                        }
                    }
                }
            });

            worker.handle = Some(handle);
        }

        // 等待所有工作线程就绪
        self.barrier.wait();
    }

    /// 设置NUMA亲和性
    fn set_numa_affinity(numa_node: usize, cpus: &[usize]) -> Result<(), String> {
        #[cfg(target_os = "linux")]
        {
            use libc::{cpu_set_t, sched_setaffinity, sched_getaffinity, pid_t};

            unsafe {
                let mut cpu_set: cpu_set_t = std::mem::zeroed();

                // 设置CPU亲和性
                for &cpu in cpus {
                    let cpu_id = cpu as libc::c_uint;
                    *(cpu_set.__bits.as_mut_ptr().add((cpu_id / 64) as usize)) |=
                        1u64 << (cpu_id % 64);
                }

                let result = sched_setaffinity(
                    0 as pid_t,
                    std::mem::size_of::<cpu_set_t>(),
                    &cpu_set as *const _ as *const _,
                );

                if result != 0 {
                    return Err(format!("Failed to set CPU affinity: {}", std::io::Error::last_os_error()));
                }

                // 设置NUMA内存策略
                if let Err(e) = Self::set_numa_memory_policy(numa_node) {
                    return Err(format!("Failed to set NUMA memory policy: {}", e));
                }
            }
        }

        Ok(())
    }

    /// 设置NUMA内存策略
    #[cfg(target_os = "linux")]
    fn set_numa_memory_policy(numa_node: usize) -> Result<(), String> {
        use libc::{set_mempolicy, MPOL_BIND};

        unsafe {
            let mut node_mask = 0u64;
            if numa_node < 64 {
                node_mask |= 1u64 << numa_node;
            }

            let result = set_mempolicy(MPOL_BIND as i32, &node_mask as *const u64, 64);
            if result != 0 {
                return Err(format!("Failed to set NUMA memory policy: {}", std::io::Error::last_os_error()));
            }
        }

        Ok(())
    }

    /// 执行任务
    pub fn execute<F>(&self, task: F)
    where
        F: FnOnce() + Send + 'static,
    {
        let mut queue = self.task_queue.lock().unwrap();
        queue.push(Box::new(task));
    }

    /// 并行执行任务
    pub fn parallel_execute<F, R>(&self, tasks: Vec<F>) -> Vec<R>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        use std::sync::mpsc;

        let (tx, rx) = mpsc::channel();
        let barrier = Arc::new(Barrier::new(tasks.len() + 1));

        for task in tasks {
            let tx_clone = tx.clone();
            let barrier_clone = Arc::clone(&barrier);

            self.execute(move || {
                barrier_clone.wait();
                let result = task();
                tx_clone.send(result).unwrap();
            });
        }

        drop(tx);
        barrier.wait();

        rx.into_iter().collect()
    }

    /// 获取线程统计信息
    pub fn get_stats(&self) -> NumaPoolStats {
        NumaPoolStats {
            total_workers: self.workers.len(),
            numa_nodes: self.topology.num_nodes,
            workers_per_node: self.workers.len() / self.topology.num_nodes,
            numa_enabled: self.topology.is_numa_enabled,
        }
    }
}

/// NUMA线程池统计信息
#[derive(Debug)]
pub struct NumaPoolStats {
    pub total_workers: usize,
    pub numa_nodes: usize,
    pub workers_per_node: usize,
    pub numa_enabled: bool,
}

/// NUMA线程池使用示例
fn numa_thread_pool_example() {
    let mut pool = match NumaThreadPool::new(2) {
        Ok(pool) => pool,
        Err(e) => {
            println!("Failed to create NUMA thread pool: {}", e);
            return;
        }
    };

    pool.start();

    let stats = pool.get_stats();
    println!("NUMA Thread Pool Stats: {:?}", stats);

    // 并行执行计算密集型任务
    let start_time = Instant::now();

    let tasks: Vec<_> = (0..100).map(|i| {
        move || {
            let mut sum = 0u64;
            for j in 0..1_000_000 {
                sum = sum.wrapping_add((i * 1000 + j) as u64);
            }
            sum
        }
    }).collect();

    let results = pool.parallel_execute(tasks);

    let duration = start_time.elapsed();
    println!("Completed {} tasks in {:?}", results.len(), duration);

    // 验证结果
    for (i, &result) in results.iter().enumerate() {
        let expected: u64 = (0..1_000_000)
            .map(|j| (i * 1000 + j) as u64)
            .sum();
        assert_eq!(result, expected);
    }
}
```

## 内存局部性优化

### NUMA感知的数据结构
```rust
use std::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};
use std::ptr;
use std::alloc::{alloc, dealloc, Layout};

/// NUMA感知的内存分配器
pub struct NumaAllocator {
    numa_nodes: usize,
    per_node_allocations: Vec<AtomicPtr<u8>>,
    per_node_used: Vec<AtomicUsize>,
    per_node_capacity: Vec<usize>,
}

impl NumaAllocator {
    /// 创建NUMA感知分配器
    pub fn new(numa_nodes: usize, per_node_capacity: usize) -> Self {
        let mut per_node_allocations = Vec::new();
        let mut per_node_used = Vec::new();
        let mut per_node_capacity_vec = Vec::new();

        for _ in 0..numa_nodes {
            let layout = Layout::from_size_align(per_node_capacity, 64).unwrap();
            let ptr = unsafe { alloc(layout) };
            if ptr.is_null() {
                panic!("Failed to allocate NUMA memory");
            }

            per_node_allocations.push(AtomicPtr::new(ptr));
            per_node_used.push(AtomicUsize::new(0));
            per_node_capacity_vec.push(per_node_capacity);
        }

        NumaAllocator {
            numa_nodes,
            per_node_allocations,
            per_node_used,
            per_node_capacity: per_node_capacity_vec,
        }
    }

    /// 从指定NUMA节点分配内存
    pub fn allocate_from_node(&self, node_id: usize, size: usize, alignment: usize) -> *mut u8 {
        if node_id >= self.numa_nodes {
            return ptr::null_mut();
        }

        let current_used = self.per_node_used[node_id].load(Ordering::Relaxed);
        let node_capacity = self.per_node_capacity[node_id];

        if current_used + size > node_capacity {
            return ptr::null_mut(); // 节点内存不足
        }

        let base_ptr = self.per_node_allocations[node_id].load(Ordering::Relaxed);
        let aligned_offset = (current_used + alignment - 1) & !(alignment - 1);
        let final_offset = aligned_offset + size;

        if final_offset > node_capacity {
            return ptr::null_mut();
        }

        // 原子性地更新已使用大小
        if self.per_node_used[node_id].compare_exchange_weak(
            current_used,
            final_offset,
            Ordering::Relaxed,
            Ordering::Relaxed,
        ).is_ok() {
            unsafe {
                return base_ptr.add(aligned_offset);
            }
        }

        // CAS失败，重试
        self.allocate_from_node(node_id, size, alignment)
    }

    /// 分配内存（自动选择NUMA节点）
    pub fn allocate(&self, size: usize, alignment: usize) -> *mut u8 {
        // 简单策略：轮流从不同节点分配
        static NEXT_NODE: AtomicUsize = AtomicUsize::new(0);
        let node_id = NEXT_NODE.fetch_add(1, Ordering::Relaxed) % self.numa_nodes;
        self.allocate_from_node(node_id, size, alignment)
    }

    /// 获取节点内存使用统计
    pub fn get_node_usage(&self, node_id: usize) -> (usize, usize) {
        if node_id >= self.numa_nodes {
            return (0, 0);
        }

        let used = self.per_node_used[node_id].load(Ordering::Relaxed);
        let capacity = self.per_node_capacity[node_id];
        (used, capacity)
    }
}

/// NUMA感知的向量
pub struct NumaVec<T> {
    data: Vec<*mut T>,
    sizes: Vec<usize>,
    allocator: Arc<NumaAllocator>,
    element_size: usize,
}

impl<T> NumaVec<T> {
    /// 创建NUMA感知向量
    pub fn new(numa_nodes: usize, per_node_capacity: usize) -> Self {
        let allocator = Arc::new(NumaAllocator::new(numa_nodes, per_node_capacity * std::mem::size_of::<T>()));
        let element_size = std::mem::size_of::<T>();

        NumaVec {
            data: vec![ptr::null_mut(); numa_nodes],
            sizes: vec![0; numa_nodes],
            allocator,
            element_size,
        }
    }

    /// 推入元素到指定NUMA节点
    pub fn push_to_node(&mut self, item: T, node_id: usize) -> Result<(), String> {
        if node_id >= self.data.len() {
            return Err(format!("Invalid NUMA node: {}", node_id));
        }

        let ptr = self.allocator.allocate_from_node(
            node_id,
            self.element_size,
            std::mem::align_of::<T>(),
        );

        if ptr.is_null() {
            return Err("Allocation failed".to_string());
        }

        unsafe {
            ptr.copy_from_nonoverlapping(&item as *const T as *const u8, self.element_size);
        }

        self.data[node_id] = ptr;
        self.sizes[node_id] += 1;
        Ok(())
    }

    /// 获取节点元素数量
    pub fn len(&self, node_id: usize) -> usize {
        if node_id < self.sizes.len() {
            self.sizes[node_id]
        } else {
            0
        }
    }

    /// 获取总元素数量
    pub fn total_len(&self) -> usize {
        self.sizes.iter().sum()
    }

    /// 迭代所有元素
    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.data
            .iter()
            .filter(|&&ptr| !ptr.is_null())
            .map(|&ptr| unsafe { &*ptr })
    }
}

/// NUMA感知的并行排序
pub fn numa_parallel_sort<T: Ord + Send + 'static>(
    data: &mut [T],
    numa_nodes: usize,
) -> Result<(), String> {
    use std::sync::mpsc;

    if data.is_empty() {
        return Ok(());
    }

    let chunk_size = (data.len() + numa_nodes - 1) / numa_nodes;
    let (tx, rx) = mpsc::channel();
    let mut handles = Vec::new();

    // 分割数据并并行排序
    for (node_id, chunk) in data.chunks_mut(chunk_size).enumerate() {
        let tx_clone = tx.clone();
        let chunk_start = node_id * chunk_size;

        let handle = std::thread::spawn(move || {
            // 设置NUMA亲和性
            if let Err(e) = set_thread_numa_affinity(node_id, numa_nodes) {
                println!("Warning: Failed to set NUMA affinity: {}", e);
            }

            chunk.sort_unstable();

            let chunk_end = chunk_start + chunk.len();
            tx_clone.send((chunk_start, chunk_end)).unwrap();
        });

        handles.push(handle);
    }

    drop(tx);

    // 等待所有排序完成
    let mut ranges = Vec::new();
    while let Ok((start, end)) = rx.recv() {
        ranges.push((start, end));
    }

    for handle in handles {
        handle.join().unwrap();
    }

    // 归并已排序的块
    ranges.sort_by_key(|&(start, _)| start);

    let mut result = Vec::with_capacity(data.len());
    for (start, end) in ranges {
        result.extend_from_slice(&data[start..end]);
    }

    data.copy_from_slice(&result);

    Ok(())
}

/// 设置线程NUMA亲和性
fn set_thread_numa_affinity(node_id: usize, numa_nodes: usize) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        use libc::{cpu_set_t, sched_setaffinity, pid_t};

        // 简化的CPU选择策略
        let cpu_id = node_id * (num_cpus::get() / numa_nodes);

        unsafe {
            let mut cpu_set: cpu_set_t = std::mem::zeroed();
            *(cpu_set.__bits.as_mut_ptr().add((cpu_id / 64) as usize)) |=
                1u64 << (cpu_id % 64);

            let result = sched_setaffinity(
                0 as pid_t,
                std::mem::size_of::<cpu_set_t>(),
                &cpu_set as *const _ as *const _,
            );

            if result != 0 {
                return Err(format!("Failed to set CPU affinity: {}", std::io::Error::last_os_error()));
            }
        }
    }

    Ok(())
}

/// NUMA感知内存分配示例
fn numa_memory_allocation_example() {
    let numa_nodes = num_cpus::get() / 2; // 假设每个NUMA节点有2个CPU
    let allocator = NumaAllocator::new(numa_nodes, 1024 * 1024); // 每个节点1MB

    // 从不同节点分配内存
    for i in 0..numa_nodes {
        let size = 1024;
        let ptr = allocator.allocate_from_node(i, size, 64);

        if !ptr.is_null() {
            println!("Allocated {} bytes from NUMA node {}", size, i);

            let (used, capacity) = allocator.get_node_usage(i);
            println!("Node {} usage: {}/{} bytes", i, used, capacity);
        }
    }
}

/// NUMA感知排序示例
fn numa_sort_example() {
    use rand::Rng;

    let mut data: Vec<u32> = (0..100_000)
        .map(|_| rand::thread_rng().gen_range(0..1_000_000))
        .collect();

    let numa_nodes = num_cpus::get() / 4; // 假设4个CPU一个NUMA节点

    println!("Sorting {} elements with {} NUMA nodes", data.len(), numa_nodes);

    let start_time = std::time::Instant::now();

    if let Err(e) = numa_parallel_sort(&mut data, numa_nodes) {
        println!("NUMA sort failed: {}", e);
        return;
    }

    let duration = start_time.elapsed();
    println!("NUMA-aware sort completed in {:?}", duration);

    // 验证排序结果
    for i in 1..data.len() {
        assert!(data[i-1] <= data[i]);
    }
    println!("Sort verification passed!");
}
```

## 相关专题

- `../performance/cache-optimization.md` - CPU缓存优化
- `../threading/work-stealing.md` - 工作窃取算法
- `../datastructures/lockfree-queue.md` - 无锁数据结构
- `../atomic/memory-ordering.md` - 内存序模型