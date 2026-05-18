---
name: rust-concurrency
description: "Production-grade Rust concurrency: threads, atomics, async/await, lock-free data structures, memory ordering, ABA problem, executor design, NUMA, work-stealing, deadlock prevention. Use when: building high-concurrency systems, implementing lock-free algorithms, debugging race conditions, optimizing async runtimes, handling synchronization primitives, choosing channel types (mpsc/crossbeam/flume/tokio), working with DashMap/ArcSwap/parking_lot, or asking about thread patterns (scoped threads, ThreadLocal, park/unpark, affinity, priority)."
---

## NEVER DO THESE

1. **NEVER** 在不知道内存序含义时使用SeqCst - 5-10x性能损失
2. **NEVER** 在持有锁时进行耗时操作 - 导致所有线程阻塞
3. **NEVER** 无条件使用try_lock循环自旋 - CPU空转浪费
4. **NEVER** 假设relaxed atomic能同步任何数据 - 架构相关
5. **NEVER** 在不理解ABA问题时实现无锁结构 - 概率性崩溃
6. **NEVER** 使用AtomicBool::load(Ordering::SeqCst)作为简单标志 - Release/Acquire足够
7. **NEVER** 在async fn中调用同步阻塞recv() - 阻塞整个executor线程
8. **NEVER** 忘记drop原始Sender - 导致Receiver永久阻塞
9. **NEVER** 在scoped thread外访问已析构的变量 - UB
10. **NEVER** 用sync_channel(0)的rendezvous语义做高吞吐传递 - 每次发送都要等接收

---

## 并发设计思维原则

在实现并发系统前，问自己：

### 1. 正确性优先
- 数据竞争是否已证明不可能发生？（通过类型系统验证）
- 死锁场景是否已枚举并预防？（锁顺序、超时机制）
- 所有panic路径是否已处理？（join结果检查，不要盲目unwrap）

### 2. 性能瓶颈定位
- 瓶颈在锁竞争、内存带宽还是CPU缓存？（先测量再优化）
- 是否有profiling数据支持优化方向？（cargo-flamegraph、criterion）
- 优化后性能提升是否超过复杂度增加的代价？

### 3. 复杂度代价
- 无锁算法的正确性证明成本是否值得？（考虑使用crossbeam）
- 是否有更简单的替代方案？（Mutex通常足够）
- 团队是否有能力维护此复杂度？

### 4. 可测试性
- 如何测试并发正确性？（loom模型检查、miri数据竞争检测）
- 是否使用了静态分析工具？（clippy lint）
- 是否有压力测试验证稳定性？

---

## 并发方案决策树

### 线程 vs 异步
| 场景 | 推荐方案 | 加载文档 |
|------|---------|---------|
| CPU密集型计算 | 线程池 / rayon | `threading/work-stealing.md` |
| I/O密集型操作 | async运行时(tokio) | `runtime/executor-design.md` |
| 混合负载 | tokio + rayon | 两者都加载 |

### 同步原语选择
| 需求 | 首选 | 备选 |
|------|------|------|
| 简单计数器 | AtomicU64 | Mutex<u64> |
| 读多写少 | RwLock / ArcSwap | Atomic + sharding |
| 复杂临界区 | parking_lot::Mutex | std::Mutex |
| 无阻塞需求 | crossbeam队列 | 考虑lockfree |

### 通道选型快速表
| 场景 | 推荐 |
|------|------|
| 无额外依赖 MPSC | `std::sync::mpsc` |
| MPMC + select! | `crossbeam-channel` |
| sync+async双用途 | `flume` |
| 纯tokio | `tokio::sync::mpsc/broadcast/watch` |
| 极高吞吐 | `kanal` |
| 一次性响应 | `tokio::sync::oneshot` |

---

## 按需加载专题

### 线程基础与模式

**MANDATORY** - 在处理线程创建、scoped thread、park/unpark、ThreadLocal、线程优先级/亲和性时，阅读 [`threading/thread-patterns.md`](./threading/thread-patterns.md)

**场景检测**：
- "scoped thread"、"borrowed data in thread"、"非'static数据"
- "park"、"unpark"、"ThreadLocal"
- "线程优先级"、"CPU affinity"、"绑核"
- "Send"、"Sync"、"!Send"、"send_wrapper"

---

### Channel 通道选型

**MANDATORY** - 在选择通道库或遇到通道相关问题时，必须阅读 [`channels/channel-selection.md`](./channels/channel-selection.md)

