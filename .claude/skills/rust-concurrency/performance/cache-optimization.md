# CPU缓存优化技术

## 当使用此专题

- 优化高并发系统的缓存性能
- 减少CPU缓存未命中
- 设计缓存友好的数据结构
- 处理NUMA架构下的内存访问

## CPU缓存基础

### 缓存层次结构
```
L1 Cache: 32KB (per core) - 1-4 cycles
L2 Cache: 256KB (per core) - 10-20 cycles
L3 Cache: 8-32MB (shared) - 30-50 cycles
Main Memory: 8-64GB - 200-300 cycles
```

### 缓存行大小
- x86_64: 通常64字节
- ARM64: 通常64-128字节

## 缓存优化技术

### 1. 数据结构填充
```rust
use std::mem;

#[repr(align(64))] // 64字节对齐
pub struct CacheAlignedData<T> {
    data: T,
    _padding: [u8; 64 - std::mem::size_of::<T>() % 64],
}

pub struct Node<T> {
    value: T,
    next: *mut Node<T>,
    // 填充到缓存行边界
    _pad: [u8; 64 - (std::mem::size_of::<T>() + std::mem::size_of::<*mut<T>>()) % 64],
}

// 简化版本：对于64字节指针系统
pub struct CacheAlignedNode<T> {
    pub data: T,
    pub next: *mut Node<T>,
    _pad: [u8; 48], // 填充到64字节 (T + 8 + 48 = 64)
}
```

### 2. 避免伪共享 (False Sharing)
```rust
use std::sync::atomic::{AtomicU64, Ordering};

// ❌ 伪共享 - 多个Counter可能位于同一缓存行
struct BadCounters {
    counter1: AtomicU64,
    counter2: AtomicU64,
}

// ✅ 分离到不同缓存行
struct GoodCounters {
    counter1: AtomicU64,
    _pad1: [u8; 56], // 分隔64字节
    counter2: AtomicU64,
}

// 甚至更好的实现
struct OptimalCounters {
    counters: [CacheAlignedData<AtomicU64>; 2],
}

impl OptimalCounters {
    pub fn new() -> Self {
        Self {
            counters: [
                CacheAlignedData { data: AtomicU64::new(0), _padding: [0; 56] },
                CacheAlignedData { data: AtomicU64::new(0), _padding: [0; 56] },
            ],
        }
    }

    pub fn increment(&self, index: usize) {
        self.counters[index].data.fetch_add(1, Ordering::Relaxed);
    }
}
```

### 3. 数组对齐优化
```rust
use std::alloc::Layout;

pub struct AlignedArray<T> {
    data: Vec<T>,
    layout: Layout,
}

impl<T: Copy> AlignedArray<T> {
    pub fn new(size: usize, alignment: usize) -> Self {
        let layout = Layout::from_size_align(
            std::mem::size_of::<T>() * size,
            alignment,
        ).unwrap();

        unsafe {
            let ptr = std::alloc::alloc(layout) as *mut T;
            Vec::from_raw_parts(ptr, size)
        }
    }

    pub fn get(&self, index: usize) -> T {
        self.data[index]
    }

    pub fn set(&self, index: usize, value: T) {
        self.data[index] = value;
    }
}

impl<T: Copy> Drop for AlignedArray<T> {
    fn drop(&mut self) {
        unsafe {
            let layout = Layout::from_size_align(
                self.data.len() * std::mem::size_of::<T>(),
                64, // 假设64字节对齐
            ).unwrap();

            std::alloc::dealloc(
                self.data.as_mut_ptr() as *mut u8,
                layout,
            );
        }
    }
}
```

### 4. 预取优化
```rust
pub struct PrefetchedData {
    data: Vec<u8>,
}

impl PrefetchedData {
    pub fn new(data: Vec<u8>) -> Self {
        Self {
            data,
        }
    }

    // 手动预取
    #[inline]
    pub fn prefetch(&self, index: usize) {
        if let Some(&value) = self.data.get(index) {
            unsafe {
                // 使用内建汇编进行预取
                std::arch::x86_64::_mm_prefetch(
                    &value as *const _ as *const (),
                    std::arch::x86_64::_MM_HINT_T0,
                );
            }
        }
    }

    // 批量预取
    pub fn prefetch_range(&self, start: usize, end: usize) {
        for i in start..end {
            if i < self.data.len() {
                self.prefetch(i);
            }
        }
    }
}
```

## NUMA架构优化

