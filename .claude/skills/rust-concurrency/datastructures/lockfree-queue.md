# 无锁队列实现

## 当使用此专题

- 实现高性能无锁数据结构
- 理解内存序和原子操作
- 构建无阻塞并发算法
- 解决ABA问题

## 无锁队列基础

### 单生产者单消费者队列
```rust
use std::sync::atomic::{AtomicUsize, Ordering};

/// 单生产者单消费者无锁队列
pub struct SPSCQueue<T> {
    buffer: Vec<Option<T>>,
    head: AtomicUsize,
    tail: AtomicUsize,
    mask: usize,
}

impl<T> SPSCQueue<T> {
    /// 创建新的队列，容量必须是2的幂
    pub fn new(capacity: usize) -> Self {
        assert!(capacity.is_power_of_two(), "Capacity must be power of 2");

        Self {
            buffer: (0..capacity).map(|_| None).collect(),
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
            mask: capacity - 1,
        }
    }

    /// 推入元素（仅生产者调用）
    pub fn push(&self, item: T) -> bool {
        let current_tail = self.tail.load(Ordering::Relaxed);
        let next_tail = (current_tail + 1) & self.mask;

        // 检查队列是否已满
        if next_tail == self.head.load(Ordering::Acquire) {
            return false; // 队列已满
        }

        // 存储元素
        unsafe {
            let ptr = self.buffer.as_ptr().add(current_tail & self.mask);
            (*ptr) = Some(item);
        }

        // 更新tail指针
        self.tail.store(next_tail, Ordering::Release);
        true
    }

    /// 弹出元素（仅消费者调用）
    pub fn pop(&self) -> Option<T> {
        let current_head = self.head.load(Ordering::Relaxed);

        // 检查队列是否为空
        if current_head == self.tail.load(Ordering::Acquire) {
            return None; // 队列为空
        }

        // 获取元素
        let item = unsafe {
            let ptr = self.buffer.as_ptr().add(current_head & self.mask);
            (*ptr).take()
        };

        // 更新head指针
        self.head.store((current_head + 1) & self.mask, Ordering::Release);
        item
    }

    /// 获取队列长度
    pub fn len(&self) -> usize {
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Relaxed);

        if tail >= head {
            tail - head
        } else {
            (self.mask + 1) - (head - tail)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn test_spsc_basic() {
        let queue = SPSCQueue::new(4);

        assert_eq!(queue.len(), 0);
        assert!(queue.push(1));
        assert!(queue.push(2));
        assert!(queue.push(3));
        assert!(queue.push(4));
        assert!(!queue.push(5)); // 队列已满

        assert_eq!(queue.pop(), Some(1));
        assert_eq!(queue.pop(), Some(2));
        assert_eq!(queue.len(), 2);

        assert!(queue.push(5));
        assert_eq!(queue.pop(), Some(3));
        assert_eq!(queue.pop(), Some(4));
        assert_eq!(queue.pop(), Some(5));
        assert_eq!(queue.pop(), None); // 队列为空
    }

    #[test]
    fn test_spsc_concurrent() {
        let queue = std::sync::Arc::new(SPSCQueue::new(1024));
        let queue_clone = std::sync::Arc::clone(&queue);

        let producer = thread::spawn(move || {
            for i in 0..10000 {
                while !queue_clone.push(i) {
                    thread::yield_now();
                }
            }
        });

        let consumer = thread::spawn(move || {
            let mut sum = 0u64;
            let mut count = 0;

            while count < 10000 {
                if let Some(item) = queue.pop() {
                    sum += item as u64;
                    count += 1;
                } else {
                    thread::yield_now();
                }
            }

            (sum, count)
        });

        producer.join().unwrap();
        let (sum, count) = consumer.join().unwrap();

        assert_eq!(count, 10000);
        assert_eq!(sum, (0..10000).sum::<i32>() as u64);
    }
}
```

## 多生产者多消费者队列

