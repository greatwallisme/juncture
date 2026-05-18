# 容器同步原语（Cell/RefCell/OnceCell/LazyLock/Cow）

本章对应书中第4章"容器同步原语"。这些类型提供内部可变性或延迟初始化，
是理解 Arc/Mutex 与 Rc/RefCell 选择边界的基础。

## 速查：单线程 vs 多线程

| 类型 | 线程安全 | 用途 |
|------|----------|------|
| `Cell<T>` | ❌ 单线程 | Copy 类型内部可变 |
| `RefCell<T>` | ❌ 单线程 | 运行时借用检查 |
| `OnceCell<T>` | ❌ 单线程 | 单线程单次初始化 |
| `LazyCell<T>` | ❌ 单线程 | 延迟初始化 |
| `Arc<T>` | ✅ 多线程 | 共享所有权 |
| `Mutex<T>` | ✅ 多线程 | 互斥可变 |
| `sync::OnceLock<T>` | ✅ 多线程 | 多线程单次初始化 |
| `sync::LazyLock<T>` | ✅ 多线程 | 多线程延迟初始化 |
| `Rc<T>` | ❌ 单线程 | 单线程引用计数 |
| `Cow<'a, B>` | — | 写时克隆 |

## Cell\<T\> — Copy 类型内部可变

只能存储实现 Copy 的类型，**无运行时开销**：

```rust
use std::cell::Cell;

let c = Cell::new(5i32);
c.set(10);
println!("{}", c.get());  // 10
```

**不能共享引用**：只能通过 `get()`/`set()` 访问，不能获取内部引用。

## RefCell\<T\> — 运行时借用检查

允许在不可变引用上进行可变借用，但**运行时panic**（vs 编译时）：

```rust
use std::cell::RefCell;

let data = RefCell::new(vec![1, 2, 3]);

// 不可变借用
let r = data.borrow();
println!("{:?}", *r);
drop(r);

// 可变借用
data.borrow_mut().push(4);

// 注意：同时存在可变和不可变借用会 panic！
```

**与 Arc 组合**（单线程共享可变状态）：
```rust
use std::rc::Rc;
use std::cell::RefCell;

let shared = Rc::new(RefCell::new(0));
let clone = Rc::clone(&shared);
*clone.borrow_mut() += 1;
```
> ⚠️ `Arc<RefCell<T>>` 不是线程安全的，多线程请用 `Arc<Mutex<T>>`

## OnceCell\<T\> — 单次初始化（单线程）

```rust
use std::cell::OnceCell;

let cell: OnceCell<String> = OnceCell::new();

// 第一次调用初始化
let value = cell.get_or_init(|| "hello".to_string());
println!("{}", value);  // hello

// 之后直接返回已有值
let value2 = cell.get_or_init(|| "world".to_string());
println!("{}", value2);  // 仍是 hello
```

**多线程版本**：`std::sync::OnceLock<T>`（Rust 1.70+）

```rust
use std::sync::OnceLock;

static GLOBAL: OnceLock<String> = OnceLock::new();

fn get_value() -> &'static String {
    GLOBAL.get_or_init(|| "initialized once".to_string())
}
```

## LazyCell\<T\> 和 LazyLock\<T\> — 延迟初始化

`LazyCell`（单线程）和 `LazyLock`（多线程，Rust 1.80+）提供更便捷的延迟初始化：

```rust
use std::cell::LazyCell;
use std::sync::LazyLock;

// 单线程
let lazy: LazyCell<i32> = LazyCell::new(|| 42);
println!("{}", *lazy);  // 首次访问时初始化

// 多线程全局变量（替代 lazy_static!）
static CONFIG: LazyLock<String> = LazyLock::new(|| {
    std::env::var("CONFIG").unwrap_or_default()
});
```

> `LazyLock` 是 `lazy_static!` 宏的标准库替代，Rust 1.80+ 稳定。

## Rc\<T\> vs Arc\<T\>

```rust
use std::rc::Rc;
use std::sync::Arc;

// Rc: 单线程，无原子操作，更快
let rc = Rc::new(5);
let rc2 = Rc::clone(&rc);  // 引用计数+1（非原子）

// Arc: 多线程，原子引用计数
let arc = Arc::new(5);
let arc2 = Arc::clone(&arc);  // 引用计数+1（原子操作）
```

**规则**：多线程用 Arc，单线程用 Rc（性能更好）。

## Cow\<'a, B\> — 写时克隆

`Cow`（Clone on Write）延迟克隆，只在需要修改时才实际克隆：

```rust
use std::borrow::Cow;

fn ensure_uppercase(s: &str) -> Cow<str> {
    if s.chars().all(|c| c.is_uppercase()) {
        Cow::Borrowed(s)  // 不克隆
    } else {
        Cow::Owned(s.to_uppercase())  // 才克隆
    }
}

let s = "HELLO";
let result = ensure_uppercase(s);
// result 是 Borrowed，未发生分配
```

**并发场景**：`Arc<Cow<'_, T>>` 可在线程间共享，只在写时克隆。
