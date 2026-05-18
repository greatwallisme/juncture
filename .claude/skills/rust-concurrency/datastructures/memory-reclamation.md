# 内存回收策略

## 当使用此专题

- 实现无锁数据结构的安全内存回收
- 理解各种内存回收算法的优缺点
- 避免ABA问题和内存泄漏
- 优化内存回收的性能开销

## 内存回收基础

### Hazard Pointer机制
```rust
use std::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};
use std::ptr;
use std::collections::HashMap;
use std::cell::RefCell;

/// Hazard Pointer域
pub struct HazardDomain {
    retired_list: RefCell<Vec<*const u8>>,
    hazard_pointers: Vec<AtomicPtr<u8>>,
    max_retired: usize,
    reclaim_threshold: f64,
}

impl HazardDomain {
    /// 创建新的Hazard域
    pub fn new(num_threads: usize, max_retired: usize) -> Self {
        Self {
            retired_list: RefCell::new(Vec::new()),
            hazard_pointers: (0..num_threads)
                .map(|_| AtomicPtr::new(ptr::null_mut()))
                .collect(),
            max_retired,
            reclaim_threshold: 0.5, // 50%的指针可以回收时开始回收
        }
    }

    /// 获取线程的hazard pointer
    pub fn get_hazard_pointer(&self, thread_id: usize) -> *mut u8 {
        self.hazard_pointers[thread_id].load(Ordering::Acquire)
    }

    /// 设置hazard pointer
    pub fn set_hazard_pointer(&self, thread_id: usize, ptr: *mut u8) {
        self.hazard_pointers[thread_id].store(ptr, Ordering::Release);
    }

    /// 清除hazard pointer
    pub fn clear_hazard_pointer(&self, thread_id: usize) {
        self.hazard_pointers[thread_id].store(ptr::null_mut(), Ordering::Release);
    }

    /// 检查指针是否被hazard
    fn is_hazarded(&self, ptr: *const u8) -> bool {
        self.hazard_pointers
            .iter()
            .any(|hp| hp.load(Ordering::Acquire) == ptr as *mut u8)
    }

    /// 添加到待回收列表
    pub fn retire(&self, ptr: *const u8) {
        self.retired_list.borrow_mut().push(ptr);

        // 检查是否需要回收
        if self.should_reclaim() {
            self.reclaim();
        }
    }

    /// 判断是否应该回收
    fn should_reclaim(&self) -> bool {
        let retired_count = self.retired_list.borrow().len();
        let hazard_count = self.hazard_pointers
            .iter()
            .filter(|hp| !hp.load(Ordering::Acquire).is_null())
            .count();

        retired_count > self.max_retired ||
        (hazard_count > 0 && retired_count as f64 / hazard_count as f64 > self.reclaim_threshold)
    }

    /// 回收内存
    fn reclaim(&self) {
        let mut retired = self.retired_list.borrow_mut();
        let mut i = 0;

        while i < retired.len() {
            let ptr = retired[i];
            if !self.is_hazarded(ptr) {
                // 安全回收
                unsafe {
                    // 这里需要根据实际类型进行回收
                    // 示例中假设是简单的字节指针
                    let _ = Box::from_raw(ptr as *mut u8);
                }
                retired.remove(i);
            } else {
                i += 1;
            }
        }
    }
}

/// 使用Hazard Pointer的无锁栈节点
struct StackNode<T> {
    data: T,
    next: AtomicPtr<StackNode<T>>,
}

/// 使用Hazard Pointer的无锁栈
pub struct HazardStack<T> {
    head: AtomicPtr<StackNode<T>>,
    hazard_domain: HazardDomain,
}

impl<T> HazardStack<T> {
    pub fn new(num_threads: usize) -> Self {
        Self {
            head: AtomicPtr::new(ptr::null_mut()),
            hazard_domain: HazardDomain::new(num_threads, 100),
        }
    }

    /// 入栈
    pub fn push(&self, data: T, thread_id: usize) {
        let new_node = Box::into_raw(Box::new(StackNode {
            data,
            next: AtomicPtr::new(ptr::null_mut()),
        }));

        loop {
            let old_head = self.head.load(Ordering::Acquire);
            unsafe {
                (*new_node).next.store(old_head, Ordering::Relaxed);
            }

            match self.head.compare_exchange_weak(
                old_head,
                new_node,
                Ordering::Release,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(_) => continue,
            }
        }
    }

    /// 出栈
    pub fn pop(&self, thread_id: usize) -> Option<T> {
        loop {
            let head = self.head.load(Ordering::Acquire);
            if head.is_null() {
                return None;
            }

            // 设置hazard pointer
            self.hazard_domain.set_hazard_pointer(thread_id, head as *mut u8);

            // 重新验证head没有被修改
            if self.head.load(Ordering::Acquire) != head {
                self.hazard_domain.clear_hazard_pointer(thread_id);
                continue;
            }

            unsafe {
                let next = (*head).next.load(Ordering::Acquire);

                match self.head.compare_exchange_weak(
                    head,
                    next,
                    Ordering::Release,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => {
                        self.hazard_domain.clear_hazard_pointer(thread_id);
                        let data = ptr::read(&(*head).data);

                        // 延迟回收节点
                        self.hazard_domain.retire(head as *const u8);

                        return Some(data);
                    }
                    Err(_) => {
                        self.hazard_domain.clear_hazard_pointer(thread_id);
                        continue;
                    }
                }
            }
        }
    }
}

/// Hazard Pointer栈使用示例
fn hazard_stack_example() {
    let stack = Arc::new(HazardStack::new(4));
    let mut handles = vec![];

    // 生产者线程
    for i in 0..2 {
        let stack_clone = Arc::clone(&stack);
        let handle = std::thread::spawn(move || {
            for j in 0..1000 {
                stack_clone.push(i * 1000 + j, i);
            }
        });
        handles.push(handle);
    }

    // 消费者线程
    for i in 2..4 {
        let stack_clone = Arc::clone(&stack);
        let handle = std::thread::spawn(move || {
            let mut count = 0;
            while count < 1000 {
                if let Some(value) = stack_clone.pop(i) {
                    count += 1;
                }
            }
        });
        handles.push(handle);
    }

    for handle in handles {
        handle.join().unwrap();
    }
}
```