### 基于数组的MPMC队列
```rust
use std::sync::atomic::{AtomicUsize, AtomicPtr, Ordering};

/// 多生产者多消费者无锁队列
pub struct MPMCQueue<T> {
    buffer: Vec<AtomicPtr<T>>,
    mask: usize,
    head: AtomicUsize,
    tail: AtomicUsize,
}

impl<T> MPMCQueue<T> {
    pub fn new(capacity: usize) -> Self {
        assert!(capacity.is_power_of_two(), "Capacity must be power of 2");

        let buffer: Vec<AtomicPtr<T>> = (0..capacity)
            .map(|_| AtomicPtr::new(std::ptr::null_mut()))
            .collect();

        Self {
            buffer,
            mask: capacity - 1,
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
        }
    }

    pub fn push(&self, item: T) -> Result<(), T> {
        let item = Box::new(item);
        let item_ptr = Box::into_raw(item);

        loop {
            let current_tail = self.tail.load(Ordering::Relaxed);
            let next_tail = (current_tail + 1) & self.mask;

            // 检查队列是否已满
            if next_tail == self.head.load(Ordering::Acquire) {
                // 队列已满，释放内存并返回错误
                let boxed_item = unsafe { Box::from_raw(item_ptr) };
                return Err(boxed_item);
            }

            // 尝试推进tail指针
            if self.tail.compare_exchange_weak(
                current_tail,
                next_tail,
                Ordering::Release,
                Ordering::Relaxed,
            ).is_ok() {
                // 成功获得槽位，存储元素
                let slot = &self.buffer[current_tail & self.mask];
                slot.store(item_ptr, Ordering::Release);
                return Ok(());
            }
        }
    }

    pub fn pop(&self) -> Option<T> {
        loop {
            let current_head = self.head.load(Ordering::Relaxed);

            // 检查队列是否为空
            if current_head == self.tail.load(Ordering::Acquire) {
                return None;
            }

            // 尝试推进head指针
            let next_head = (current_head + 1) & self.mask;
            if self.head.compare_exchange_weak(
                current_head,
                next_head,
                Ordering::Release,
                Ordering::Relaxed,
            ).is_ok() {
                // 成功获得槽位，读取元素
                let slot = &self.buffer[current_head & self.mask];
                let item_ptr = slot.swap(std::ptr::null_mut(), Ordering::Acquire);

                if item_ptr.is_null() {
                    continue; // 另一个消费者正在处理这个槽位
                }

                // 转换指针为Box并获取所有权
                let boxed_item = unsafe { Box::from_raw(item_ptr) };
                return Some(*boxed_item);
            }
        }
    }
}

impl<T> Drop for MPMCQueue<T> {
    fn drop(&mut self) {
        // 清理剩余的元素
        for atomic_ptr in &self.buffer {
            let ptr = atomic_ptr.load(Ordering::Relaxed);
            if !ptr.is_null() {
                let _ = unsafe { Box::from_raw(ptr) };
            }
        }
    }
}
```

