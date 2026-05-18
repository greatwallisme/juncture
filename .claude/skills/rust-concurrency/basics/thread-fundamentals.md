# 线程专家提示与陷阱

## Panic传播行为

**关键**: 子线程panic不会自动传播到父线程,必须通过`join()`检查。

```rust
use std::thread;

// 错误: 忽略join结果,panic被静默丢弃
thread::spawn(|| {
    panic!("Critical error!");
});
// 如果不join,panic被忽略

// 正确: 检查panic
let handle = thread::spawn(|| {
    panic!("Something failed");
});
match handle.join() {
    Ok(_) => {},
    Err(payload) => {
        // payload是Any类型,可downcast获取panic消息
        if let Some(msg) = payload.downcast_ref::<&str>() {
            eprintln!("Thread panicked: {}", msg);
        }
    }
}
```

## JoinHandle错误处理模式

```rust
use std::thread::{self, JoinHandle};

/// 批量join时收集所有错误而非快速失败
fn join_all_handles(handles: Vec<JoinHandle<Result<(), String>>>) -> Vec<Result<(), String>> {
    handles.into_iter()
        .map(|h| h.join().unwrap_or(Err("Thread panicked".to_string()))) // unwrap_or处理join本身panic
        .collect()
}
```

## 线程命名技巧

```rust
// 调试时命名线程可追踪日志
thread::Builder::new()
    .name("db-writer-1".to_string())
    .spawn(|| {
        // 日志中会显示"db-writer-1"而非ThreadId
        println!("Writing data in {:?}", thread::current().name());
    });
```

## 栈大小调整决策标准

```rust
// 默认栈大小因平台而异(通常2-8MB)
// 仅在以下场景调整:

// 1. 深递归: 增大栈
thread::Builder::new()
    .stack_size(8 * 1024 * 1024) // 8MB
    .spawn(|| deep_recursive_algorithm());

// 2. 大量短命线程: 减小栈(降低内存占用)
thread::Builder::new()
    .stack_size(256 * 1024) // 256KB
    .spawn(|| simple_task_no_recursion());
```

## 线程边界闭包陷阱

```rust
// 错误: 闭包捕获引用导致编译失败
let data = vec![1, 2, 3];
thread::spawn(|| {
    println!("{:?}", data); // 编译错误: data可能在线程执行前被drop
});

// 正确: 使用move转移所有权
let data = vec![1, 2, 3];
thread::spawn(move || {
    println!("{:?}", data); // data所有权转移至新线程
});
```

## 作用域线程模式

```rust
// 确保所有线程在作用域结束前完成
use crossbeam::scope;

scope(|s| {
    s.spawn(|_| {
        // 可以借用外部数据,因为scope保证线程在作用域内结束
        println!("Thread running");
    });
}); // 这里会阻塞直到所有spawn的线程完成
```

## 相关专题

- `../sync/mutex-rwlock.md` - 锁的正确使用
- `../threading/work-stealing.md` - 线程池设计
- `../debugging/concurrent-bugs.md` - 调试并发问题
