# Rust 定时器模式（Timer / Ticker）

对应书中第9章。定时器用于延迟执行，Ticker 用于周期性任务。

## 最简单的定时器：thread::sleep

```rust
use std::thread;
use std::time::Duration;

thread::sleep(Duration::from_secs(5));
println!("5秒后执行");
```

异步版本：`tokio::time::sleep`（见 `async/tokio-sync.md`）

## timer 库 — 多线程定时任务调度

```rust
use timer::Timer;
use chrono::Duration;

let timer = Timer::new();

// 3秒后执行一次
let _guard = timer.schedule_with_delay(Duration::seconds(3), move || {
    println!("3秒后执行");
});

// 阻塞等待（guard drop会取消定时器）
std::thread::sleep(std::time::Duration::from_secs(5));

// 周期性执行（每隔1秒）
let count = std::sync::Mutex::new(0);
let _guard = timer.schedule_repeating(Duration::milliseconds(200), move || {
    let mut c = count.lock().unwrap();
    *c += 1;
});
```

内部用两个线程实现：调度线程（执行回调）和通信线程。

## async-io Timer — 异步 Timer/Stream

`async_io::Timer` 既是 Future 也是 Stream：

```rust
use async_io::Timer;
use std::time::Duration;

// 作为 Future：等待一次
Timer::after(Duration::from_secs(1)).await;

// 永不触发（用于条件性 select 分支）
let timer = Timer::never();

// 作为 Stream：周期性
use futures_lite::StreamExt;
let period = Duration::from_secs(1);
Timer::interval(period).next().await;  // 等待一个周期

// 从特定时间点开始的间隔
let start = std::time::Instant::now();
Timer::interval_at(start, period).next().await;
```

## crossbeam-channel 定时器通道

crossbeam 内置三种特殊通道（只有接收端）：

```rust
use crossbeam_channel::{after, tick, never, select};
use std::time::Duration;

let ticker = tick(Duration::from_millis(50));    // 每50ms触发
let timeout = after(Duration::from_secs(1));     // 1秒后触发一次
let never_ch = never::<()>();                    // 永不触发

loop {
    select! {
        recv(ticker) -> _ => println!("tick"),
        recv(timeout) -> _ => { println!("超时"); break; }
        recv(never_ch) -> _ => unreachable!(),  // 用于禁用某个分支
    }
}
```

## ticker 库 — 迭代器限流 Ticker

```rust
use ticker::Ticker;
use std::time::Duration;

// 每秒处理一个元素，总共10次
let ticker = Ticker::new(0..10, Duration::from_secs(1));
for i in ticker {
    println!("{:?}", i);
}
```

## tokio::time — 异步时间（推荐）

见 `async/tokio-sync.md` 中的 `tokio::time` 部分，包括：
- `sleep(duration)` — 等待
- `interval(period)` — 周期触发
- `timeout(duration, future)` — 超时保护

## 选型指南

| 场景 | 推荐 |
|------|------|
| 同步代码延迟 | `std::thread::sleep` |
| 同步周期任务 | `timer` 库或 `crossbeam tick` |
| 异步延迟 | `tokio::time::sleep` |
| 异步周期任务 | `tokio::time::interval` |
| 异步超时保护 | `tokio::time::timeout` |
| select! 中的超时 | `crossbeam_channel::after` |
| 迭代器限流 | `ticker` 库 |