### Michael-Scott无锁队列
```rust
use std::sync::atomic::{AtomicPtr, Ordering};
use std::marker::PhantomData;

/// Michael-Scott无锁队列节点
struct Node<T> {
    value: T,
    next: AtomicPtr<Node<T>>,
}

/// Michael-Scott无锁队列
pub struct MSQueue<T> {
    head: AtomicPtr<Node<T>>,
    tail: AtomicPtr<Node<T>>,
    _marker: PhantomData<T>,
}

impl<T> MSQueue<T> {
    pub fn new() -> Self {
        // 创建哑节点
        let dummy = Box::into_raw(Box::new(Node {
            value: unsafe { std::mem::zeroed() },
            next: AtomicPtr::new(std::ptr::null_mut()),
        }));

        Self {
            head: AtomicPtr::new(dummy),
            tail: AtomicPtr::new(dummy),
            _marker: PhantomData,
        }
    }

    pub fn push(&self, value: T) {
        let new_node = Box::into_raw(Box::new(Node {
            value,
            next: AtomicPtr::new(std::ptr::null_mut()),
        }));

        loop {
            let tail = self.tail.load(Ordering::Acquire);
            let next = unsafe { (*tail).next.load(Ordering::Acquire) };

            if next.is_null() {
                // 尝试链接新节点
                if unsafe { (*tail).next.compare_exchange_weak(
                    std::ptr::null_mut(),
                    new_node,
                    Ordering::Release,
                    Ordering::Relaxed,
                ) }.is_ok() {
                    // 成功链接，推进tail
                    self.tail.compare_exchange(
                        tail,
                        new_node,
                        Ordering::Release,
                        Ordering::Relaxed,
                    );
                    break;
                }
            } else {
                // 帮助推进tail指针
                self.tail.compare_exchange(
                    tail,
                    next,
                    Ordering::Release,
                    Ordering::Relaxed,
                );
            }
        }
    }

    pub fn pop(&self) -> Option<T> {
        loop {
            let head = self.head.load(Ordering::Acquire);
            let tail = self.tail.load(Ordering::Acquire);
            let next = unsafe { (*head).next.load(Ordering::Acquire) };

            if head == tail {
                if next.is_null() {
                    return None; // 队列为空
                }
                // 帮助推进tail指针
                self.tail.compare_exchange(
                    tail,
                    next,
                    Ordering::Release,
                    Ordering::Relaxed,
                );
            } else {
                // 尝试推进head指针
                if self.head.compare_exchange_weak(
                    head,
                    next,
                    Ordering::Release,
                    Ordering::Relaxed,
                ).is_ok() {
                    // 成功移除节点，提取值
                    let old_head = unsafe { Box::from_raw(head) };
                    return Some(old_head.value);
                }
            }
        }
    }
}

impl<T> Drop for MSQueue<T> {
    fn drop(&mut self) {
        // 清理所有节点
        let mut current = self.head.load(Ordering::Relaxed);
        while !current.is_null() {
            let node = unsafe { Box::from_raw(current) };
            current = node.next.load(Ordering::Relaxed);
        }
    }
}
```

## 防ABA问题的解决方案

### 版本化指针
```rust
use std::sync::atomic::{AtomicU64, Ordering};

/// 版本化指针
#[derive(Debug, Clone, Copy)]
struct VersionedPtr<T> {
    ptr: *mut T,
    version: u64,
}

impl<T> VersionedPtr<T> {
    fn new(ptr: *mut T, version: u64) -> Self {
        Self { ptr, version }
    }

    fn combine(ptr: *mut T, version: u64) -> u64 {
        ((version as u64) << 48) | (ptr as u64)
    }

    fn from_combined(value: u64) -> Self {
        let ptr = (value & 0x0000FFFFFFFFFFFF) as *mut T;
        let version = value >> 48;
        Self { ptr, version }
    }
}

/// 使用版本化指针的无锁栈
pub struct LockFreeStack<T> {
    head: AtomicU64,
    _marker: PhantomData<T>,
}

impl<T> LockFreeStack<T> {
    pub fn new() -> Self {
        Self {
            head: AtomicU64::new(0), // null pointer, version 0
            _marker: PhantomData,
        }
    }

    pub fn push(&self, value: T) {
        let new_node = Box::into_raw(Box::new(value));

        loop {
            let current = self.head.load(Ordering::Acquire);
            let current_versioned = VersionedPtr::from_combined(current);

            // 设置新节点的next指针
            unsafe {
                *(new_node as *mut *mut T) = current_versioned.ptr;
            }

            let new_version = current_versioned.version + 1;
            let new_combined = VersionedPtr::combine(new_node, new_version);

            if self.head.compare_exchange_weak(
                current,
                new_combined,
                Ordering::Release,
                Ordering::Relaxed,
            ).is_ok() {
                break;
            }
        }
    }

    pub fn pop(&self) -> Option<T> {
        loop {
            let current = self.head.load(Ordering::Acquire);
            let current_versioned = VersionedPtr::from_combined(current);

            if current_versioned.ptr.is_null() {
                return None; // 栈为空
            }

            let next = unsafe { *(current_versioned.ptr as *mut *mut T) };
            let new_version = current_versioned.version + 1;
            let new_combined = VersionedPtr::combine(next, new_version);

            if self.head.compare_exchange_weak(
                current,
                new_combined,
                Ordering::Release,
                Ordering::Relaxed,
            ).is_ok() {
                let node = unsafe { Box::from_raw(current_versioned.ptr) };
                return Some(*node);
            }
        }
    }
}

impl<T> Drop for LockFreeStack<T> {
    fn drop(&mut self) {
        // 清理所有节点
        while let Some(_) = self.pop() {}
    }
}
```

