# 互斥锁和读写锁

## 高级同步模式

### 嵌套锁和锁顺序

**MANDATORY**: 正确的锁顺序是避免死锁的关键

```rust
use std::sync::{Arc, Mutex};

/// 正确的锁顺序避免死锁
struct Database {
    users: Mutex<Vec<String>>,
    posts: Mutex<Vec<String>>,
}

impl Database {
    fn new() -> Self {
        Self {
            users: Mutex::new(Vec::new()),
            posts: Mutex::new(Vec::new()),
        }
    }

    /// 按固定顺序获取锁,避免死锁
    fn add_user_and_post(&self, user: String, post: String) {
        // 总是先获取users锁,再获取posts锁
        let mut users = self.users.lock().unwrap();
        let mut posts = self.posts.lock().unwrap();

        users.push(user);
        posts.push(post);
    }

    /// 错误示例:可能导致死锁
    #[allow(dead_code)]
    fn add_user_and_post_wrong(&self, user: String, post: String) {
        let mut posts = self.posts.lock().unwrap();
        let mut users = self.users.lock().unwrap(); // 与其他方法顺序相反

        users.push(user);
        posts.push(post);
    }
}

/// 死锁演示(仅用于教学)
#[allow(dead_code)]
fn deadlock_demo() {
    use std::thread;
    use std::time::Duration;

    let data1 = Arc::new(Mutex::new(0));
    let data2 = Arc::new(Mutex::new(0));

    let data1_clone = Arc::clone(&data1);
    let data2_clone = Arc::clone(&data2);

    let handle1 = thread::spawn(move || {
        let _lock1 = data1_clone.lock().unwrap();
        thread::sleep(Duration::from_millis(100));
        let _lock2 = data2_clone.lock().unwrap(); // 可能死锁
    });

    let handle2 = thread::spawn(move || {
        let _lock2 = data2.lock().unwrap();
        thread::sleep(Duration::from_millis(100));
        let _lock1 = data1.lock().unwrap(); // 可能死锁
    });

    // 注意:这个例子会死锁,仅用于演示
    // 在实际使用中应该设置超时或避免这种锁顺序
}
```

### 条件变量配合

```rust
use std::sync::{Arc, Mutex, Condvar};
use std::thread;

/// 生产者-消费者使用条件变量
fn producer_consumer_condvar() {
    let queue = Arc::new(Mutex::new(Vec::new()));
    let not_empty = Arc::new(Condvar::new());
    let not_full = Arc::new(Condvar::new());
    let mut handles = vec![];

    // 消费者
    for consumer_id in 0..2 {
        let queue = Arc::clone(&queue);
        let not_empty = Arc::clone(&not_empty);
        let handle = thread::spawn(move || {
            loop {
                let mut q = queue.lock().unwrap();

                while q.is_empty() {
                    q = not_empty.wait(q).unwrap();
                }

                let item = q.remove(0);
                println!("Consumer {} consumed: {}", consumer_id, item);

                // 通知生产者有空间了
                not_full.notify_one();

                if item == 99 { // 结束信号
                    break;
                }
            }
        });
        handles.push(handle);
    }

    // 生产者
    for producer_id in 0..2 {
        let queue = Arc::clone(&queue);
        let not_full = Arc::clone(&not_full);
        let handle = thread::spawn(move || {
            for i in (0..50).map(|x| producer_id * 50 + x) {
                let mut q = queue.lock().unwrap();

                while q.len() >= 10 {
                    q = not_full.wait(q).unwrap();
                }

                q.push(i);
                println!("Producer {} produced: {}", producer_id, i);

                // 通知消费者有新数据
                not_empty.notify_one();
            }
        });
        handles.push(handle);
    }

    // 发送结束信号
    {
        let mut q = queue.lock().unwrap();
        q.push(99);
        q.push(99);
        not_empty.notify_all();
    }

    for handle in handles {
        handle.join().unwrap();
    }
}
```

## 死锁预防和检测

### 锁超时机制