**场景检测**：
- "channel"、"通道"、"mpsc"、"mpmc" → 加载此文件
- "crossbeam-channel"、"flume"、"kanal"、"async_channel" → 加载此文件
- "select!"、"多路复用"、"同时监听多个通道" → 加载此文件
- "broadcast"、"watch"、"oneshot"、"广播" → 加载此文件

---

### 同步原语

**MANDATORY** - 在使用锁前阅读 [`sync/mutex-rwlock.md`](./sync/mutex-rwlock.md) 重点关注死锁预防部分

**场景检测**：
- "死锁"、"deadlock"、"锁顺序" → 加载此文件
- "Mutex"、"RwLock"、"锁竞争" → 加载此文件

**parking_lot** - 需要更高性能或更多功能的锁时，阅读 [`sync/parking-lot.md`](./sync/parking-lot.md)

**场景检测**：
- "parking_lot"、"公平锁"、"FairMutex"、"可重入锁" → 加载此文件
- "比std Mutex更快"、"lock().unwrap()太烦" → 加载此文件

---

### 内存序模型

**MANDATORY** - 在使用任何原子操作前，必须阅读 [`atomic/memory-ordering.md`](./atomic/memory-ordering.md)

**场景检测**：
- "原子"、"atomic"、"compare_exchange" → 加载此文件
- "内存序"、"memory ordering"、"happens-before" → 加载此文件
- "Acquire"、"Release"、"SeqCst"、"Relaxed" → 加载此文件

**速查：Ordering 选择原则**：
- `Relaxed` — 无同步需求，只需原子性（如独立计数器）
- `Release`(store) + `Acquire`(load) — 标准同步对，写-读建立happens-before
- `AcqRel` — 读改写操作同时需要两者语义
- `SeqCst` — 全局全序，最强保证，有明确需求时才用

---

### 并发集合

**MANDATORY** - 在使用线程安全集合时，阅读 [`datastructures/concurrent-collections.md`](./datastructures/concurrent-collections.md)

**场景检测**：
- "DashMap"、"并发HashMap"、"线程安全Map" → 加载此文件
- "ArcSwap"、"arc-swap"、"配置热更新"、"读多写少" → 加载此文件
- "evmap"、"最终一致性Map" → 加载此文件

**MANDATORY** - 在实现分段锁或自定义并发Map时，加载 [`datastructures/concurrent-map.md`](./datastructures/concurrent-map.md)

---

### 无锁数据结构

**MANDATORY** - 在实现无锁队列/栈前，必须阅读 [`datastructures/lockfree-queue.md`](./datastructures/lockfree-queue.md)

**场景检测**：
- "无锁"、"lock-free"、"ArrayQueue"、"SegQueue" → 加载此文件
- "MPMC"、"SPSC"、"Michael-Scott" → 加载此文件
- "CAS"、"compare_exchange" → 加载此文件

**配合加载**：
- 如果涉及指针复用，同时加载 [`atomic/aba-problem.md`](./atomic/aba-problem.md)
- 如果涉及内存释放，同时加载 [`datastructures/memory-reclamation.md`](./datastructures/memory-reclamation.md)

---

### 工作窃取线程池

**MANDATORY** - 在实现任务调度器前，必须阅读 [`threading/work-stealing.md`](./threading/work-stealing.md)

**场景检测**：
- "线程池"、"thread pool"、"工作窃取"、"rayon" → 加载此文件
- "work-stealing"、"任务调度"、"NUMA" → 加载此文件

**线程池库快速选型**：
- `rayon` — CPU密集型并行计算，parallel iterators
- `threadpool` — 简单任务队列
- `rusty_pool` — 支持Future的混合线程池
- `scheduled_thread_pool` — 定时/周期任务
- `fast_threadpool` — 高吞吐量低延迟

---

### 异步运行时设计

**MANDATORY** - 在设计执行器前，必须阅读 [`runtime/executor-design.md`](./runtime/executor-design.md)

**场景检测**：
- "执行器"、"executor"、"Future"、"async runtime" → 加载此文件

**配合加载**：如果涉及Future实现细节，同时加载 [`async/future-trait.md`](./async/future-trait.md)

---

### 异步 I/O 操作

**MANDATORY** - 在实现异步文件/网络操作时阅读：
- [`async-io/async-file-operations.md`](./async-io/async-file-operations.md) — 异步文件读写、Tokio fs
- [`async-io/async-network-programming.md`](./async-io/async-network-programming.md) — TcpListener/TcpStream、UDP

