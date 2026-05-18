# 内存序模型详解

## 当使用此专题

- 理解和选择正确的内存序
- 实现高性能无锁算法
- 处理跨CPU核心的内存可见性
- 优化原子操作性能


## 5种内存序模型详解

### 1. Relaxed (最弱保证)
**使用场景**: 只需要原子性，不需要顺序保证

```rust
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

// 只保证原子递增，不保证顺序
COUNTER.fetch_add(1, Ordering::Relaxed);

// 获取最新值，不保证与之前操作的顺序
let value = COUNTER.load(Ordering::Relaxed);
```

**性能**: 最快，通常1个CPU周期

### 2. Acquire (获取语义)
**使用场景**: 读取操作，确保后续读写不会被重排到当前操作之前

```rust
static DATA_READY: AtomicBool = AtomicBool::new(false);
static DATA: AtomicU64 = AtomicU64::new(0);

// 生产者
DATA.store(42, Ordering::Relaxed);
DATA_READY.store(true, Ordering::Release);

// 消费者
if DATA_READY.load(Ordering::Acquire) {
    // 保证能看到DATA的完整更新
    let data = DATA.load(Ordering::Relaxed);
    assert_eq!(data, 42);
}
```

### 3. Release (释放语义)
**使用场景**: 写入操作，确保之前的所有写操作在当前操作之前完成

```rust
static DATA: AtomicU64 = AtomicU64::new(0);
static READY: AtomicBool = AtomicBool::new(false);

// 写入数据，使用Release确保数据写入完成
DATA.store(42, Ordering::Relaxed);
READY.store(true, Ordering::Release); // Release语义
```

### 4. AcqRel (获取-释放语义)
**使用场景**: 读-改-写操作，同时需要Acquire和Release保证

```rust
static FLAG: AtomicBool = AtomicBool::new(false);

// 原子的读-改-写操作
FLAG.compare_exchange_weak(
    false,              // 期望值
    true,               // 新值
    Ordering::AcqRel,   // 失败时用Acquire
    Ordering::Relaxed,  // 成功时用Release
);
```

### 5. SeqCst (顺序一致性)
**使用场景**: 需要全局一致的内存序

```rust
use std::sync::atomic::{AtomicI64, Ordering};

static GLOBAL_COUNTER: AtomicI64 = AtomicI64::new(0);

// 保证所有线程看到相同的操作顺序
GLOBAL_COUNTER.fetch_add(1, Ordering::SeqCst);
```

## 🔧 实际应用模式

### 生产者-消费者模式
```rust
use std::sync::atomic::{AtomicPtr, AtomicBool, Ordering};
use std::ptr;

struct MPSCQueue<T> {
    head: AtomicPtr<Node<T>>,
    tail: AtomicPtr<Node<T>>,
    ready: AtomicBool,
}

impl<T> MPSCQueue<T> {
    pub fn enqueue(&self, data: T) {
        let new_node = Box::into_raw(Box::new(Node {
            data: Some(data),
            next: ptr::null_mut(),
        }));

        loop {
            let tail = self.tail.load(Ordering::Acquire);

            unsafe {
                if (*tail).next.load(Ordering::Relaxed).is_null() {
                    if (*tail).next.compare_exchange_weak(
                        ptr::null_mut(),
                        new_node,
                        Ordering::Release,  // 发布新节点
                        Ordering::Relaxed,
                    ).is_ok() {
                        self.tail.store(new_node, Ordering::Release);
                        break;
                    }
                } else {
                    // 帮助推进tail
                    let next = (*tail).next.load(Ordering::Relaxed);
                    self.tail.compare_exchange(
                        tail,
                        next,
                        Ordering::Release,
                        Ordering::Relaxed
                    );
                }
            }
        }
    }
}
```

### 状态机实现
```rust
use std::sync::atomic::{AtomicU8, Ordering};

#[repr(u8)]
enum State {
    Idle = 0,
    Starting = 1,
    Running = 2,
    Stopping = 3,
    Stopped = 4,
}

struct StateMachine {
    state: AtomicU8,
}

impl StateMachine {
    pub fn transition_to(&self, new_state: State) -> Result<(), State> {
        self.state.compare_exchange_weak(
            new_state as u8 - 1,  // 期望的前一个状态
            new_state as u8,
            Ordering::AcqRel,
            Ordering::Relaxed,
        ).map_err(|current| unsafe { std::mem::transmute(current) })
    }
}
```

## 性能优化指南

### x86_64 架构优化
```rust
// 在x86_64上，Relaxed通常足够，因为强内存模型
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

// x86_64: Relaxed + compiler barrier 足够达到SeqCst效果
COUNTER.fetch_add(1, Ordering::Relaxed);
compiler_fence(Ordering::Release);  // 确保编译器不重排
```

### ARM 架构优化
```rust
// 在ARM上，需要更谨慎的内存序选择
static FLAG: AtomicBool = AtomicBool::new(false);

// ARM: 必须使用Acquire/Release确保内存可见性
if FLAG.load(Ordering::Acquire) {
    // 确保能看到之前的所有写入
    process_data();
}

// 写入时使用Release
FLAG.store(true, Ordering::Release);
```

## 性能基准

基于书中测试结果：
- **Relaxed**: 1 CPU周期
- **Acquire/Release**: 2-3 CPU周期
- **AcqRel**: 3-4 CPU周期
- **SeqCst**: 5-10 CPU周期

## 常见陷阱

### 1. 过度使用SeqCst
```rust
// 不必要的使用SeqCst
static COUNTER: AtomicU64 = AtomicU64::new(0);
COUNTER.fetch_add(1, Ordering::SeqCst); // 过度保证

// 使用Relaxed就足够
COUNTER.fetch_add(1, Ordering::Relaxed);
```

### 2. 忽略架构差异
```rust
// 跨平台使用相同策略
FLAG.store(true, Ordering::Relaxed); // ARM上可能有问题

// 考虑架构差异
#[cfg(target_arch = "x86_64")]
FLAG.store(true, Ordering::Relaxed);

#[cfg(target_arch = "arm")]
FLAG.store(true, Ordering::Release);
```

### 3. Acquire/Release不匹配
```rust
// Release没有对应的Acquire
DATA.store(value, Ordering::Release);
let data = DATA.load(Ordering::Relaxed); // 可能看不到完整更新

// Release必须有对应的Acquire
DATA.store(value, Ordering::Release);
let data = DATA.load(Ordering::Acquire); // 保证看到完整更新
```

## 相关模板
- `../templates/atomic/versioned-pointer.rs` - 版本化指针模式

## 相关专题
- `../atomic/aba-problem.md` - ABA问题与解决方案