### NUMA感知的内存分配
```rust
use std::sync::Arc;
use std::thread;

struct NUMAAwarePool<T> {
    pools: Vec<Vec<T>>,
    current_cpu: std::sync::atomic::AtomicUsize,
    elements_per_pool: usize,
}

impl<T: Default> NUMAAwarePool<T> {
    pub fn new(num_nodes: usize, elements_per_pool: usize) -> Self {
        let mut pools = Vec::new();
        for _ in 0..num_nodes {
            pools.push(Vec::with_capacity(elements_per_pool));
        }

        Self {
            pools,
            current_cpu: std::sync::atomic::AtomicUsize::new(0),
            elements_per_pool,
        }
    }

    pub fn allocate(&self) -> Option<T> {
        let node_id = self.get_current_numa_node();
        let pool = &mut self.pools[node_id];

        pool.pop().or_else(|| {
            // Pool exhausted, allocate new element
            Some(T::default())
        })
    }

    pub fn deallocate(&self, item: T, item_node: Option<usize>) {
        if let Some(node_id) = item_node {
            if node_id < self.pools.len() {
                let pool = &mut self.pools[node_id];
                if pool.len() < self.elements_per_pool {
                    pool.push(item);
                }
                // 否则让元素被drop
            }
        } else {
            // 放入当前NUMA节点的池中
            let node_id = self.get_current_numa_node();
            let pool = &mut self.pools[node_id];
            pool.push(item);
        }
    }

    fn get_current_numa_node(&self) -> usize {
        self.current_cpu.load(std::sync::atomic::Ordering::Relaxed) /
        (get_cpus_per_node().unwrap_or(1))
    }
}

fn get_cpus_per_node() -> Option<usize> {
    if cfg!(target_os = "linux") {
        Some(get_num_cpus() / get_numa_nodes())
    } else {
        None
    }
}

fn get_num_cpus() -> usize {
    num_cpus::get()
}

fn get_numa_nodes() -> usize {
    // 简化实现，实际应该读取系统信息
    2
}
```

## 高级优化技术

### 1. 循环展开与向量化
```rust
pub struct VectorizedOps;

impl VectorizedOps {
    // 4路循环展开
    #[inline]
    pub fn sum_unrolled(data: &[f32]) -> f32 {
        let mut sum = 0.0;
        let chunks = data.chunks_exact(4);

        for chunk in chunks {
            sum += chunk[0] + chunk[1] + chunk[2] + chunk[3];
        }

        // 处理剩余元素
        sum += data.chunks_exact(4).into_iter().sum::<f32>();

        sum
    }

    // 使用SIMD指令（需要nightly和feature gate）
    #[cfg(target_feature = "avx2")]
    pub fn sum_simd(data: &[f32]) -> f32 {
        use std::arch::x86_64::*;

        let (prefix, chunks, suffix) = unsafe { data.align_to::<f32x4>() };

        let mut sum = 0.0f32;

        // 处理前缀
        sum += prefix.iter().sum::<f32>();

        // 使用AVX向量化
        for chunk in chunks {
            let vector = f32x4::from_array([
                chunk[0],
                chunk[1],
                chunk[2],
                chunk[3],
            ]);

            sum += vector.iter().sum::<f32>();
        }

        // 处理后缀
        sum += suffix.iter().sum::<f32>();

        sum
    }

    #[cfg(not(target_feature = "avx2"))]
    pub fn sum_simd(data: &[f32]) -> f32 {
        Self::sum_unrolled(data)
    }
}
```

### 2. 分支预测优化
```rust
pub struct OptimizedHash<K, V> {
    entries: Vec<Entry<K, V>>,
}

struct Entry<K, V> {
    key: K,
    value: V,
    hash: u64,
}

impl<K: Clone + Eq, V: Clone> OptimizedHash<K, V> {
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: Vec::with_capacity(capacity),
        }
    }

    #[inline]
    pub fn get(&self, key: &K) -> Option<&V> {
        let hash = calculate_hash(key);
        let index = hash as usize % self.entries.len();

        let entry = &self.entries[index];

        // 首先检查哈希值，避免不必要的字符串比较
        if entry.hash == hash {
            if entry.key == *key {
                return Some(&entry.value);
            }
        }

        None
    }

    #[inline]
    pub fn insert(&mut self, key: K, value: V) -> Option<V> {
        let hash = calculate_hash(&key);
        let index = hash as usize % self.entries.len();

        if self.entries[index].key == key {
            let old_value = self.entries[index].value.clone();
            self.entries[index].value = value;
            self.entries[index].hash = hash;
            Some(old_value)
        } else {
            self.entries[index] = Entry {
                key,
                value,
                hash,
            };
            None
        }
    }
}

#[inline]
fn calculate_hash<T: std::hash::Hash>(item: &T) -> u64 {
    use std::hash::Hasher;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    item.hash(&mut hasher);
    hasher.finish()
}
```