## Epoch-Based回收机制

### Epoch-Based内存回收
```rust
use std::sync::atomic::{AtomicU64, AtomicPtr, Ordering};
use std::sync::Arc;
use std::collections::VecDeque;
use std::ptr;

/// Epoch管理器
pub struct EpochManager {
    current_epoch: AtomicU64,
    retire_lists: [VecDeque<*const u8>; 3],
    active_counts: [AtomicU64; 3],
}

impl EpochManager {
    pub fn new() -> Self {
        Self {
            current_epoch: AtomicU64::new(0),
            retire_lists: [
                VecDeque::new(),
                VecDeque::new(),
                VecDeque::new(),
            ],
            active_counts: [
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
            ],
        }
    }

    /// 进入临界区
    pub fn enter_critical(&self) -> EpochGuard {
        let epoch = self.current_epoch.load(Ordering::Acquire);
        let epoch_index = (epoch as usize) % 3;

        self.active_counts[epoch_index].fetch_add(1, Ordering::Relaxed);

        EpochGuard {
            manager: self,
            epoch,
            epoch_index,
        }
    }

    /// 进入下一个epoch
    pub fn advance_epoch(&self) {
        let new_epoch = self.current_epoch.fetch_add(1, Ordering::Relaxed) + 1;
        let old_epoch_index = (new_epoch as usize - 2) % 3;

        // 检查是否可以回收旧epoch
        if self.active_counts[old_epoch_index].load(Ordering::Relaxed) == 0 {
            self.reclaim_epoch(old_epoch_index);
        }
    }

    /// 回收指定epoch的内存
    fn reclaim_epoch(&self, epoch_index: usize) {
        while let Some(ptr) = self.retire_lists[epoch_index].pop_front() {
            unsafe {
                // 安全回收内存
                let _ = Box::from_raw(ptr as *mut u8);
            }
        }
    }

    /// 退役内存
    pub fn retire(&self, ptr: *const u8, current_epoch: u64) {
        let retire_epoch_index = ((current_epoch + 2) % 3) as usize;
        self.retire_lists[retire_epoch_index].push_back(ptr);
    }

    /// 离开临界区
    fn leave_critical(&self, epoch_index: usize) {
        self.active_counts[epoch_index].fetch_sub(1, Ordering::Relaxed);
    }
}

/// Epoch Guard
pub struct EpochGuard<'a> {
    manager: &'a EpochManager,
    epoch: u64,
    epoch_index: usize,
}

impl<'a> Drop for EpochGuard<'a> {
    fn drop(&mut self) {
        self.manager.leave_critical(self.epoch_index);
    }
}

/// Epoch-Based无锁队列
pub struct EpochQueue<T> {
    head: AtomicPtr<QueueNode<T>>,
    tail: AtomicPtr<QueueNode<T>>,
    epoch_manager: Arc<EpochManager>,
}

#[derive(Debug)]
struct QueueNode<T> {
    data: Option<T>,
    next: AtomicPtr<QueueNode<T>>,
}

impl<T> EpochQueue<T> {
    pub fn new() -> Self {
        let dummy = Box::into_raw(Box::new(QueueNode {
            data: None,
            next: AtomicPtr::new(ptr::null_mut()),
        }));

        Self {
            head: AtomicPtr::new(dummy),
            tail: AtomicPtr::new(dummy),
            epoch_manager: Arc::new(EpochManager::new()),
        }
    }

    /// 入队
    pub fn enqueue(&self, data: T) {
        let new_node = Box::into_raw(Box::new(QueueNode {
            data: Some(data),
            next: AtomicPtr::new(ptr::null_mut()),
        }));

        loop {
            let _guard = self.epoch_manager.enter_critical();

            let tail = self.tail.load(Ordering::Acquire);
            unsafe {
                if (*tail).next.load(Ordering::Relaxed).is_null() {
                    match (*tail).next.compare_exchange_weak(
                        ptr::null_mut(),
                        new_node,
                        Ordering::Release,
                        Ordering::Relaxed,
                    ) {
                        Ok(_) => {
                            self.tail.compare_exchange(
                                tail,
                                new_node,
                                Ordering::Release,
                                Ordering::Relaxed,
                            ).ok();
                            break;
                        }
                        Err(_) => continue,
                    }
                } else {
                    // 帮助推进tail指针
                    let next = (*tail).next.load(Ordering::Relaxed);
                    self.tail.compare_exchange(
                        tail,
                        next,
                        Ordering::Release,
                        Ordering::Relaxed,
                    ).ok();
                }
            }
        }
    }

    /// 出队
    pub fn dequeue(&self) -> Option<T> {
        loop {
            let _guard = self.epoch_manager.enter_critical();

            let head = self.head.load(Ordering::Acquire);
            let tail = self.tail.load(Ordering::Acquire);

            unsafe {
                let next = (*head).next.load(Ordering::Acquire);

                if head == tail {
                    if next.is_null() {
                        return None;
                    }

                    // 帮助推进tail指针
                    self.tail.compare_exchange(
                        tail,
                        next,
                        Ordering::Release,
                        Ordering::Relaxed,
                    ).ok();
                    continue;
                }

                let data = (*next).data.take();

                if self.head.compare_exchange_weak(
                    head,
                    next,
                    Ordering::Release,
                    Ordering::Relaxed,
                ).is_ok() {
                    // 延迟回收head节点
                    let current_epoch = self.epoch_manager.current_epoch.load(Ordering::Relaxed);
                    self.epoch_manager.retire(head as *const u8, current_epoch);

                    return data;
                }
            }
        }
    }

    /// 定期推进epoch
    pub fn advance_epoch(&self) {
        self.epoch_manager.advance_epoch();
    }
}

/// Epoch-Based队列使用示例
fn epoch_queue_example() {
    let queue = Arc::new(EpochQueue::new());
    let mut handles = vec![];

    // 启动epoch推进线程
    let queue_clone = Arc::clone(&queue);
    let advance_handle = std::thread::spawn(move || {
        for _ in 0..100 {
            std::thread::sleep(std::time::Duration::from_millis(10));
            queue_clone.advance_epoch();
        }
    });
    handles.push(advance_handle);

    // 生产者线程
    for i in 0..2 {
        let queue_clone = Arc::clone(&queue);
        let handle = std::thread::spawn(move || {
            for j in 0..1000 {
                queue_clone.enqueue(format!("{}_{}", i, j));
            }
        });
        handles.push(handle);
    }

    // 消费者线程
    for i in 2..4 {
        let queue_clone = Arc::clone(&queue);
        let handle = std::thread::spawn(move || {
            let mut count = 0;
            while count < 500 {
                if let Some(_item) = queue_clone.dequeue() {
                    count += 1;
                }
            }
        });
        handles.push(handle);
    }

    for handle in handles {
        handle.join().unwrap();
    }
}
```