**场景检测**：
- "异步文件"、"async file"、"Tokio fs" → 加载 async-file-operations.md
- "异步网络"、"async network"、"TcpListener"、"TcpStream" → 加载 async-network-programming.md

---

### 并发调试

**MANDATORY** - 在调试竞态条件时，阅读 [`debugging/concurrent-bugs.md`](./debugging/concurrent-bugs.md)

**场景检测**：
- "竞态"、"race condition"、"数据竞争"、"死锁" → 加载此文件
- "loom"、"miri"、"ThreadSanitizer" → 加载此文件

---

### 性能优化

**MANDATORY** - 当用户提到性能优化时，阅读：
- [`performance/cache-optimization.md`](./performance/cache-optimization.md) - 缓存行优化、伪共享、CachePadded
- [`performance/numa-programming.md`](./performance/numa-programming.md) - NUMA架构编程

**场景检测**：
- "性能优化"、"cache miss"、"NUMA"、"缓存对齐"、"伪共享"、"CachePadded"

---

### 并发模式

**MANDATORY** - 在设计并发架构时阅读 [`patterns/concurrent-patterns.md`](./patterns/concurrent-patterns.md)

**场景检测**：
- "生产者消费者"、"工作池"、"pipeline"、"fan-out/fan-in" → 加载此文件

**配合加载**：如果涉及错误传播策略，同时加载 [`patterns/error-handling.md`](./patterns/error-handling.md)

---

### 压力测试与稳定性验证

**MANDATORY** - 在验证并发系统稳定性时阅读 [`testing/stress-testing.md`](./testing/stress-testing.md)

**场景检测**：
- "压力测试"、"压测"、"负载测试"、"模糊测试" → 加载此文件
- "并发测试"、"fuzz testing"、"稳定性测试" → 加载此文件

---

### 容器同步原语（Cell / RefCell / OnceCell / LazyLock）

**MANDATORY** - 在使用内部可变性类型或延迟初始化时，阅读 [`basics/cell-types.md`](./basics/cell-types.md)

**场景检测**：
- "Cell"、"RefCell"、"OnceCell"、"LazyCell"、"LazyLock"、"OnceLock" → 加载此文件
- "内部可变性"、"单次初始化"、"延迟初始化"、"lazy_static 替代" → 加载此文件
- "Rc vs Arc"、"Cow"、"写时克隆" → 加载此文件

---

### std 基础同步原语补充（Barrier / Once / Semaphore）

**MANDATORY** - 在使用 Barrier、Once、Semaphore 或 LazyLock 时，阅读 [`sync/std-sync-extras.md`](./sync/std-sync-extras.md)

**场景检测**：
- "Barrier"、"屏障"、"栅栏"、"同步到达点" → 加载此文件
- "Once"、"OnceLock"、"一次性初始化"、"全局变量初始化" → 加载此文件
- "Semaphore"、"信号量"、"限制并发数" → 加载此文件

---

### Rayon 并行编程

**MANDATORY** - 在使用 rayon 或数据并行时，阅读 [`threading/rayon-parallel.md`](./threading/rayon-parallel.md)

**场景检测**：
- "rayon"、"par_iter"、"并行迭代"、"parallel iterator" → 加载此文件
- "rayon::join"、"rayon::scope"、"fork-join" → 加载此文件
- "数据并行"、"CPU密集型并行"、"并行sort" → 加载此文件

---

### Tokio 同步原语与时间 API

**MANDATORY** - 在使用 tokio 同步原语或时间 API 时，阅读 [`async/tokio-sync.md`](./async/tokio-sync.md)

**场景检测**：
- "tokio::sync::Notify"、"事件通知"、"notify_one" → 加载此文件
- "tokio::sync::Semaphore"、"tokio::sync::OnceCell" → 加载此文件
- "tokio::time"、"sleep"、"interval"、"timeout"、"异步定时" → 加载此文件
- "tokio::process"、"异步进程" → 加载此文件

---

### 定时器模式（Timer / Ticker）

**MANDATORY** - 在实现定时或周期性任务时，阅读 [`timer/timer-patterns.md`](./timer/timer-patterns.md)

**场景检测**：
- "timer"、"定时器"、"定时任务"、"延迟执行" → 加载此文件
- "ticker"、"周期性任务"、"interval"、"tick" → 加载此文件
- "timer 库"、"async-io Timer"、"crossbeam after/tick" → 加载此文件

---

### Crossbeam 工具集（AtomicCell / WaitGroup / Backoff 等）

