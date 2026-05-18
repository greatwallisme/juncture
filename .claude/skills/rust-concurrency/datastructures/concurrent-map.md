# 并发HashMap实现

## 当使用此专题

- 实现高性能并发HashMap
- 理解分段锁和无锁HashMap技术
- 优化哈希表并发访问性能
- 解决哈希冲突和扩容问题

## 并发HashMap基础

### 分段锁HashMap
```rust
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::hash::{Hash, Hasher, BuildHasherDefault};
use std::collections::hash_map::DefaultHasher;

/// 分段锁并发HashMap
pub struct SegmentedHashMap<K, V, H = BuildHasherDefault<DefaultHasher>> {
    segments: Vec<RwLock<HashMap<K, V, H>>>,
    segment_count: usize,
    hasher: H,
    build_hasher: H,
}

impl<K, V> SegmentedHashMap<K, V, BuildHasherDefault<DefaultHasher>>
where
    K: Hash + Eq + Clone,
    V: Clone,
{
    /// 创建新的分段锁HashMap
    pub fn new(segment_count: usize) -> Self {
        Self {
            segments: (0..segment_count)
                .map(|_| RwLock::new(HashMap::new()))
                .collect(),
            segment_count,
            hasher: DefaultHasher::default(),
            build_hasher: DefaultHasher::default(),
        }
    }

    /// 计算键对应的段索引
    fn segment_index(&self, key: &K) -> usize {
        let mut hasher = self.build_hasher.build_hasher();
        key.hash(&mut hasher);
        (hasher.finish() as usize) % self.segment_count
    }

    /// 插入键值对
    pub fn insert(&self, key: K, value: V) -> Option<V> {
        let segment = &self.segments[self.segment_index(&key)];
        let mut map = segment.write().unwrap();
        map.insert(key, value)
    }

    /// 获取值
    pub fn get(&self, key: &K) -> Option<V> {
        let segment = &self.segments[self.segment_index(key)];
        let map = segment.read().unwrap();
        map.get(key).cloned()
    }

    /// 删除键值对
    pub fn remove(&self, key: &K) -> Option<V> {
        let segment = &self.segments[self.segment_index(key)];
        let mut map = segment.write().unwrap();
        map.remove(key)
    }

    /// 检查是否包含键
    pub fn contains_key(&self, key: &K) -> bool {
        let segment = &self.segments[self.segment_index(key)];
        let map = segment.read().unwrap();
        map.contains_key(key)
    }

    /// 获取所有段的大小总和
    pub fn len(&self) -> usize {
        self.segments
            .iter()
            .map(|segment| segment.read().unwrap().len())
            .sum()
    }

    /// 检查是否为空
    pub fn is_empty(&self) -> bool {
        self.segments
            .iter()
            .all(|segment| segment.read().unwrap().is_empty())
    }
}

/// 分段锁HashMap使用示例
fn segmented_hashmap_example() {
    let map = Arc::new(SegmentedHashMap::new(16));
    let mut handles = vec![];

    // 多线程并发写入
    for i in 0..100 {
        let map_clone = Arc::clone(&map);
        let handle = std::thread::spawn(move || {
            for j in 0..1000 {
                let key = format!("key_{}_{}", i, j);
                let value = j * 10;
                map_clone.insert(key, value);
            }
        });
        handles.push(handle);
    }

    // 等待所有写入完成
    for handle in handles {
        handle.join().unwrap();
    }

    println!("HashMap size: {}", map.len());

    // 测试读取
    let read_handles: Vec<_> = (0..10)
        .map(|_| {
            let map_clone = Arc::clone(&map);
            std::thread::spawn(move || {
                for i in 0..1000 {
                    let key = format!("key_0_{}", i);
                    if let Some(value) = map_clone.get(&key) {
                        assert_eq!(value, i * 10);
                    }
                }
            })
        })
        .collect();

    for handle in read_handles {
        handle.join().unwrap();
    }
}
```

## 高级并发HashMap