## 内存回收性能优化

### 混合回收策略
```rust
use std::sync::{Arc, RwLock, Mutex};
use std::collections::HashMap;
use std::time::{Instant, Duration};

/// 内存回收策略
#[derive(Debug, Clone)]
pub enum ReclamationStrategy {
    Hazard { max_retired: usize },
    Epoch { advance_interval: Duration },
    RCU { grace_period: Duration },
    Hybrid {
        hazard_threshold: usize,
        epoch_interval: Duration,
    },
}

/// 混合内存回收管理器
pub struct HybridReclamationManager {
    strategy: ReclamationStrategy,
    retired_list: Arc<Mutex<Vec<*const u8>>>,
    hazard_pointers: Arc<RwLock<HashMap<usize, *mut u8>>>,
    epoch: Arc<RwLock<u64>>,
    last_advance: Arc<Mutex<Instant>>,
    reclaim_count: Arc<Mutex<usize>>,
}

impl HybridReclamationManager {
    pub fn new(strategy: ReclamationStrategy) -> Self {
        Self {
            strategy,
            retired_list: Arc::new(Mutex::new(Vec::new())),
            hazard_pointers: Arc::new(RwLock::new(HashMap::new())),
            epoch: Arc::new(RwLock::new(0)),
            last_advance: Arc::new(Mutex::new(Instant::now())),
            reclaim_count: Arc::new(Mutex::new(0)),
        }
    }

    /// 设置hazard pointer
    pub fn set_hazard(&self, thread_id: usize, ptr: *mut u8) {
        let mut hazards = self.hazard_pointers.write().unwrap();
        hazards.insert(thread_id, ptr);
    }

    /// 清除hazard pointer
    pub fn clear_hazard(&self, thread_id: usize) {
        let mut hazards = self.hazard_pointers.write().unwrap();
        hazards.remove(&thread_id);
    }

    /// 检查指针是否被hazard
    fn is_hazarded(&self, ptr: *const u8) -> bool {
        let hazards = self.hazard_pointers.read().unwrap();
        hazards.values().any(|&hp| hp as *const u8 == ptr)
    }

    /// 退役内存
    pub fn retire(&self, ptr: *const u8) {
        let mut retired = self.retired_list.lock().unwrap();
        retired.push(ptr);

        // 根据策略决定是否立即回收
        match &self.strategy {
            ReclamationStrategy::Hazard { max_retired } => {
                if retired.len() > *max_retired {
                    self.reclaim_hazard();
                }
            }
            ReclamationStrategy::Epoch { .. } => {
                // Epoch模式下定期回收
                self.try_epoch_advance();
            }
            ReclamationStrategy::RCU { .. } => {
                // RCU模式需要等待宽限期
                self.reclaim_rcu();
            }
            ReclamationStrategy::Hybrid { hazard_threshold, epoch_interval } => {
                let mut last_advance = self.last_advance.lock().unwrap();

                if retired.len() > *hazard_threshold ||
                   last_advance.elapsed() > *epoch_interval {
                    self.reclaim_hybrid();
                    *last_advance = Instant::now();
                }
            }
        }
    }

    /// Hazard Pointer回收
    fn reclaim_hazard(&self) {
        let mut retired = self.retired_list.lock().unwrap();
        let mut i = 0;

        while i < retired.len() {
            let ptr = retired[i];
            if !self.is_hazarded(ptr) {
                unsafe {
                    let _ = Box::from_raw(ptr as *mut u8);
                }
                retired.remove(i);

                let mut count = self.reclaim_count.lock().unwrap();
                *count += 1;
            } else {
                i += 1;
            }
        }
    }

    /// 尝试推进epoch
    fn try_epoch_advance(&self) {
        let mut last_advance = self.last_advance.lock().unwrap();

        if let ReclamationStrategy::Epoch { advance_interval } = &self.strategy {
            if last_advance.elapsed() > *advance_interval {
                self.advance_epoch();
                *last_advance = Instant::now();
            }
        }
    }

    /// 推进epoch
    fn advance_epoch(&self) {
        let mut epoch = self.epoch.write().unwrap();
        *epoch += 1;

        // 回收两个epoch前的内存
        self.reclaim_epoch(*epoch.saturating_sub(2));
    }

    /// 回收指定epoch的内存
    fn reclaim_epoch(&self, target_epoch: u64) {
        // 简化实现，实际需要更复杂的epoch管理
        self.reclaim_hazard();
    }

    /// RCU回收
    fn reclaim_rcu(&self) {
        if let ReclamationStrategy::RCU { grace_period } = &self.strategy {
            let mut last_advance = self.last_advance.lock().unwrap();
            if last_advance.elapsed() > *grace_period {
                self.reclaim_hazard();
                *last_advance = Instant::now();
            }
        }
    }

    /// 混合回收
    fn reclaim_hybrid(&self) {
        // 先尝试Hazard回收
        self.reclaim_hazard();

        // 然后推进epoch
        if let ReclamationStrategy::Hybrid { .. } = &self.strategy {
            self.advance_epoch();
        }
    }

    /// 获取回收统计
    pub fn get_reclaim_stats(&self) -> (usize, usize) {
        let retired_count = self.retired_list.lock().unwrap().len();
        let reclaim_count = *self.reclaim_count.lock().unwrap();

        (retired_count, reclaim_count)
    }
}

/// 混合回收策略示例
fn hybrid_reclamation_example() {
    use std::thread;
    use std::time::Duration;

    // 创建混合策略
    let strategy = ReclamationStrategy::Hybrid {
        hazard_threshold: 50,
        epoch_interval: Duration::from_millis(100),
    };

    let manager = Arc::new(HybridReclamationManager::new(strategy));
    let mut handles = vec![];

    // 多线程进行内存分配和回收
    for i in 0..8 {
        let manager_clone = Arc::clone(&manager);
        let handle = thread::spawn(move || {
            for j in 0..1000 {
                // 模拟内存分配
                let data = Box::new(i * 1000 + j);
                let ptr = Box::into_raw(data) as *const u8;

                // 设置hazard pointer
                manager_clone.set_hazard(i, ptr as *mut u8);

                // 模拟使用内存
                thread::sleep(Duration::from_nanos(100));

                // 清除hazard pointer并退役内存
                manager_clone.clear_hazard(i);
                manager_clone.retire(ptr);
            }
        });
        handles.push(handle);
    }

    // 统计线程
    let manager_clone = Arc::clone(&manager);
    let stats_handle = thread::spawn(move || {
        for _ in 0..100 {
            thread::sleep(Duration::from_millis(50));
            let (retired, reclaimed) = manager_clone.get_reclaim_stats();
            println!("Retired: {}, Reclaimed: {}", retired, reclaimed);
        }
    });
    handles.push(stats_handle);

    for handle in handles {
        handle.join().unwrap();
    }

    let final_stats = manager.get_reclaim_stats();
    println!("Final - Retired: {}, Reclaimed: {}", final_stats.0, final_stats.1);
}
```

## 相关专题
- `../atomic/aba-problem.md` - ABA问题详解
- `../datastructures/lockfree-queue.md` - 无锁队列实现
- `../performance/cache-optimization.md` - 内存缓存优化
