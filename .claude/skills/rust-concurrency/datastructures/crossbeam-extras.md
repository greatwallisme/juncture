# Crossbeam 工具集补充

本文件覆盖书中第11章中其他文件未涵盖的 crossbeam 工具：
AtomicCell、WaitGroup、ShardedLock、Parker、Backoff、CachePadded、crossbeam-skiplist。

## AtomicCell\<T\> — 线程安全可变内存位置

`AtomicCell` 对任意 `T` 提供原子读写（内部用 atomic 指令或 mutex）：

```rust
use crossbeam::atomic::AtomicCell;

let cell = AtomicCell::new(0i32);

// 原子读写
cell.store(42);
let v = cell.load();  // 42

// 原子操作（对数值类型）
let old = cell.fetch_add(1);  // 返回旧值

// 在多线程中使用
use std::sync::Arc;
let shared = Arc::new(AtomicCell::new(0i32));
for _ in 0..10 {
    let cell = shared.clone();
    std::thread::spawn(move || {
        cell.fetch_add(1);
    });
}
```

**vs AtomicU32 等**：AtomicCell 对任意类型有效（不限 Copy + 固定大小），但对大类型会回退到 mutex。

## WaitGroup — 等待一组线程完成

`WaitGroup` 是 Go 语言 `sync.WaitGroup` 的 Rust 实现。标准库没有此原语：

```rust
use crossbeam::sync::WaitGroup;

let wg = WaitGroup::new();

for i in 0..5 {
    let wg = wg.clone();  // clone 注册一个线程
    std::thread::spawn(move || {
        // 模拟工作
        std::thread::sleep(std::time::Duration::from_millis(50));
        println!("线程 {} 完成", i);
        drop(wg);  // 等同于 wg.done()：减少计数
    });
}

wg.wait();  // 阻塞直到所有 clone 都被 drop
println!("所有线程完成");
```

**WaitGroup vs Barrier**：
- WaitGroup：等待"完成"；Barrier：等待"到达"某一点
- WaitGroup 线程数可动态增加；Barrier 在构造时固定
- WaitGroup 只同步一次；Barrier 可重用

**第三方 `wg` crate** 也提供类似功能，并支持异步：
```rust
use wg::WaitGroup;
let wg = WaitGroup::new();
let t = wg.add(1);  // 返回 token
std::thread::spawn(move || { t.done(); });
wg.wait();
```

## ShardedLock — 高性能分片读写锁

`ShardedLock` 通过分片减少读锁竞争，读比 RwLock 快，写比 RwLock 慢：

```rust
use crossbeam::sync::ShardedLock;

let lock = ShardedLock::new(5i32);

// 读（多线程并发）
{
    let r = lock.read().unwrap();
    println!("{}", *r);
}

// 写（独占）
{
    let mut w = lock.write().unwrap();
    *w += 1;
}
```

**适用场景**：读极多、写极少（如配置缓存）。内部按CPU核心数分片。

## Parker — 线程挂起/唤醒原语

比 `std::thread::park/unpark` 更可靠，不会有虚假唤醒：

```rust
use crossbeam::sync::Parker;

let p = Parker::new();
let u = p.unparker().clone();

// 先 unpark，再 park：park 立即返回（不丢失通知）
u.unpark();
p.park();  // 立即返回

// 通常用法：等待某个事件
let u2 = p.unparker().clone();
std::thread::spawn(move || {
    std::thread::sleep(std::time::Duration::from_millis(500));
    u2.unpark();
});
p.park();  // 等待约500ms
```

## Backoff — 自旋退避

在自旋循环中使用指数退避，减少竞争、提升性能：

```rust
use crossbeam::utils::Backoff;
use std::sync::atomic::{AtomicUsize, Ordering};

fn spin_until(flag: &AtomicUsize, expected: usize) {
    let backoff = Backoff::new();
    loop {
        if flag.load(Ordering::Acquire) == expected {
            return;
        }
        backoff.spin();  // 少量spin时：CPU yield指令；多次后：让出线程
    }
}

// snooze() 在多次 spin 失败后会让出线程
// is_completed() 检查是否应切换到阻塞等待
```

## CachePadded — 缓存行填充

防止不同变量共享缓存行（false sharing），提升多核性能：

```rust
use crossbeam::utils::CachePadded;
use std::sync::atomic::AtomicUsize;

// 不好：head 和 tail 可能在同一缓存行
struct BadQueue {
    head: AtomicUsize,
    tail: AtomicUsize,
}

// 好：每个字段独占一个缓存行
struct GoodQueue {
    head: CachePadded<AtomicUsize>,
    tail: CachePadded<AtomicUsize>,
}
```

缓存行大小假设：x86-64、aarch64、powerpc64 = 128字节；arm、mips = 32字节；s390x = 256字节；其他 = 64字节。

## crossbeam-skiplist — 并发有序Map/Set

基于 lock-free 跳表的并发容器（实验性）：

```rust
use crossbeam_skiplist::SkipMap;

let map = SkipMap::new();

// 多线程并发插入
crossbeam::thread::scope(|s| {
    s.spawn(|_| {
        map.insert("Alice", 30);
        map.insert("Bob", 25);
    });
    s.spawn(|_| {
        map.insert("Carol", 28);
    });
}).unwrap();

// 有序遍历
for entry in map.iter() {
    println!("{}: {}", entry.key(), entry.value());
}

map.remove("Alice");
assert!(!map.contains_key("Alice"));
```

**适用场景**：需要并发有序Map（替代 `Mutex<BTreeMap>`）。