```rust
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use std::thread;

/// 带超时的锁获取
fn try_lock_with_timeout<T>(mutex: &Mutex<T>, timeout: Duration) -> Result<std::sync::MutexGuard<T>, ()> {
    let start = Instant::now();

    loop {
        match mutex.try_lock() {
            Ok(guard) => return Ok(guard),
            Err(_) => {
                if start.elapsed() > timeout {
                    return Err(());
                }
                thread::sleep(Duration::from_millis(1));
            }
        }
    }
}

/// 使用超时避免死锁
fn timeout_example() {
    let mutex1 = Arc::new(Mutex::new(0));
    let mutex2 = Arc::new(Mutex::new(0));

    let m1_clone = Arc::clone(&mutex1);
    let m2_clone = Arc::clone(&mutex2);

    let handle = thread::spawn(move || {
        if let Ok(_lock1) = try_lock_with_timeout(&m1_clone, Duration::from_secs(1)) {
            if let Ok(_lock2) = try_lock_with_timeout(&m2_clone, Duration::from_secs(1)) {
                println!("Successfully acquired both locks");
            } else {
                println!("Timeout acquiring second lock");
            }
        } else {
            println!("Timeout acquiring first lock");
        }
    });

    handle.join().unwrap();
}
```

### 锁的层次结构

**MANDATORY**: 锁层次是系统化避免死锁的最佳实践

```rust
use std::sync::{Arc, Mutex};

/// 定义锁的层次,避免循环等待
const LOCK_LEVEL_DATABASE: u8 = 1;
const LOCK_LEVEL_USER: u8 = 2;
const LOCK_LEVEL_POST: u8 = 3;

struct LockLevelChecker {
    current_level: Mutex<Option<u8>>,
}

impl LockLevelChecker {
    fn new() -> Self {
        Self {
            current_level: Mutex::new(None),
        }
    }

    fn acquire_lock(&self, level: u8) -> Result<(), String> {
        let mut current = self.current_level.lock().unwrap();

        if let Some(curr_level) = *current {
            if curr_level >= level {
                return Err(format!("Lock level violation: trying to acquire level {} while holding level {}", level, curr_level));
            }
        }

        *current = Some(level);
        Ok(())
    }

    fn release_lock(&self) {
        let mut current = self.current_level.lock().unwrap();
        *current = None;
    }
}

/// 使用锁层次的安全数据库操作
struct SafeDatabase {
    lock_checker: Arc<LockLevelChecker>,
    users: Mutex<Vec<String>>,
    posts: Mutex<Vec<String>>,
}

impl SafeDatabase {
    fn new() -> Self {
        Self {
            lock_checker: Arc::new(LockLevelChecker::new()),
            users: Mutex::new(Vec::new()),
            posts: Mutex::new(Vec::new()),
        }
    }

    fn add_user(&self, user: String) -> Result<(), String> {
        self.lock_checker.acquire_lock(LOCK_LEVEL_USER)?;
        let mut users = self.users.lock().unwrap();
        users.push(user);
        self.lock_checker.release_lock();
        Ok(())
    }

    fn add_post(&self, post: String) -> Result<(), String> {
        self.lock_checker.acquire_lock(LOCK_LEVEL_POST)?;
        let mut posts = self.posts.lock().unwrap();
        posts.push(post);
        self.lock_checker.release_lock();
        Ok(())
    }
}
```

## 性能优化技巧

### 减少锁的粒度

```rust
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;
use std::thread;

/// 锁粒度优化示例
struct OptimizedCounter {
    value: AtomicU64,
    // 使用原子操作代替Mutex
}

impl OptimizedCounter {
    fn new() -> Self {
        Self {
            value: AtomicU64::new(0),
        }
    }

    fn increment(&self) -> u64 {
        self.value.fetch_add(1, Ordering::Relaxed)
    }

    fn get(&self) -> u64 {
        self.value.load(Ordering::Relaxed)
    }
}

/// 对比:使用Mutex的性能
struct MutexCounter {
    value: Mutex<u64>,
}

impl MutexCounter {
    fn new() -> Self {
        Self {
            value: Mutex::new(0),
        }
    }

    fn increment(&self) -> u64 {
        let mut value = self.value.lock().unwrap();
        *value += 1;
        *value
    }

    fn get(&self) -> u64 {
        *self.value.lock().unwrap()
    }
}

/// 性能对比测试
fn performance_comparison() {
    const ITERATIONS: usize = 1_000_000;
    const THREADS: usize = 8;

    // 测试AtomicU64版本
    let optimized = Arc::new(OptimizedCounter::new());
    let start = Instant::now();

    let mut handles = vec![];
    for _ in 0..THREADS {
        let counter = Arc::clone(&optimized);
        let handle = thread::spawn(move || {
            for _ in 0..ITERATIONS / THREADS {
                counter.increment();
            }
        });
        handles.push(handle);
    }

    for handle in handles {
        handle.join().unwrap();
    }

    let atomic_duration = start.elapsed();

    // 测试Mutex版本
    let mutex_counter = Arc::new(MutexCounter::new());
    let start = Instant::now();

    let mut handles = vec![];
    for _ in 0..THREADS {
        let counter = Arc::clone(&mutex_counter);
        let handle = thread::spawn(move || {
            for _ in 0..ITERATIONS / THREADS {
                counter.increment();
            }
        });
        handles.push(handle);
    }

    for handle in handles {
        handle.join().unwrap();
    }

    let mutex_duration = start.elapsed();

    println!("AtomicU64 version: {:?}", atomic_duration);
    println!("Mutex version: {:?}", mutex_duration);
    println!("Performance ratio: {:.2}x",
        mutex_duration.as_secs_f64() / atomic_duration.as_secs_f64());
}
```

