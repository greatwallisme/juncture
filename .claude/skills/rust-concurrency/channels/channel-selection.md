# Rust Channel 通道选型指南

## 快速决策树

```
需要Channel?
├── 只需标准库，无依赖 → std::sync::mpsc
├── 需要 MPMC / select! / 定时器通道 → crossbeam-channel
├── 需要极简无unsafe代码 + MPMC + 同异步兼容 → flume
├── 同步代码和异步代码混用 → crossfire（基于crossbeam）
├── 纯异步环境 → tokio::sync 或 async_channel
├── 只需发一次值 → tokio::sync::oneshot 或 futures::channel::oneshot 或 kanal::oneshot
└── 极高吞吐量 → kanal
```

## 通道类型对照

| 类型 | std::mpsc | crossbeam | flume | tokio | async_channel | crossfire | kanal |
|------|-----------|-----------|-------|-------|---------------|-----------|-------|
| MPSC | ✅ | ✅ | ✅ | ✅ | ✅ | ✅(快) | ✅ |
| MPMC | ❌ | ✅ | ✅ | broadcast | ✅ | ✅ | ✅ |
| SPMC | ❌ | ✅ | ✅ | watch | ❌ | ❌ | ❌ |
| oneshot | ❌ | ❌ | ❌ | ✅ | ❌ | ❌ | ✅ |
| select! | ❌ | ✅ | Selector结构体 | tokio::select! | ❌ | ❌ | ❌ |
| 同步+异步 | ❌ | ❌ | ✅ | ❌ | ❌ | ✅ | ✅ |
| 定时器通道 | ❌ | after/tick/never | ❌ | time::sleep等 | ❌ | ❌ | ❌ |
| unsafe代码 | 有 | 有 | **无** | 有 | 有 | 有 | 有 |

> **flume 注**：没有 `select!` 宏，但提供 `Selector` 结构体可同时监听多个通道

---

## std::sync::mpsc

**适用**：标准库无额外依赖，简单的多生产者单消费者。

```rust
use std::sync::mpsc;
use std::thread;

// 异步(无界)通道
let (tx, rx) = mpsc::channel();

// 同步(有界)通道 - 缓冲区3，满时阻塞发送者
let (tx, rx) = mpsc::sync_channel(3);

// 多生产者：clone tx
let tx2 = tx.clone();

// 接收
rx.recv()         // 阻塞等待
rx.try_recv()     // 非阻塞，立即返回 Err 若无消息
rx.recv_timeout(Duration::from_millis(100))  // 超时接收
```

**陷阱**：
- 无MPMC支持，多消费者需要用Mutex包裹Receiver
- sync_channel缓冲为0时变成rendezvous channel（完全同步，发送必须等接收）
- 忘记drop所有tx会导致rx.recv()永久阻塞

---

## crossbeam-channel

**适用**：需要 select!、MPMC、定时器通道的生产级应用。

```rust
use crossbeam_channel::{bounded, unbounded, select, after, tick};
use std::time::Duration;

// 有界通道
let (s, r) = bounded(10);
// 无界通道
let (s, r) = unbounded::<i32>();

// select! 多路复用 (类似Go select)
select! {
    recv(r1) -> msg => { /* 处理r1消息 */ }
    recv(r2) -> msg => { /* 处理r2消息 */ }
    default   => { /* 无消息时执行 */ }
}

// 定时器通道
let timeout = after(Duration::from_secs(2));
let ticker  = tick(Duration::from_millis(100));

select! {
    recv(ticker)  -> _ => { println!("tick"); }
    recv(timeout) -> _ => { println!("timeout"); break; }
}
```

**关键特性**：
- `after(d)` — d后触发一次
- `at(instant)` — 特定时间点触发
- `never()` — 永不触发（用于条件性select分支）
- `tick(d)` — 每隔d触发（周期ticker）

---

## flume

**适用**：需要同时在sync和async代码中使用同一通道，且不希望引入unsafe。

```rust
use flume;

let (tx, rx) = flume::unbounded();
let (tx, rx) = flume::bounded(32);

// 同步发送/接收
tx.send(42)?;
let v = rx.recv()?;

// 异步发送/接收 (在async块中)
tx.send_async(42).await?;
let v = rx.recv_async().await?;

// 转成Stream (配合futures生态)
use futures::StreamExt;
let stream = rx.into_stream();
```

**优势**：整个代码库无 unsafe，兼容 sync+async，可无缝替换 std::mpsc。

---

## tokio::sync channels

**适用**：纯 tokio 异步应用，需要广播/watch等高级语义。

```rust
// mpsc - 多生产者单消费者 (最常用)
let (tx, mut rx) = tokio::sync::mpsc::channel(32);
tx.send(msg).await?;
rx.recv().await;

// oneshot - 一次性响应 (request/response模式)
let (tx, rx) = tokio::sync::oneshot::channel();
tx.send(result)?;  // 消耗tx
let result = rx.await?;

// broadcast - 一对多广播 (每个接收者都收到所有消息)
let (tx, mut rx1) = tokio::sync::broadcast::channel(16);
let mut rx2 = tx.subscribe();  // 新订阅者
tx.send(msg)?;
// rx1和rx2都会收到msg

// watch - 最新值通知 (配置更新、状态推送)
let (tx, rx) = tokio::sync::watch::channel(initial_value);
tx.send(new_value)?;
let current = rx.borrow();  // 读取最新值
rx.changed().await?;        // 等待值变化
```

**各类型使用场景**：
- `mpsc` — 任务向runtime汇报结果，工作队列
- `oneshot` — RPC风格请求响应，spawn后等待单次返回值
- `broadcast` — 事件广播，pub/sub，所有订阅者都需要收到
- `watch` — 配置热更新，最新状态快照（旧值自动丢弃）

---

## kanal

**适用**：追求极致吞吐量，比crossbeam快2-10x的场景。

```rust
use kanal;

let (tx, rx) = kanal::unbounded();
let (tx, rx) = kanal::bounded(32);

// API与crossbeam-channel类似
tx.send(msg)?;
rx.recv()?;

// 异步支持
tx.to_async().send(msg).await?;
rx.to_async().recv().await?;
```

---

## 常见陷阱

### 1. 通道泄漏导致永久阻塞
```rust
// 错误：tx克隆忘记drop，rx.recv()永不返回Err
let txs: Vec<_> = (0..4).map(|_| tx.clone()).collect();
drop(tx);  // 必须drop原始tx！
// 然后等待所有txs也被drop后，rx.recv()才会返回Err
```

### 2. sync_channel缓冲为0的rendezvous语义
```rust
let (tx, rx) = mpsc::sync_channel::<i32>(0);
// 发送者会阻塞，直到接收者调用recv()
// 两个线程必须同时"会面"才能完成传递
```

### 3. broadcast通道的滞后问题
```rust
let (tx, mut rx) = broadcast::channel(16);
// 若rx消费太慢，超过16条消息未读
// 后续recv()会返回 Err(RecvError::Lagged(n))
// 需要处理滞后情况
```

### 4. 在异步中使用同步channel
```rust
// 错误：在async任务中调用阻塞的recv()会阻塞整个线程
let msg = rx.recv()?;  // ❌ 在async fn中

// 正确：使用tokio的spawn_blocking或async版本
let msg = rx.recv_async().await?;  // ✅ flume的async接口
let msg = rx.recv().await?;         // ✅ tokio channel
```