### 内存回收策略
```rust
use std::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// 危险指针（Hazard Pointer）实现
struct HazardPointer {
    pointer: AtomicPtr<()>,
    thread_id: usize,
}

impl HazardPointer {
    fn new(thread_id: usize) -> Self {
        Self {
            pointer: AtomicPtr::new(std::ptr::null_mut()),
            thread_id,
        }
    }

    fn protect<T>(&self, ptr: *mut T) {
        self.pointer.store(ptr as *mut (), Ordering::Release);
    }

    fn clear(&self) {
        self.pointer.store(std::ptr::null_mut(), Ordering::Release);
    }

    fn get<T>(&self) -> *mut T {
        self.pointer.load(Ordering::Acquire) as *mut T
    }
}

/// 带危险指针保护的无锁队列
pub struct HazardProtectedQueue<T> {
    hazard_pointers: Vec<Arc<HazardPointer>>,
    retire_list: Arc<Mutex<Vec<*mut Node<T>>>>,
    // ... 其他队列字段
}

impl<T> HazardProtectedQueue<T> {
    pub fn new(num_threads: usize) -> Self {
        let hazard_pointers: Vec<_> = (0..num_threads)
            .map(|i| Arc::new(HazardPointer::new(i)))
            .collect();

        Self {
            hazard_pointers,
            retire_list: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn safe_to_reclaim<T>(&self, ptr: *mut T) -> bool {
        // 检查是否有线程正在保护这个指针
        for hp in &self.hazard_pointers {
            if hp.get::<T>() == ptr {
                return false;
            }
        }
        true
    }

    fn retire_node(&self, node: *mut Node<T>) {
        self.retire_list.lock().unwrap().push(node);

        // 尝试回收可以安全释放的节点
        let mut list = self.retire_list.lock().unwrap();
        list.retain(|&node| {
            if self.safe_to_reclaim(node) {
                let _ = unsafe { Box::from_raw(node) };
                false
            } else {
                true
            }
        });
    }
}
```

## 性能优化和测试

