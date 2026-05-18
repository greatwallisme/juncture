# Rayon 并行编程库

对应书中第12章。Rayon 是 Rust 中数据并行的首选库，自动将工作分配到线程池。

## 核心理念

Rayon 通过**并行迭代器**将顺序代码零改动地并行化，内部使用工作窃取调度。

## 并行集合 — par_iter

几乎所有标准集合都支持 `par_iter()` / `par_iter_mut()` / `into_par_iter()`：

```rust
use rayon::prelude::*;

let numbers: Vec<i32> = (0..1_000_000).collect();

// 并行求和（vs 顺序 .iter().sum()）
let sum: i32 = numbers.par_iter().sum();

// 并行 map + filter
let result: Vec<i32> = numbers.par_iter()
    .filter(|&&x| x % 2 == 0)
    .map(|&x| x * x)
    .collect();

// 并行 sort
let mut data = vec![3, 1, 4, 1, 5, 9, 2, 6];
data.par_sort();

// 并行 for_each
numbers.par_iter().for_each(|&x| {
    // 每个元素在独立线程中处理
    process(x);
});
```

**当数据量小时 rayon 不一定更快**（线程开销），建议在 > 10,000 元素时使用。

## scope — 动态并行任务

`rayon::scope` 允许在闭包内动态 spawn 任务，任务可借用外部栈变量：

```rust
use rayon::scope;

let data = vec![1, 2, 3, 4, 5];
let mut results = vec![0; 5];

scope(|s| {
    for (i, val) in data.iter().enumerate() {
        let result = &mut results[i];
        s.spawn(move |_| {
            *result = val * 2;  // 可借用外部变量
        });
    }
    // scope 结束时所有任务已完成
});

println!("{:?}", results);  // [2, 4, 6, 8, 10]
```

**注意**：`scope` 内的 panic 会被传播到调用方。

## 线程池 — ThreadPool

自定义线程池（默认是全局池，线程数 = CPU核心数）：

```rust
use rayon::ThreadPoolBuilder;

let pool = ThreadPoolBuilder::new()
    .num_threads(4)
    .build()
    .unwrap();

// 在自定义池中执行工作
pool.install(|| {
    let sum: i32 = (0..100).into_par_iter().sum();
    println!("sum = {}", sum);
});

// 全局池并行度
rayon::current_num_threads();  // CPU 核心数
```

## join — fork-join 并行

`rayon::join` 并行执行两个闭包（fork-join 模型）：

```rust
use rayon::join;

fn parallel_sum(data: &[i32]) -> i32 {
    if data.len() <= 1000 {
        return data.iter().sum();  // 顺序处理小数据
    }
    let mid = data.len() / 2;
    let (left, right) = data.split_at(mid);
    
    let (left_sum, right_sum) = join(
        || parallel_sum(left),
        || parallel_sum(right),
    );
    left_sum + right_sum
}
```

## 并行 vs 顺序的选择

| 场景 | 推荐 |
|------|------|
| 大量独立元素处理 | `par_iter()` |
| 递归分治算法 | `rayon::join` |
| 动态任务，借用外部数据 | `rayon::scope` |
| CPU密集型，需要控制线程数 | 自定义 `ThreadPool` |
| 元素 < 1000 | 顺序迭代（无需 rayon） |

## scoped thread（rayon）

```rust
use rayon::scope;

let local_data = vec![1, 2, 3];
scope(|s| {
    s.spawn(|_| println!("{:?}", local_data));  // 借用外部变量
});
```
