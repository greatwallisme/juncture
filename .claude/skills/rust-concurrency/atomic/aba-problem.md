# ABA问题及其解决方案

## ABA问题详解

### 什么是ABA问题

ABA问题发生在以下场景：
1. 线程A读取内存地址P的值为A
2. 线程B将P的值从A改为B，然后又改回A
3. 线程A进行CAS操作，误以为值没有被修改过

```rust
use std::sync::atomic::{AtomicPtr, Ordering};
use std::ptr;

struct Node<T> {
    data: T,
    next: *mut Node<T>,
}

struct Stack<T> {
    head: AtomicPtr<Node<T>>,
}

// ❌ 有ABA问题的实现
impl<T> Stack<T> {
    pub fn pop(&self) -> Option<T> {
        loop {
            let head = self.head.load(Ordering::Acquire);

            if head.is_null() {
                return None;
            }

            let next = unsafe { (*head).next };

            // 这里可能有ABA问题！
            match self.head.compare_exchange_weak(
                head,
                next,
                Ordering::Release,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    // 成功弹出
                    let node = unsafe { Box::from_raw(head) };
                    return Some(node.data);
                }
                Err(_) => continue,
            }
        }
    }

    pub fn push(&self, data: T) {
        let new_node = Box::into_raw(Box::new(Node {
            data,
            next: ptr::null_mut(),
        }));

        loop {
            let head = self.head.load(Ordering::Acquire);
            unsafe { (*new_node).next = head; }

            match self.head.compare_exchange_weak(
                head,
                new_node,
                Ordering::Release,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(_) => continue,
            }
        }
    }
}
```

## ABA问题解决方案

### 1. 版本化指针 (Tagged Pointer)

```rust
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug, Clone, Copy)]
struct TaggedPtr {
    ptr_and_tag: u64,  // 低位是指针，高位是版本号
}

impl TaggedPtr {
    fn new(ptr: *mut u8, tag: u64) -> Self {
        // 假设指针是8字节对齐的，低3位为0
        assert_eq!(ptr as u64 & 0b111, 0);
        Self {
            ptr_and_tag: ptr as u64 | (tag << 48),
        }
    }

    fn ptr(&self) -> *mut u8 {
        (self.ptr_and_tag & 0x0000FFFFFFFFFFFF) as *mut u8
    }

    fn tag(&self) -> u64 {
        self.ptr_and_tag >> 48
    }

    fn with_incremented_tag(&self) -> Self {
        Self::new(self.ptr(), self.tag() + 1)
    }
}

struct VersionedStack<T> {
    head: AtomicU64,  // 存储TaggedPtr的原始值
}

impl<T> VersionedStack<T> {
    pub fn pop(&self) -> Option<T> {
        loop {
            let current = self.head.load(Ordering::Acquire);
            let tagged = TaggedPtr { ptr_and_tag: current };

            let head_ptr = tagged.ptr() as *mut Node<T>;
            if head_ptr.is_null() {
                return None;
            }

            let next_ptr = unsafe { (*head_ptr).next };
            let next_tagged = TaggedPtr::new(next_ptr, tagged.tag());
            let next_value = next_tagged.with_incremented_tag();

            match self.head.compare_exchange_weak(
                current,
                next_value.ptr_and_tag,
                Ordering::Release,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    let node = unsafe { Box::from_raw(head_ptr) };
                    return Some(node.data);
                }
                Err(_) => continue,
            }
        }
    }
}
```

### 2. Hazard Pointer

```rust
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::{Arc, Mutex};
use std::collections::HashSet;
use std::ptr;

thread_local! {
    static HAZARD_POINTERS: [AtomicPtr<()>; 2] = [
        AtomicPtr::new(ptr::null_mut()),
        AtomicPtr::new(ptr::null_mut()),
    ];
}

struct HazardPointerManager {
    retired: Arc<Mutex<Vec<RetiredNode>>>,
    max_retired: usize,
}

struct RetiredNode {
    ptr: *mut u8,
    // 其他元数据...
}

impl HazardPointerManager {
    pub fn new(max_retired: usize) -> Self {
        Self {
            retired: Arc::new(Mutex::new(Vec::new())),
            max_retired,
        }
    }

    pub fn protect<F, R>(&self, idx: usize, f: F) -> R
    where
        F: FnOnce(*mut u8) -> R,
    {
        HAZARD_POINTERS[idx].store(ptr::null_mut(), Ordering::SeqCst);

        let result = f(HAZARD_POINTERS[idx].load(Ordering::SeqCst) as *mut u8);

        // 清除hazard pointer
        HAZARD_POINTERS[idx].store(ptr::null_mut(), Ordering::SeqCst);

        result
    }

    pub fn retire(&self, ptr: *mut u8) {
        let mut retired = self.retired.lock().unwrap();
        retired.push(RetiredNode { ptr });

        // 如果达到了阈值，尝试回收
        if retired.len() >= self.max_retired {
            self.reclaim(&mut retired);
        }
    }

    fn reclaim(&self, retired: &mut Vec<RetiredNode>) {
        // 收集所有活跃的hazard pointers
        let mut active = HashSet::new();
        for hp in &HAZARD_POINTERS {
            let ptr = hp.load(Ordering::SeqCst);
            if !ptr.is_null() {
                active.insert(ptr as *mut u8);
            }
        }

        // 只回收不在active set中的节点
        retired.retain(|node| {
            if active.contains(&node.ptr) {
                true  // 仍然被引用，保留
            } else {
                // 安全回收
                unsafe {
                    // 这里应该调用适当的析构函数
                    std::alloc::dealloc(node.ptr, std::alloc::Layout::new::<u8>());
                }
                false  // 已回收，删除
            }
        });
    }
}
```