### 性能基准
```rust
use std::time::Instant;
use std::thread;

/// 队列性能测试
pub fn benchmark_queue<Q, T>(queue: &Q, producer_count: usize, consumer_count: usize, operations: usize) -> BenchmarkResult
where
    Q: Queue<T> + Send + Sync + 'static,
    T: Send + Sync + 'static + From<i32>,
{
    let start_time = Instant::now();
    let queue = std::sync::Arc::new(queue);
    let barrier = Arc::new(std::sync::Barrier::new(producer_count + consumer_count));

    // 生产者线程
    let mut producers = Vec::new();
    for producer_id in 0..producer_count {
        let queue = Arc::clone(&queue);
        let barrier = Arc::clone(&barrier);

        let producer = thread::spawn(move || {
            barrier.wait();

            let operations_per_producer = operations / producer_count;
            for i in 0..operations_per_producer {
                let value = T::from((producer_id * 1000 + i) as i32);
                while !queue.push(value) {
                    thread::yield_now();
                }
            }
        });
        producers.push(producer);
    }

    // 消费者线程
    let mut consumers = Vec::new();
    for _consumer_id in 0..consumer_count {
        let queue = Arc::clone(&queue);
        let barrier = Arc::clone(&barrier);

        let consumer = thread::spawn(move || {
            barrier.wait();

            let mut consumed = 0;
            while consumed < operations {
                if let Some(_value) = queue.pop() {
                    consumed += 1;
                } else {
                    thread::yield_now();
                }
            }
            consumed
        });
        consumers.push(consumer);
    }

    // 等待所有线程完成
    for producer in producers {
        producer.join().unwrap();
    }

    let mut total_consumed = 0;
    for consumer in consumers {
        total_consumed += consumer.join().unwrap();
    }

    let duration = start_time.elapsed();

    BenchmarkResult {
        producer_count,
        consumer_count,
        operations,
        total_consumed,
        duration,
        throughput: total_consumed as f64 / duration.as_secs_f64(),
    }
}

#[derive(Debug)]
pub struct BenchmarkResult {
    pub producer_count: usize,
    pub consumer_count: usize,
    pub operations: usize,
    pub total_consumed: usize,
    pub duration: std::time::Duration,
    pub throughput: f64,
}

/// 队列trait
pub trait Queue<T> {
    fn push(&self, value: T) -> bool;
    fn pop(&self) -> Option<T>;
    fn len(&self) -> usize;
}
```

### 压力测试
```rust
/// 压力测试：模拟高并发访问
pub fn stress_test<Q, T>(queue: &Q, num_threads: usize, operations_per_thread: usize)
where
    Q: Queue<T> + Send + Sync + 'static,
    T: Send + Sync + 'static + From<i32> + Copy,
{
    let queue = std::sync::Arc::new(queue);
    let start_time = Instant::now();
    let mut handles = Vec::new();

    // 启动多个线程，同时进行生产者和消费者操作
    for thread_id in 0..num_threads {
        let queue = Arc::clone(&queue);
        let handle = thread::spawn(move || {
            for i in 0..operations_per_thread {
                if i % 2 == 0 {
                    // 生产者操作
                    let value = T::from((thread_id * 1000 + i) as i32);
                    while !queue.push(value) {
                        thread::yield_now();
                    }
                } else {
                    // 消费者操作
                    let _ = queue.pop();
                }
            }
        });
        handles.push(handle);
    }

    // 等待所有线程完成
    for handle in handles {
        handle.join().unwrap();
    }

    let duration = start_time.elapsed();
    let total_operations = num_threads * operations_per_thread;

    println!("Stress test completed:");
    println!("  Threads: {}", num_threads);
    println!("  Operations per thread: {}", operations_per_thread);
    println!("  Total operations: {}", total_operations);
    println!("  Duration: {:?}", duration);
    println!("  Throughput: {:.2} ops/sec",
        total_operations as f64 / duration.as_secs_f64());
}
```

## 最佳实践

### 内存序选择
- **Relaxed**: 只需要原子性，不需要同步
- **Acquire**: 用于读取操作，确保后续的读取不被重排到前面
- **Release**: 用于写入操作，确保前面的写入不被重排到后面
- **AcqRel**: 同时需要Acquire和Release保证
- **SeqCst**: 需要全局一致性保证

### ABA问题防范
1. 使用版本化指针
2. 实现危险指针
3. 使用epoch-based回收
4. 避免重用指针值

### 性能考虑
1. 减少内存分配次数
2. 预分配缓冲区
3. 使用缓存友好的数据布局
4. 避免不必要的内存屏障

## 🔗 相关专题

- `../atomic/aba-problem.md` - ABA问题详解
- `../atomic/memory-ordering.md` - 内存序模型
- `../performance/cache-optimization.md` - 缓存优化
- `../debugging/concurrent-bugs.md` - 并发调试技术