### 3. 分支预测器友好的查找
```rust
pub struct BinarySearchTree<K: Ord, V> {
    root: Option<Box<Node<K, V>>>,
}

struct Node<K, V> {
    key: K,
    value: V,
    left: Option<Box<Node<K, V>>>,
    right: Option<Box<Node<K, V>>,
    _padding: [u8; 56], // 对齐到缓存行
}

impl<K: Ord, V> BinarySearchTree<K, V> {
    #[inline]
    pub fn find(&self, key: &K) -> Option<&V> {
        let mut current = self.root.as_ref();

        while let Some(node) = current {
            match key.cmp(&node.key) {
                std::cmp::Ordering::Equal => return Some(&node.value),
                std::cmp::Ordering::Less => current = node.left.as_ref(),
                std::cmp::Ordering::Greater => current = node.right.as_ref(),
            }
        }

        None
    }
}
```

## 性能基准

### 缓存对齐效果
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;
    use std::collections::HashMap;

    #[test]
    fn test_cache_alignment_performance() {
        const ITERATIONS: usize = 1_000_000;

        // 未对齐的计数器
        let counters_bad = BadCounters {
            counter1: std::sync::atomic::AtomicU64::new(0),
            counter2: std::sync::atomic::AtomicU64::new(0),
        };

        // 对齐的计数器
        let counters_good = GoodCounters::new();

        // 测试未对齐版本
        let start = Instant::now();
        for _ in 0..ITERATIONS {
            counters_bad.counter1.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            counters_bad.counter2.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        let unaligned_duration = start.elapsed();

        // 测试对齐版本
        let start = Instant::now();
        for _ in 0..ITERATIONS {
            counters_good.increment(0);
            counters_good.increment(1);
        }
        let aligned_duration = start.elapsed();

        println!("Unaligned: {:?}", unaligned_duration);
        println!("Aligned: {:?}", aligned_duration);

        // 对齐版本应该更快
        assert!(aligned_duration < unaligned_duration);
    }
}
```

### 数组访问模式对比
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    const ARRAY_SIZE: usize = 1_000_000;

    #[test]
    fn test_array_access_patterns() {
        let data = vec![42u8; ARRAY_SIZE];

        // 随机访问
        let start = Instant::now();
        let mut sum1 = 0;
        for i in (0..ARRAY_SIZE).step_by(64) {
            sum1 += data[i] as u32;
        }
        let strided_duration = start.elapsed();

        // 随机访问 + 预取
        let start = Instant::now();
        let mut sum2 = 0;
        for i in (0..ARRAY_SIZE).step_by(64) {
            unsafe {
                std::arch::x86_64::_mm_prefetch(
                    &data[i] as *const _ as *const (),
                    std::arch::x86_64::_MM_HINT_T0,
                );
            }
            sum2 += data[i] as u32;
        }
        let prefetched_duration = start.elapsed();

        // 随机访问 + 批量处理
        let start = Instant::now();
        let mut sum3 = 0;
        for chunk in data.chunks_exact(64) {
            sum3 += chunk.iter().sum::<u32>();
        }
        let batched_duration = start.elapsed();

        println!("Strided access: {:?}", strided_duration);
        println!("Prefetched access: {:?}", prefetched_duration);
        println!("Batched access: {:?}", batched_duration);

        // 批量处理通常最快
        assert!(batched_duration < strided_duration);
    }
}
```

## 相关模板

### 缓存对齐数据结构
- `../templates/atomic/cache-aligned-atomic.rs` - 缓存对齐的原子类型

### NUMA优化工具
- `../tools/analysis/numa-analyzer.rs` - NUMA架构分析工具

### 性能测试套件
- `../tools/testing/cache-performance-benchmark.rs` - 缓存性能基准测试

## 最佳实践

### 1. 对齐规则
- 64字节对齐是x86_64的黄金标准
- 指针通常8字节对齐，低3位为0
- 避免跨越缓存行的结构体

### 2. 访问模式
- 优先使用顺序访问
- 避免随机访问大数组
- 预取已知要访问的数据

### 3. 数据布局
- 热数据放前面，冷数据放后面
- 相关数据聚集在一起
- 使用结构体数组替代对象池

### 4. NUMA考虑
- 每个NUMA节点独立的内存池
- 线程绑定到特定NUMA节点
- 避免跨NUMA节点的频繁访问

## 相关专题

- `../performance/numa-programming.md` - NUMA架构编程