### 3. Epoch-Based Reclamation

```rust
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::collections::VecDeque;

struct EpochManager {
    global_epoch: AtomicU64,
    local_epochs: Vec<AtomicU64>,  // 每个线程一个
    retired: Arc<Mutex<VecDeque<RetiredVec>>>,
}

struct RetiredVec {
    epoch: u64,
    objects: Vec<*mut u8>,
}

impl EpochManager {
    pub fn new(num_threads: usize) -> Self {
        let mut local_epochs = Vec::new();
        for _ in 0..num_threads {
            local_epochs.push(AtomicU64::new(0));
        }

        Self {
            global_epoch: AtomicU64::new(0),
            local_epochs,
            retired: Arc::new(Mutex::new(VecDeque::new())),
        }
    }

    pub fn enter_critical_section(&self, thread_id: usize) -> u64 {
        let current_epoch = self.global_epoch.load(Ordering::Acquire);
        self.local_epochs[thread_id].store(current_epoch, Ordering::Release);
        current_epoch
    }

    pub fn exit_critical_section(&self, thread_id: usize) {
        self.local_epochs[thread_id].store(u64::MAX, Ordering::Release);
    }

    pub fn retire(&self, ptr: *mut u8, thread_id: usize) {
        let current_epoch = self.local_epochs[thread_id].load(Ordering::Acquire);

        let mut retired = self.retired.lock().unwrap();

        // 找到或创建对应epoch的retired vec
        for retired_vec in retired.iter_mut() {
            if retired_vec.epoch == current_epoch {
                retired_vec.objects.push(ptr);
                return;
            }
        }

        // 创建新的epoch entry
        retired.push_back(RetiredVec {
            epoch: current_epoch,
            objects: vec![ptr],
        });

        // 尝试推进全局epoch
        self.try_advance_epoch(&mut retired);
    }

    fn try_advance_epoch(&self, retired: &mut VecDeque<RetiredVec>) {
        let current_epoch = self.global_epoch.load(Ordering::Acquire);

        // 检查是否所有线程都离开了当前epoch
        for local_epoch in &self.local_epochs {
            let epoch = local_epoch.load(Ordering::Acquire);
            if epoch != u64::MAX && epoch == current_epoch {
                return;  // 还有线程在当前epoch，不能推进
            }
        }

        // 所有线程都离开了，可以推进epoch
        let new_epoch = current_epoch + 1;
        if self.global_epoch.compare_exchange_weak(
            current_epoch,
            new_epoch,
            Ordering::SeqCst,
            Ordering::Relaxed,
        ).is_ok() {
            // 回收两个epoch前的对象
            while let Some(front) = retired.front() {
                if front.epoch < new_epoch - 1 {
                    let retired_vec = retired.pop_front().unwrap();
                    for ptr in retired_vec.objects {
                        unsafe {
                            std::alloc::dealloc(ptr, std::alloc::Layout::new::<u8>());
                        }
                    }
                } else {
                    break;
                }
            }
        }
    }
}
```

## 性能对比

| 方案 | 内存开销 | CPU开销 | 复杂度 | 适用场景 |
|------|----------|---------|--------|----------|
| 无保护 | 0 | 最低 | 简单 | 短期运行，指针不复用 |
| 版本化指针 | 8字节 | 低 | 中等 | 指针空间充足 |
| Hazard Pointer | 每线程8-16字节 | 中等 | 高 | 长期运行，高并发 |
| Epoch-Based | 每线程8字节 | 中等 | 中等 | 批量操作，定期回收 |

## 实现模板

### 版本化无锁队列
- `../templates/atomic/versioned-pointer.rs` - 完整实现

### Hazard Pointer栈
- `../templates/atomic/hazard-pointer-stack.rs` - 防护实现

### Epoch-Based容器
- `../templates/atomic/epoch-based-container.rs` - EBR实现

## 最佳实践

### 1. 选择合适的防护策略
```rust
// 短期运行，低并发 - 使用版本化指针
if lifetime < Duration::from_secs(10) && threads <= 4 {
    return VersionedProtection::new();
}

// 长期运行，高并发 - 使用Hazard Pointer
if lifetime > Duration::from_hours(1) || threads > 16 {
    return HazardPointerProtection::new();
}

// 批量操作 - 使用Epoch-Based
if batch_size > 1000 {
    return EpochBasedProtection::new();
}
```

### 2. 性能测试
```rust
// 性能基准测试
#[bench]
fn bench_stack_operations(b: &mut Bencher) {
    let stack = VersionedStack::new();

    b.iter(|| {
        // 压入元素
        for i in 0..1000 {
            stack.push(i);
        }

        // 弹出所有元素
        while let Some(_) = stack.pop() {}
    });
}
```

## 🔗 相关专题

- `../atomic/memory-ordering.md` - 内存序模型详解
- `../datastucutres/lockfree-queue.md` - 无锁队列