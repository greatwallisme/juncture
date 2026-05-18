# Rust 并发集合指南

## 快速选型

| 需求 | 推荐方案 | 理由 |
|------|---------|------|
| 通用并发Map | `DashMap` | 接口友好，性能好，内置分段锁 |
| 读多写极少(配置) | `ArcSwap` | 无锁读，适合配置快照 |
| 并发Vec/Map简单包装 | `Arc<Mutex<Vec<T>>>` | 最简单，正确性易保证 |
| 读多写少的Map | `Arc<RwLock<HashMap>>` | 允许并发读 |
| 布隆过滤器替代 | `cuckoofilter` | 支持删除，内存效率高 |

---

## Arc<Mutex<T>> 模式 - 通用并发容器

```rust
use std::sync::{Arc, Mutex};
use std::collections::HashMap;

// 线程安全的 Vec
let shared_vec: Arc<Mutex<Vec<i32>>> = Arc::new(Mutex::new(vec![]));

let handles: Vec<_> = (0..5).map(|i| {
    let v = Arc::clone(&shared_vec);
    std::thread::spawn(move || {
        v.lock().unwrap().push(i);
    })
}).collect();
for h in handles { h.join().unwrap(); }

// 线程安全的 HashMap
let shared_map: Arc<Mutex<HashMap<i32, i32>>> = Arc::new(Mutex::new(HashMap::new()));
```

**缺点**：整个容器一把锁，高并发时竞争严重。读多写少时考虑 `RwLock`。

---

## DashMap - 生产级并发HashMap

DashMap 是 `RwLock<HashMap<K,V>>` 的高性能直接替代，内部使用分段锁。

```rust
use dashmap::DashMap;
use std::sync::Arc;

let map = Arc::new(DashMap::new());

// 直接插入/读取，无需显式加锁
let handles: Vec<_> = (0..10).map(|i| {
    let m = Arc::clone(&map);
    std::thread::spawn(move || {
        m.insert(i, i * i);
    })
}).collect();
for h in handles { h.join().unwrap(); }

// 读取
if let Some(v) = map.get(&5) {
    println!("{}", *v);
}

// 可变访问
if let Some(mut v) = map.get_mut(&5) {
    *v *= 2;
}

// 条目API
map.entry(key).or_insert(0);
map.entry(key).and_modify(|v| *v += 1).or_insert(1);

// DashSet
use dashmap::DashSet;
let set = DashSet::new();
set.insert("hello");
```

**注意**：持有 `DashMap::get()` 返回的引用时，整个分片被锁定。不要长时间持有。

---

## ArcSwap - 读多写少的原子指针交换

适用场景：配置热更新、路由表、几秒更新一次的快照数据。

```rust
use arc_swap::ArcSwap;
use std::sync::Arc;

// 初始值
let config = ArcSwap::new(Arc::new(Config::default()));

// 读取（无锁，极快）
let current = config.load();  // 返回 Guard，短暂持有
println!("{:?}", *current);

// 原子更新（无需停止读取）
config.store(Arc::new(Config::new_version()));

// 比较交换
let old = config.load_full();  // Arc<Config>
config.compare_and_swap(&old, Arc::new(new_config));
```

**vs RwLock<Arc<T>>**：
- `RwLock<Arc<T>>`：需要加锁获取Arc，有CPU级别竞争，稳定读流量可能阻塞写
- `ArcSwap`：读完全无锁，写用原子操作，在读多写少场景性能显著更好

---

## evmap - 最终一致性并发Map

适用场景：允许读者看到"稍旧"版本数据，追求极致读性能。

```rust
use evmap;
use std::sync::{Arc, Mutex};

let (reader, writer) = evmap::new::<String, String>();

// 写操作需要Mutex保护（不支持并发写）
let writer = Arc::new(Mutex::new(writer));

let w = Arc::clone(&writer);
std::thread::spawn(move || {
    let mut w = w.lock().unwrap();
    w.insert("key".to_string(), "value".to_string());
    w.refresh();  // 发布更新，读者可见
});

// 读操作无锁，高并发友好
while reader.get_one("key").is_none() {
    std::thread::yield_now();
}
```

**注意**：`refresh()` 后读者才能看到新数据，写者和读者可能看到不同版本（最终一致性）。

---

## lockfree 无锁集合

```rust
// crossbeam提供的无锁队列（推荐，维护活跃）
use crossbeam::queue::{ArrayQueue, SegQueue};

// 有界无锁队列（固定容量，适合生产者-消费者）
let q: ArrayQueue<i32> = ArrayQueue::new(100);
// push 返回 Result<(), T>，满时 Err(value) 将值还给调用者
if q.push(42).is_err() {
    println!("queue is full");
}
// pop 返回 Option<T>，空时 None
if let Some(v) = q.pop() {
    println!("{}", v);
}

// 无界无锁队列
let q: SegQueue<i32> = SegQueue::new();
q.push(42);
let v = q.pop();
```

---

## 标准集合的线程安全包装决策

```
需要多线程访问集合?
├── 读写比例如何?
│   ├── 只读(初始化后不变) → Arc<T>即可，无需锁
│   ├── 读多写少 → Arc<RwLock<T>> 或 DashMap(如果是Map)
│   └── 频繁读写 → Arc<Mutex<T>> 或 DashMap
├── 操作粒度?
│   ├── 整体操作 → Arc<Mutex<T>>
│   └── 键级别操作 → DashMap(内置分段锁)
└── 是否需要最终一致性读? → evmap
```
