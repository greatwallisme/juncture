# Rust 线程模式指南

## 线程创建模式

### 基础创建
```rust
use std::thread;

// 最简单的线程
let handle = thread::spawn(|| {
    println!("Hello from thread!");
});
handle.join().unwrap();  // 必须join，否则线程可能不执行完

// 获取线程返回值
let handle = thread::spawn(|| -> i32 { 42 });
let result = handle.join().unwrap();  // result == 42

// 启动N个线程
let handles: Vec<_> = (0..N).map(|i| {
    thread::spawn(move || {
        println!("Thread {}", i);
    })
}).collect();
for h in handles { h.join().unwrap(); }
```

### ThreadBuilder - 精细控制
```rust
let builder = thread::Builder::new()
    .name("worker-thread".into())   // 便于调试/profiling
    .stack_size(4 * 1024 * 1024);  // 4MB栈（默认2MB）

let handle = builder.spawn(|| {
    // 线程体
}).unwrap();
```

**何时设置栈大小**：
- 深递归算法（如递归解析器）
- 栈上分配大数组
- 嵌套深度不确定的场景
- 默认2MB通常足够，除非有明确需要

### Scoped Thread - 安全借用栈变量
```rust
// 普通spawn无法借用非'static数据
// scoped thread解决这个问题

let data = vec![1, 2, 3];
let mut result = 0;

thread::scope(|s| {
    s.spawn(|| {
        println!("{:?}", data);  // 可以借用data！
    });
    s.spawn(|| {
        result = data[0] + data[2];  // 可以借用和修改
    });
});
// scope结束后，所有scoped线程已join

// crossbeam scope支持孙线程 (crossbeam crate，注意路径是 crossbeam::thread::scope)
crossbeam::thread::scope(|s| {
    s.spawn(|s| {           // s传递给子线程
        s.spawn(|_| {       // 子线程中再spawn孙线程
            println!("grandchild");
        });
    });
}).unwrap();

// rayon scope用于fork-join并行
rayon::scope(|s| {
    s.spawn(|_| { /* parallel task */ });
    s.spawn(|_| { /* parallel task */ });
});
```

---

## 线程本地存储 (ThreadLocal)

```rust
use std::cell::RefCell;

thread_local! {
    static COUNTER: RefCell<u32> = RefCell::new(0);
}

fn increment() {
    COUNTER.with(|c| {
        *c.borrow_mut() += 1;
    });
}

fn get() -> u32 {
    COUNTER.with(|c| *c.borrow())
}
```

**适用场景**：
- 线程私有缓存（避免锁竞争）
- 随机数生成器（如每线程一个RNG）
- 连接池的本地连接
- 统计计数器（聚合时汇总各线程）

**注意**：TLS值在线程退出时被销毁，不要跨线程访问TLS。

---

## park / unpark - 低层线程同步

```rust
use std::thread;
use std::sync::Arc;

// park: 阻塞当前线程（释放CPU）
// unpark: 唤醒被park的线程
// 每个线程有一个"令牌"，unpark放入令牌，park消耗令牌

let parked = thread::spawn(|| {
    println!("going to park");
    thread::park();           // 阻塞，等待令牌
    println!("unparked!");
});

thread::sleep(Duration::from_millis(10));
parked.thread().unpark();    // 发放令牌
parked.join().unwrap();

// 重要：令牌最多一个！多次unpark != 多次park通行
// park可能被虚假唤醒，应在循环中检查条件
```

**vs Condvar**：park/unpark更轻量但功能弱；Condvar支持条件等待和广播。

---

## 线程优先级与CPU亲和性

```rust
// 设置线程优先级 (需要 thread-priority crate)
use thread_priority::*;

// 当前线程
set_current_thread_priority(ThreadPriority::Max).unwrap();
set_current_thread_priority(ThreadPriority::Min).unwrap();

// 创建时设置
ThreadBuilder::default()
    .priority(ThreadPriority::Max)
    .spawn(|_| { /* ... */ })
    .unwrap();

// CPU亲和性绑定 (需要 affinity crate，不支持macOS)
let cores: Vec<usize> = (0..affinity::get_core_num())
    .step_by(2)  // 偶数核
    .collect();
affinity::set_thread_affinity(&cores).unwrap();
```

**CPU亲和性绑定最佳实践**：
- 将线程绑定在同一NUMA节点的核上（减少跨节点内存访问）
- 延迟敏感服务绑定专用核，避免被OS调度走
- 不要过度绑定，留出核给OS和其他进程

---

## Send 与 Sync

| Trait | 含义 | 典型类型 |
|-------|------|---------|
| `Send` | 可以安全地移动到另一个线程 | `Mutex<T>`, `Arc<T>` |
| `Sync` | 可以安全地从多个线程共享引用 | `Mutex<T>`, `AtomicI64` |
| `!Send` | 不能跨线程移动 | `Rc<T>`, `*mut T` |
| `!Sync` | 不能跨线程共享 | `Cell<T>`, `RefCell<T>` |

```rust
// Rc 不是 Send，无法直接跨线程
// 方案1：改用 Arc
let arc = Arc::new(42);

// 方案2：用 send_wrapper 临时包裹（谨慎使用）
use send_wrapper::SendWrapper;
let wrapped = SendWrapper::new(Rc::new(42));
// 注意：只在原线程访问时才安全！
```

---

## 线程控制与取消

```rust
// 通过channel发送停止信号
let (stop_tx, stop_rx) = mpsc::channel();

let handle = thread::spawn(move || {
    loop {
        match stop_rx.try_recv() {
            Ok(_) | Err(TryRecvError::Disconnected) => break,
            Err(TryRecvError::Empty) => {}
        }
        // 执行工作...
    }
});

stop_tx.send(()).unwrap();
handle.join().unwrap();

// 或使用 thread-control crate
use thread_control::*;
let (flag, control) = make_pair();
let handle = thread::spawn(move || {
    while flag.alive() {
        // 执行工作...
    }
});
control.stop();
handle.join().unwrap();
```

---

## Panic 处理

```rust
// join返回Err如果线程panic
let h = thread::spawn(|| {
    panic!("boom");
});
match h.join() {
    Ok(v)  => println!("success: {:?}", v),
    Err(e) => println!("thread panicked: {:?}", e),
}

// 在线程内捕获panic（不传播给join）
let h = thread::spawn(|| {
    let result = std::panic::catch_unwind(|| {
        panic!("boom");
    });
    println!("caught: {}", result.is_err());
    // join()将返回Ok，因为panic被捕获了
});

// std::thread::scope：任何子线程panic且未捕获，scope会在调用线程重新panic（不是返回Err）
// crossbeam::thread::scope 则返回 Result，Err包含panic payload
thread::scope(|s| {
    s.spawn(|| panic!("oh no"));
}); // 此处调用线程会panic（不是返回Err）
```

**生产建议**：
- 始终检查`join()`的结果，不要盲目`.unwrap()`
- 使用有意义的线程名（`Builder::name`），方便panic时定位
- 关键业务线程panic时应有监控告警和重启机制
