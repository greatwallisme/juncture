# Tokio 同步原语与时间 API

对应书中第13章（同步原语与时间相关部分）。
Tokio 的同步原语专为**异步代码跨 `.await` 保持锁**而设计。

## 何时用 tokio::sync vs std::sync

> **重要**：在异步代码中，如果临界区内没有 `.await`，优先使用 `std::sync::Mutex` 或 `parking_lot::Mutex`（更快）。
> `tokio::sync::Mutex` 的优势是**可以在 `.await` 点跨越持有锁**。

## tokio::sync::Mutex

```rust
use tokio::sync::Mutex;
use std::sync::Arc;

let mutex = Arc::new(Mutex::new(0));

async fn increment(mutex: Arc<Mutex<i32>>) {
    let mut guard = mutex.lock().await;  // 异步等待锁
    *guard += 1;
    // 可以在持有锁时 .await（std::sync::Mutex 不能这样做）
}
```

**注意**：tokio Mutex 不会因 panic 而毒化（vs std::sync::Mutex 会 poison）。

## tokio::sync::Notify — 事件通知

类似 Condvar，但更简单：单次许可通知机制：

```rust
use tokio::sync::Notify;
use std::sync::Arc;

let notify = Arc::new(Notify::new());
let notify2 = notify.clone();

// 等待方（接收通知）
tokio::spawn(async move {
    notify2.notified().await;  // 等待通知
    println!("收到通知！");
});

// 通知方
tokio::time::sleep(Duration::from_millis(100)).await;
notify.notify_one();  // 唤醒一个等待者

// notify_waiters() 唤醒所有当前等待者
notify.notify_waiters();
```

**与 Condvar 的区别**：
- `notify_one()` 在没有等待者时存储许可，下一次 `notified().await` 立即返回
- 适合"事件驱动"模式，不需要关联 Mutex

## tokio::sync::Semaphore — 并发限制

```rust
use tokio::sync::Semaphore;
use std::sync::Arc;

// 限制最多3个并发任务
let sem = Arc::new(Semaphore::new(3));

async fn limited_task(sem: Arc<Semaphore>, id: u32) {
    let _permit = sem.acquire().await.unwrap();
    println!("任务 {} 获得许可", id);
    tokio::time::sleep(Duration::from_millis(100)).await;
}  // permit 在这里自动释放

// 或者获取多个许可
let permits = sem.acquire_many(2).await.unwrap();
```

## tokio::sync::OnceCell — 异步单次初始化

允许**初始化过程本身是异步**的：

```rust
use tokio::sync::OnceCell;

static DB: OnceCell<DatabaseConnection> = OnceCell::const_new();

async fn get_db() -> &'static DatabaseConnection {
    DB.get_or_init(|| async {
        DatabaseConnection::connect("postgres://...").await.unwrap()
    }).await
}
```

## tokio::time — 时间 API

### sleep — 异步等待

```rust
use tokio::time::{sleep, Duration};

sleep(Duration::from_millis(100)).await;  // std::thread::sleep 的异步对应
```

最大持续时间约 2.2 年，精度为毫秒级。

### interval — 周期性触发

```rust
use tokio::time::{interval, Duration};

let mut interval = interval(Duration::from_millis(10));

loop {
    interval.tick().await;  // 第一次立即返回，之后每10ms返回一次
    do_periodic_work().await;
}
```

`interval_at(start, period)` — 在指定时间点开始的周期间隔。

### timeout — 操作超时

```rust
use tokio::time::{timeout, Duration};

match timeout(Duration::from_millis(100), some_async_op()).await {
    Ok(result) => println!("成功: {:?}", result),
    Err(_) => println!("超时！"),
}

// timeout_at 使用绝对时间点
use tokio::time::{timeout_at, Instant};
timeout_at(Instant::now() + Duration::from_secs(5), op()).await;
```

## tokio::process::Command — 异步进程管理

```rust
use tokio::process::Command;

let output = Command::new("echo")
    .arg("hello world")
    .output()
    .await?;
assert!(output.status.success());

// 逐行处理输出
use tokio::io::{BufReader, AsyncBufReadExt};
let mut cmd = Command::new("cat").stdout(Stdio::piped()).spawn()?;
let stdout = cmd.stdout.take().unwrap();
let mut lines = BufReader::new(stdout).lines();
while let Some(line) = lines.next_line().await? {
    println!("{}", line);
}
```

## tokio 同步原语选择

| 需求 | 选择 |
|------|------|
| 跨 .await 持有锁 | `tokio::sync::Mutex` |
| 不跨 .await | `std::sync::Mutex` 或 `parking_lot` |
| 事件通知 | `tokio::sync::Notify` |
| 并发限制 | `tokio::sync::Semaphore` |
| 异步初始化全局 | `tokio::sync::OnceCell` |
| 多生产者单消费者 | `tokio::sync::mpsc` |
| 单次消息 | `tokio::sync::oneshot` |
| 广播(最新值) | `tokio::sync::watch` |
| 广播(所有值) | `tokio::sync::broadcast` |