### 无锁HashMap设计
```rust
use std::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};
use std::ptr;
use std::hash::{Hash, Hasher, BuildHasher};
use std::collections::hash_map::DefaultHasher;
use std::marker::PhantomData;

/// 哈希表节点
struct Node<K, V> {
    key: K,
    value: V,
    hash: u64,
    next: *mut Node<K, V>,
}

/// 无锁HashMap实现
pub struct LockFreeHashMap<K, V, H = BuildHasherDefault<DefaultHasher>> {
    table: AtomicPtr<AtomicPtr<Node<K, V>>>,
    size: AtomicUsize,
    capacity: usize,
    hasher: H,
    build_hasher: H,
    _marker: PhantomData<(K, V)>,
}

impl<K, V> LockFreeHashMap<K, V, BuildHasherDefault<DefaultHasher>>
where
    K: Hash + Eq + Clone + Send + 'static,
    V: Clone + Send + 'static,
{
    /// 创建新的无锁HashMap
    pub fn new(capacity: usize) -> Self {
        let table = unsafe {
            let ptr = std::alloc::alloc(
                std::alloc::Layout::array::<AtomicPtr<Node<K, V>>>(capacity)
                    .unwrap(),
            ) as *mut AtomicPtr<Node<K, V>>;

            for i in 0..capacity {
                ptr.add(i).write(AtomicPtr::new(ptr::null_mut()));
            }
            ptr
        };

        Self {
            table: AtomicPtr::new(table),
            size: AtomicUsize::new(0),
            capacity,
            hasher: DefaultHasher::default(),
            build_hasher: DefaultHasher::default(),
            _marker: PhantomData,
        }
    }

    /// 计算哈希值
    fn hash_key(&self, key: &K) -> u64 {
        let mut hasher = self.build_hasher.build_hasher();
        key.hash(&mut hasher);
        hasher.finish()
    }

    /// 计算桶索引
    fn bucket_index(&self, hash: u64) -> usize {
        (hash as usize) % self.capacity
    }

    /// 在桶中查找节点
    fn find_node(&self, bucket: *mut Node<K, V>, key: &K, hash: u64) -> Option<*mut Node<K, V>> {
        let mut current = bucket;
        while !current.is_null() {
            unsafe {
                if (*current).hash == hash && (*current).key == *key {
                    return Some(current);
                }
                current = (*current).next;
            }
        }
        None
    }

    /// 插入键值对
    pub fn insert(&self, key: K, value: V) -> Option<V> {
        let hash = self.hash_key(&key);
        let bucket_index = self.bucket_index(hash);

        let table = self.table.load(Ordering::Acquire);
        let bucket_ptr = unsafe { table.add(bucket_index) };

        // 创建新节点
        let new_node = Box::into_raw(Box::new(Node {
            key: key.clone(),
            value: value.clone(),
            hash,
            next: ptr::null_mut(),
        }));

        loop {
            let bucket_head = (*bucket_ptr).load(Ordering::Acquire);

            // 检查键是否已存在
            if let Some(existing_node) = self.find_node(bucket_head, &key, hash) {
                unsafe {
                    let old_value = std::mem::replace(&mut (*existing_node).value, value);
                    // 清理新节点
                    let _ = Box::from_raw(new_node);
                    return Some(old_value);
                }
            }

            // 设置新节点的next指针
            unsafe {
                (*new_node).next = bucket_head;
            }

            // 原子性地插入新节点到桶头
            match (*bucket_ptr).compare_exchange_weak(
                bucket_head,
                new_node,
                Ordering::Release,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    self.size.fetch_add(1, Ordering::Relaxed);
                    return None;
                }
                Err(_) => {
                    // CAS失败，重试
                    continue;
                }
            }
        }
    }

    /// 获取值
    pub fn get(&self, key: &K) -> Option<V> {
        let hash = self.hash_key(key);
        let bucket_index = self.bucket_index(hash);

        let table = self.table.load(Ordering::Acquire);
        let bucket_ptr = unsafe { table.add(bucket_index) };
        let bucket_head = (*bucket_ptr).load(Ordering::Acquire);

        if let Some(node) = self.find_node(bucket_head, key, hash) {
            unsafe {
                return Some((*node).value.clone());
            }
        }
        None
    }

    /// 获取当前大小
    pub fn len(&self) -> usize {
        self.size.load(Ordering::Relaxed)
    }

    /// 检查是否为空
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// 无锁HashMap使用示例
fn lockfree_hashmap_example() {
    let map = Arc::new(LockFreeHashMap::new(1024));
    let mut handles = vec![];

    // 多线程并发操作
    for i in 0..8 {
        let map_clone = Arc::clone(&map);
        let handle = std::thread::spawn(move || {
            for j in 0..1000 {
                let key = format!("{}_{}", i, j);
                let value = i * 1000 + j;
                map_clone.insert(key, value);
            }
        });
        handles.push(handle);
    }

    for handle in handles {
        handle.join().unwrap();
    }

    println!("LockFreeHashMap size: {}", map.len());
}
```

## 哈希表扩容策略