### 读写锁使用技巧

```rust
use std::sync::{Arc, RwLock};

/// 批量操作优化
struct BatchProcessor {
    data: RwLock<Vec<i32>>,
    pending: RwLock<Vec<i32>>,
}

impl BatchProcessor {
    fn new() -> Self {
        Self {
            data: RwLock::new(Vec::new()),
            pending: RwLock::new(Vec::new()),
        }
    }

    /// 快速写入到pending区域
    fn add_item(&self, item: i32) {
        let mut pending = self.pending.write().unwrap();
        pending.push(item);
    }

    /// 批量处理,减少写锁时间
    fn process_batch(&self) {
        let mut pending = self.pending.write().unwrap();
        let mut data = self.data.write().unwrap();

        // 一次性处理所有pending数据
        data.append(&mut *pending);

        println!("Processed batch, total items: {}", data.len());
    }

    /// 快速读取
    fn read_all(&self) -> Vec<i32> {
        let data = self.data.read().unwrap();
        data.clone()
    }
}
```

## 反模式

### 锁顺序不一致导致死锁

```rust
// 错误示例:不同方法使用不同锁顺序
struct BadDatabase {
    users: Mutex<Vec<String>>,
    posts: Mutex<Vec<String>>,
}

impl BadDatabase {
    // 先users后posts
    fn method_a(&self) {
        let _users = self.users.lock().unwrap();
        let _posts = self.posts.lock().unwrap();
    }

    // 先posts后users - 与method_a相反,可能死锁!
    fn method_b(&self) {
        let _posts = self.posts.lock().unwrap();
        let _users = self.users.lock().unwrap();
    }
}

// 正确示例:所有方法使用相同锁顺序
struct GoodDatabase {
    users: Mutex<Vec<String>>,
    posts: Mutex<Vec<String>>,
}

impl GoodDatabase {
    // 始终先users后posts
    fn method_a(&self) {
        let _users = self.users.lock().unwrap();
        let _posts = self.posts.lock().unwrap();
    }

    fn method_b(&self) {
        let _users = self.users.lock().unwrap();
        let _posts = self.posts.lock().unwrap();
    }
}
```

### 持有锁时执行耗时操作

```rust
// 错误示例:持有锁时进行I/O操作
fn bad_lock_usage(data: &Mutex<Vec<i32>>) {
    let mut data = data.lock().unwrap();
    data.push(42);

    // 持有锁时进行网络请求 - 阻塞所有其他线程!
    let response = reqwest::get("https://example.com"); // 示例
    data.push(response.len() as i32);
} // 锁在这里释放

// 正确示例:缩小临界区
fn good_lock_usage(data: &Mutex<Vec<i32>>) {
    // 临界区开始
    let mut data = data.lock().unwrap();
    data.push(42);
    drop(data); // 显式释放锁
    // 临界区结束

    // 锁已释放,其他线程可以访问
    let response = reqwest::get("https://example.com"); // 示例

    // 重新获取锁
    let mut data = data.lock().unwrap();
    data.push(response.len() as i32);
}
```

### 无条件try_lock自旋

```rust
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

// 错误示例:无条件自旋浪费CPU
fn bad_spin_lock(mutex: &Arc<Mutex<u64>>) {
    loop {
        if let Ok(_) = mutex.try_lock() {
            break;
        }
        // CPU空转,浪费资源
    }
}

// 正确示例:使用条件变量或超时
fn good_wait(mutex: &Arc<Mutex<u64>>) {
    let timeout = Duration::from_secs(1);
    let start = std::time::Instant::now();

    while start.elapsed() < timeout {
        if let Ok(_) = mutex.try_lock() {
            break;
        }
        thread::yield_now(); // 让出CPU
    }
}
```

## 相关专题

- `../atomic/memory-ordering.md` - 内存序模型详解
- `../debugging/concurrent-bugs.md` - 并发Bug调试
