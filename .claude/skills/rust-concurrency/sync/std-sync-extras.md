# std 基础同步原语补充（Barrier/Once/Semaphore/Exclusive）

本文件覆盖书中第5章中 `sync/mutex-rwlock.md` 未涵盖的同步原语。

## Barrier — 屏障/栅栏

让所有参与线程在某一点同步，全部到达后才继续：

```rust
use std::sync::{Arc, Barrier};
use std::thread;

let barrier = Arc::new(Barrier::new(5));  // 需要5个线程到达

let mut handles = vec![];
for i in 0..5 {
    let b = Arc::clone(&barrier);
    handles.push(thread::spawn(move || {
        println!("线程 {} 已到达屏障", i);
        let wait_result = b.wait();  // 阻塞直到5个线程都调用wait()
        if wait_result.is_leader() {
            println!("线程 {} 是Leader，最后到达", i);
        }
        println!("线程 {} 继续执行", i);
    }));
}
for h in handles { h.join().unwrap(); }
```

**Barrier vs WaitGroup**（crossbeam）：
- Barrier 在构造时必须知道线程数；WaitGroup 可动态增加
- Barrier 可重用（所有线程同步后可再次使用）；WaitGroup 只同步一次
- 所有线程都等待；WaitGroup 中每个线程可选择等待或继续

**tokio::sync::Barrier** 提供异步版本，用法类似。

## Once / OnceLock — 一次性初始化

```rust
use std::sync::{Once, OnceLock};

// Once: 运行一次初始化代码（适合有副作用的初始化）
static INIT: Once = Once::new();
static mut GLOBAL_STATE: Option<String> = None;

INIT.call_once(|| {
    unsafe { GLOBAL_STATE = Some("initialized".to_string()); }
});

// OnceLock: 存储初始化后的值（Rust 1.70+，更安全）
static CONFIG: OnceLock<String> = OnceLock::new();
let val = CONFIG.get_or_init(|| "default_config".to_string());
```

**parking_lot::Once** 特性：可作为 `static` 而无需 `lazy_static!`，支持 `call_once_force` 处理中毒（panic）情况。

## LazyLock — 延迟初始化全局变量（Rust 1.80+）

替代 `lazy_static!` 宏的标准方式：

```rust
use std::sync::LazyLock;
use std::collections::HashMap;

static LOOKUP: LazyLock<HashMap<&str, u32>> = LazyLock::new(|| {
    let mut m = HashMap::new();
    m.insert("one", 1);
    m.insert("two", 2);
    m
});

fn main() {
    println!("{}", LOOKUP["one"]);  // 首次访问时初始化
}
```

## Exclusive\<T\> — 提供独占 &mut 访问（nightly）

`Exclusive<T>` 使任何 `T` 变成 `Sync`，通过只提供独占的可变引用来访问内部值：

```rust
// 目前是 nightly-only (#![feature(exclusive_wrapper)])
use std::sync::Exclusive;

let mut exclusive = Exclusive::new(RefCell::new(0));
let inner = exclusive.get_mut();  // 只能通过 &mut Exclusive 访问
```

## 信号量（Semaphore）— 限制并发访问数量

标准库没有 Semaphore，需使用第三方库：

```rust
// tokio::sync::Semaphore（异步环境）
use tokio::sync::Semaphore;
use std::sync::Arc;

let sem = Arc::new(Semaphore::new(3));  // 允许3个并发

async fn limited_op(sem: Arc<Semaphore>) {
    let _permit = sem.acquire().await.unwrap();
    // 同时最多3个任务执行此区域
    do_work().await;
}  // permit drop时释放

// parking_lot / std 没有内置 Semaphore
// 可以用 Mutex<u32> + Condvar 模拟：
use std::sync::{Arc, Mutex, Condvar};

struct Semaphore {
    count: Mutex<usize>,
    cv: Condvar,
}

impl Semaphore {
    fn new(n: usize) -> Self {
        Self { count: Mutex::new(n), cv: Condvar::new() }
    }
    fn acquire(&self) {
        let mut c = self.count.lock().unwrap();
        while *c == 0 { c = self.cv.wait(c).unwrap(); }
        *c -= 1;
    }
    fn release(&self) {
        let mut c = self.count.lock().unwrap();
        *c += 1;
        self.cv.notify_one();
    }
}
```

**选择建议**：
- 异步代码 → `tokio::sync::Semaphore`
- 同步代码 → `parking_lot` 没有，用 `Mutex + Condvar` 或 `async_lock::Semaphore`（block_on）