**MANDATORY** - 在使用 crossbeam 非通道工具时，阅读 [`datastructures/crossbeam-extras.md`](./datastructures/crossbeam-extras.md)

**场景检测**：
- "AtomicCell"、"crossbeam 原子" → 加载此文件
- "WaitGroup"、"等待所有线程完成"、"wg" → 加载此文件
- "ShardedLock"、"分片锁"、"ShardedRwLock" → 加载此文件
- "Parker"、"Unparker"、"crossbeam park" → 加载此文件
- "Backoff"、"指数退避"、"自旋退避" → 加载此文件
- "CachePadded"、"缓存行填充"、"false sharing" → 加载此文件（也见 performance/cache-optimization.md）
- "crossbeam-skiplist"、"SkipMap"、"SkipSet" → 加载此文件

---

## 快速代码模板

### Arc<Mutex<T>> 基础模式
```rust
use std::sync::{Arc, Mutex};
let shared = Arc::new(Mutex::new(vec![]));
let clone = Arc::clone(&shared);
std::thread::spawn(move || {
    clone.lock().unwrap().push(42);
}).join().unwrap();
```

### DashMap 并发Map
```rust
use dashmap::DashMap;
let map = Arc::new(DashMap::new());
map.insert("key", "value");
map.entry("counter").and_modify(|v| *v += 1).or_insert(1);
```

### crossbeam select!
```rust
use crossbeam_channel::{select, tick, after};
let ticker = tick(Duration::from_millis(100));
let timeout = after(Duration::from_secs(5));
loop {
    select! {
        recv(ticker) -> _ => { /* periodic work */ }
        recv(timeout) -> _ => break,
    }
}
```

### parking_lot Mutex
```rust
use parking_lot::Mutex;
let mutex = Mutex::new(0);
let mut guard = mutex.lock(); // 无需 .unwrap()
*guard += 1;
```

### Scoped Thread (借用栈变量)
```rust
let data = vec![1, 2, 3];
let mut result = 0;
std::thread::scope(|s| {
    s.spawn(|| { result = data.iter().sum(); });
});
println!("{result}");
```

---

## 代码模板文件

- [`templates/thread-pools/thread-pool-basic.rs`](./templates/thread-pools/thread-pool-basic.rs)
- [`templates/thread-pools/thread-pool-work-stealing.rs`](./templates/thread-pools/thread-pool-work-stealing.rs)
- [`templates/async/simple-executor.rs`](./templates/async/simple-executor.rs)
- [`templates/async/async-runtime-config.rs`](./templates/async/async-runtime-config.rs)
- [`templates/atomic/versioned-pointer.rs`](./templates/atomic/versioned-pointer.rs)

## 示例代码

- [`examples/atomic-counter.rs`](./examples/atomic-counter.rs) — 原子计数器完整实现
- [`examples/lock-free-queue.rs`](./examples/lock-free-queue.rs) — 无锁队列完整实现
- [`examples/async-web-server.rs`](./examples/async-web-server.rs) — 异步Web服务器示例

## 补充参考

- [`basics/thread-fundamentals.md`](./basics/thread-fundamentals.md) — 线程陷阱与专家提示（补充 thread-patterns.md）
- [`basics/cell-types.md`](./basics/cell-types.md) — Cell/RefCell/OnceCell/LazyLock/Rc/Cow（书第4章）
- [`sync/std-sync-extras.md`](./sync/std-sync-extras.md) — Barrier/Once/OnceLock/Semaphore
- [`threading/rayon-parallel.md`](./threading/rayon-parallel.md) — rayon 并行集合/scope/join
- [`async/tokio-sync.md`](./async/tokio-sync.md) — tokio Notify/Semaphore/OnceCell/time API
- [`timer/timer-patterns.md`](./timer/timer-patterns.md) — 定时器与周期任务模式
- [`datastructures/crossbeam-extras.md`](./datastructures/crossbeam-extras.md) — crossbeam AtomicCell/WaitGroup/Backoff等

## 测试与调试工具

- [`tools/testing/async-test-framework.md`](./tools/testing/async-test-framework.md) — 异步测试框架
- [`tools/testing/race-condition-detector.rs`](./tools/testing/race-condition-detector.rs) — 竞态检测
- [`tools/analysis/contention-analyzer.rs`](./tools/analysis/contention-analyzer.rs) — 锁竞争分析
- [`tools/contention-analyzer.sh`](./tools/contention-analyzer.sh) — 竞争分析脚本
- [`tools/performance-benchmark.rs`](./tools/performance-benchmark.rs) — 性能基准测试
