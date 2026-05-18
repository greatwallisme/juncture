# parking_lot 并发库指南

parking_lot 提供比标准库更快、更功能丰富的同步原语。在大多数场景下可直接替换 `std::sync`。

## 为什么用 parking_lot

- **更快**：无竞争时 Mutex 快约1.5x；有线程竞争时快最多5x；RwLock 在某些情况下快最多50x
- **更小**：Mutex 和 Once 只占**1字节**；Condvar 和 RwLock 只占**1个机器字**（vs std 需要动态分配的Box）
- **可作 static 全局变量**：由单个原子变量组成，无需 `lazy_static!` 初始化
- **更多功能**：公平锁、可重入锁、无 `unwrap()` 的接口，Condvar 额外等待变体
- **无毒化（Poisoning）**：lock() 直接返回 `MutexGuard`，不是 `Result`

## Mutex

```rust
use parking_lot::Mutex;

let mutex = Mutex::new(0i32);

// 无需 unwrap！
let mut guard = mutex.lock();
*guard += 1;
drop(guard);

// 非阻塞尝试
if let Some(mut guard) = mutex.try_lock() {
    *guard += 1;
}

// 超时
if let Some(mut guard) = mutex.try_lock_for(Duration::from_millis(100)) {
    *guard += 1;
}

// 截止时间前尝试加锁（返回Option，不保证成功）
if let Some(mut guard) = mutex.try_lock_until(Instant::now() + Duration::from_secs(1)) {
    *guard += 1;
}
```

## FairMutex - 公平锁

标准 Mutex 和 parking_lot Mutex 都不保证公平性（可能有线程饿死）。FairMutex 保证FIFO顺序。

```rust
use parking_lot::FairMutex;

let mutex = FairMutex::new(data);
let guard = mutex.lock();  // 严格按等待顺序获取锁
```

**何时用公平锁**：系统中有高优先级和低优先级混合操作，不能容忍低优先级任务饿死时。

## RwLock

```rust
use parking_lot::RwLock;

let lock = RwLock::new(HashMap::new());

// 读锁（允许多个并发读者）
{
    let map = lock.read();
    println!("{:?}", map.get("key"));
}  // 读锁在此释放

// 写锁（独占）
{
    let mut map = lock.write();
    map.insert("key".to_string(), "value".to_string());
}

// 尝试读/写
lock.try_read();
lock.try_write();
```

**parking_lot RwLock vs std RwLock**：
- parking_lot 可以在读锁升级为写锁（`upgradable_read()`）
- parking_lot 不会在写者饿死（有公平性保证）

## ReentrantMutex - 可重入锁

同一线程可以多次获取同一把锁（std::Mutex 会死锁）。

```rust
use parking_lot::ReentrantMutex;
use std::cell::RefCell;

let mutex = ReentrantMutex::new(RefCell::new(0));

let guard1 = mutex.lock();
let guard2 = mutex.lock();  // 同线程再次获取：OK
*guard2.borrow_mut() += 1;
```

**注意**：可重入锁容易掩盖设计问题，使用前确认真的需要。

## Once - 一次性初始化

```rust
use parking_lot::Once;

static INIT: Once = Once::new();

INIT.call_once(|| {
    // 只执行一次的初始化代码
    println!("initializing...");
});

// 检查是否已完成
INIT.state() == OnceState::Done
```

## Condvar

parking_lot 的 Condvar 与 std 有两个重要区别：
1. **无伪唤醒**：保证只在超时或被通知时唤醒（std 不保证）
2. **`notify_all` 只唤醒一个线程**：其余等待线程被重新排队等待关联的 Mutex，避免"群体效应"（thundering herd）。std 的 `notify_all` 会唤醒所有线程。

```rust
use parking_lot::{Mutex, Condvar};

let pair = Arc::new((Mutex::new(false), Condvar::new()));

// 等待方
let (lock, cvar) = &*pair;
let mut started = lock.lock();
while !*started {
    cvar.wait(&mut started);  // 注意：传入 &mut MutexGuard（vs std 按值传入）
}

// 通知方
let (lock, cvar) = &*pair;
let mut started = lock.lock();
*started = true;
cvar.notify_one();
```

**额外等待变体**（std 不具备）：
- `wait_until(&mut guard, instant)` — 等到截止时间
- `wait_while(&mut guard, |val| cond)` — 等到条件为false
- `wait_while_for(&mut guard, duration, |val| cond)` — 带超时的条件等待
- `wait_while_until(&mut guard, instant, |val| cond)` — 带截止时间的条件等待

## std vs parking_lot 对比

| 特性 | std::sync | parking_lot |
|------|-----------|-------------|
| Mutex大小 | ~40 bytes | 1 byte |
| lock()返回 | Result<Guard> | Guard |
| Poisoning | 有 | 无 |
| 公平锁 | 无 | FairMutex |
| 可重入锁 | 无 | ReentrantMutex |
| 条件变量 | `wait(MutexGuard<T>)`（按值传入，返回Result<Guard>） | `wait(&mut MutexGuard<T>)`（按可变引用传入） |
| 升级读锁 | 无 | `upgradable_read()` |

## 迁移建议

从 std 迁移到 parking_lot 通常只需修改 `use` 语句和移除 `.unwrap()`：

```rust
// std
use std::sync::{Mutex, RwLock};
let guard = mutex.lock().unwrap();

// parking_lot
use parking_lot::{Mutex, RwLock};
let guard = mutex.lock();  // 不再需要 unwrap
```