### 渐进式扩容
```rust
use std::sync::{Arc, Mutex, Condvar};
use std::thread;
use std::time::Duration;

/// 渐进式扩容的HashMap
pub struct ResizableHashMap<K, V> {
    segments: Vec<RwLock<HashMap<K, V>>>,
    segment_count: usize,
    resize_lock: Mutex<bool>,
    resize_condvar: Condvar,
    pending_resizes: AtomicUsize,
    total_size: AtomicUsize,
}

impl<K, V> ResizableHashMap<K, V>
where
    K: Hash + Eq + Clone,
    V: Clone,
{
    /// 创建新的可扩容HashMap
    pub fn new(initial_segments: usize) -> Arc<Self> {
        Arc::new(Self {
            segments: (0..initial_segments)
                .map(|_| RwLock::new(HashMap::new()))
                .collect(),
            segment_count: initial_segments,
            resize_lock: Mutex::new(false),
            resize_condvar: Condvar::new(),
            pending_resizes: AtomicUsize::new(0),
            total_size: AtomicUsize::new(0),
        })
    }

    /// 获取当前段数
    fn current_segment_count(&self) -> usize {
        self.segments.len()
    }

    /// 检查是否需要扩容
    fn should_resize(&self) -> bool {
        let current_size = self.total_size.load(Ordering::Relaxed);
        let segment_count = self.current_segment_count();
        let avg_load = current_size as f64 / segment_count as f64;

        // 平均每个段的元素超过阈值时需要扩容
        avg_load > 50.0
    }

    /// 启动后台扩容线程
    pub fn start_resize_monitor(self: Arc<Self>) {
        let map_clone = Arc::clone(&self);
        thread::spawn(move || {
            loop {
                // 等待扩容信号
                {
                    let resize_lock = map_clone.resize_lock.lock().unwrap();
                    let _guard = map_clone.resize_condvar.wait(resize_lock).unwrap();
                }

                // 执行扩容
                if map_clone.should_resize() {
                    map_clone.perform_resize();
                }

                thread::sleep(Duration::from_millis(100));
            }
        });
    }

    /// 执行扩容操作
    fn perform_resize(&self) {
        let new_segment_count = self.current_segment_count() * 2;
        println!("开始扩容: {} -> {} 段", self.current_segment_count(), new_segment_count);

        // 创建新段
        let mut new_segments: Vec<RwLock<HashMap<K, V>>> = (0..new_segment_count)
            .map(|_| RwLock::new(HashMap::new()))
            .collect();

        // 迁移数据
        let mut total_migrated = 0;
        for (old_index, segment) in self.segments.iter().enumerate() {
            let map = segment.read().unwrap();
            for (key, value) in map.iter() {
                // 计算新索引
                let new_index = self.segment_index_for_count(key, new_segment_count);
                let mut new_segment = new_segments[new_index].write().unwrap();
                new_segment.insert(key.clone(), value.clone());
                total_migrated += 1;
            }
        }

        // 原子性地替换段数组
        // 注意：这里简化了实现，实际需要更复杂的同步机制
        println!("扩容完成，迁移了 {} 个元素", total_migrated);
    }

    /// 为指定段数计算索引
    fn segment_index_for_count(&self, key: &K, segment_count: usize) -> usize {
        let mut hasher = DefaultHasher::default();
        key.hash(&mut hasher);
        (hasher.finish() as usize) % segment_count
    }

    /// 触发扩容检查
    fn check_and_trigger_resize(&self) {
        if self.should_resize() {
            self.pending_resizes.fetch_add(1, Ordering::Relaxed);
            self.resize_condvar.notify_one();
        }
    }

    /// 插入键值对
    pub fn insert(&self, key: K, value: V) -> Option<V> {
        let current_segments = self.current_segment_count();
        let segment_index = self.segment_index_for_count(&key, current_segments);

        let result = {
            let mut segment = self.segments[segment_index].write().unwrap();
            segment.insert(key, value)
        };

        if result.is_none() {
            self.total_size.fetch_add(1, Ordering::Relaxed);
            self.check_and_trigger_resize();
        }

        result
    }

    /// 获取值
    pub fn get(&self, key: &K) -> Option<V> {
        let current_segments = self.current_segment_count();
        let segment_index = self.segment_index_for_count(key, current_segments);

        let segment = self.segments[segment_index].read().unwrap();
        segment.get(key).cloned()
    }

    /// 获取总大小
    pub fn len(&self) -> usize {
        self.total_size.load(Ordering::Relaxed)
    }
}

/// 渐进式扩容示例
fn resizable_hashmap_example() {
    let map = ResizableHashMap::new(4);
    let map_clone = Arc::clone(&map);

    // 启动扩容监控
    map.start_resize_monitor();

    let mut handles = vec![];

    // 多线程插入数据
    for i in 0..10 {
        let map_clone = Arc::clone(&map);
        let handle = thread::spawn(move || {
            for j in 0..1000 {
                let key = format!("key_{}_{}", i, j);
                let value = i * 1000 + j;
                map_clone.insert(key, value);

                // 模拟一些处理时间
                if j % 100 == 0 {
                    thread::sleep(Duration::from_millis(1));
                }
            }
        });
        handles.push(handle);
    }

    for handle in handles {
        handle.join().unwrap();
    }

    println!("最终HashMap大小: {}", map.len());
    println!("最终段数: {}", map.current_segment_count());
}
```

## 相关专题

- `../atomic/memory-ordering.md` - 内存序模型
- `../performance/cache-optimization.md` - CPU缓存优化
- `../debugging/concurrent-bugs.md` - 并发Bug调试
- `../threading/work-stealing.md` - 工作窃取算法