// EXPERT NOTE: Hexagonal architecture adds indirection layers that impact performance.
// Before optimizing, MEASURE. Most performance bottlenecks are NOT where you expect.

## Profiling Workflow

```
1. Establish baseline (current performance)
2. Identify hotspot (where time is actually spent)
3. Optimize (make targeted changes)
4. Verify (measure again, confirm improvement)
5. Document (why optimization was needed)
```

## Flame Graphs: See Where CPU Time Goes

```bash
# Install flamegraph
cargo install flamegraph

# Generate flamegraph for a specific benchmark
cargo flamegraph --bin your-app --bench author_benchmark

# Result: flamegraph.svg in target/criterion/
```

**EXPERT NOTE**: Flamegraphs show stack trace frequency over time.
- Wide bars = more time spent
- Y-axis = call stack depth
- Look for unexpected wide areas (optimization targets)

### Reading Flamegraphs

```
Example interpretation:
├─ tokio runtime (40%)     # Expected for async app
├─ db query pool (30%)     # I/O bound, normal
├─ serde_json::from_str (15%)  # Potential target: consider faster parser
├─ domain validation (10%)     # Only optimize if >10K req/sec
└─ dynamic dispatch (5%)      # Negligible, ignore
```

## Criterion Benchmarks: Measure Specific Functions

```toml
# Cargo.toml
[dev-dependencies]
criterion = "0.5"

[[bench]]
name = "author_benchmark"
harness = false
```

```rust
// benches/author_benchmark.rs
use criterion::{black_box, criterion_group, criterion_main, Criterion, BenchmarkId};
use your_app::domain::models::*;
use your_app::domain::services::AuthorServiceImpl;

fn bench_author_creation(c: &mut Criterion) {
    let mut group = c.benchmark_group("author_creation");

    // Benchmark with different input sizes
    for name_len in [10, 50, 100].iter() {
        group.bench_with_input(
            BenchmarkId::from_parameter(name_len),
            name_len,
            |b, &len| {
                let name = "x".repeat(len);
                b.iter(|| {
                    AuthorName::new(black_box(name.clone())).unwrap()
                });
            },
        );
    }

    group.finish();
}

fn bench_service_create_author(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let service = setup_test_service(&rt);

    c.bench_function("service_create_author", |b| {
        b.to_async(&rt).iter(|| {
            service.create_author(black_box(&test_request()));
        });
    });
}

criterion_group!(benches, bench_author_creation, bench_service_create_author);
criterion_main!(benches);
```

```bash
# Run benchmarks
cargo bench

# Generate report (target/criterion/report/index.html)
open target/criterion/report/index.html
```

## Tracing: Production Performance Insights

```rust
// main.rs
use tracing::{info, Level};
use tracing_subscriber::{fmt, prelude::*};

#[tokio::main]
async fn main() -> Result<()> {
    // Setup tracing with performance instrumentation
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(tracing_tracy::TracyLayer::new())  // For detailed profiling
        .init();

    // Your app code...
    info!("Application started");
}
```

```toml
# Cargo.toml
[dependencies]
tracing = "0.1"
tracing-subscriber = "0.3"

# Optional: For Tracy profiler integration
tracing-tracy = "0.10"
```

### Using Tracy for Detailed Profiling

```bash
# Install Tracy
cargo install tracy

# Run your app with Tracy client
cargo run

# Open Tracy (connects automatically)
tracy
```

**EXPERT NOTE**: Tracy shows real-time metrics including:
- CPU time per function
- Memory allocations
- Lock contention
- Frame timing

## Memory Profiling

### Using dhat (Heap Profiling)

```bash
# Install dhat
cargo install dhat

# Add to Cargo.toml
[dependencies]
dhat = "0.3"

# Run with dhat
DHAT=yes cargo run
```

```rust
#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

// Code to profile
fn main() {
    let _profiler = dhat::Profiler::new_heap();
    // Your code here...
    // Profiler drops here, generates report
}
```

### Common Memory Issues in Hexagonal Architecture

| Issue | Symptom | Fix |
|-------|---------|-----|
| Arc cycles | Memory never freed | Use Weak references |
| Large cloned strings | High allocation | Use Arc<str> for shared data |
| Unbounded Vecs | OOM over time | Add limits, paginate |
| Retained large structs | Memory bloat | Drop explicitly when done |

## Database Query Profiling

### SQLite Query Analysis

```sql
-- Enable query logging
PRAGMA journal_mode = WAL;

-- Check query plan
EXPLAIN QUERY PLAN SELECT * FROM authors WHERE email = 'test@example.com';

-- Should see: SEARCH authors USING INDEX idx_authors_email
-- If you see: SCAN TABLE authors -> missing index!
```

### Add Missing Indexes

```sql
-- Create indexes for frequently queried columns
CREATE INDEX idx_authors_email ON authors(email);
CREATE INDEX idx_posts_author_id ON posts(author_id);
CREATE INDEX idx_posts_created_at ON posts(created_at DESC);
```

**EXPERT NOTE**: Index trade-offs:
- Pros: Faster reads (10-1000x for large tables)
- Cons: Slower writes, more storage
- Rule: Index columns used in WHERE, JOIN, ORDER BY

### Connection Pool Tuning

```rust
// outbound/sqlite.rs
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

impl Sqlite {
    pub async fn new(database_url: &str) -> Result<Self> {
        let opts = SqliteConnectOptions::from_str(database_url)?
            .create_if_missing(true);

        let pool = SqlitePoolOptions::new()
            .max_connections(5)  // SQLite: 5-10 is usually enough
            .min_connections(1)  // Keep 1 connection warm
            .acquire_timeout(std::time::Duration::from_secs(30))
            .idle_timeout(std::time::Duration::from_secs(600))
            .max_lifetime(std::time::Duration::from_secs(1800))
            .connect_with(opts)
            .await?;

        Ok(Self { pool })
    }
}
```

## Dynamic Dispatch Overhead

### Measuring Trait Call Cost

```rust
#[cfg(test)]
mod bench {
    use super::*;
    use criterion::*;

    // Dynamic dispatch (Arc<dyn Trait>)
    fn bench_dynamic(c: &mut Criterion) {
        let service: Arc<dyn AuthorService> = Arc::new(test_service());
        c.bench_function("dynamic_dispatch", |b| {
            b.iter(|| service.get_author(black_box(&test_id())));
        });
    }

    // Static dispatch (concrete type)
    fn bench_static(c: &mut Criterion) {
        let service: AuthorServiceImpl<MockRepo, ...> = test_service_concrete();
        c.bench_function("static_dispatch", |b| {
            b.iter(|| service.get_author(black_box(&test_id())));
        });
    }
}
```

**EXPERT NOTE**: Dynamic dispatch costs ~5-10ns per call.
- For I/O-bound operations (DB, HTTP): Negligible (<0.01% of total)
- For CPU-bound hot loops: Consider generics
- Profile before optimizing!

## Async Runtime Profiling

### Tokio Console

```toml
[dependencies]
console-subscriber = "0.1"
```

```rust
#[tokio::main]
async fn main() -> Result<()> {
    // Enable console subscriber
    console_subscriber::init();

    // Your app code...
}
```

```bash
# Run tokio-console
tokio-console

# View in browser
# http://localhost:6669
```

**Shows**:
- Task spawning rate
- Task poll duration
- Scheduler behavior
- Resource utilization

## Optimization Checklist

Before optimizing, verify:

```
[ ] Have I profiled? (Don't guess, measure)
[ ] Is this actually a bottleneck? (Focus on top 3 hotspots)
[ ] What's the baseline? (Record current numbers)
[ ] Will optimization matter for users? (100ns -> 10ns is pointless for HTTP APIs)
[ ] Have I considered trade-offs? (Code complexity vs performance)
```

## Common Hexagonal Architecture Bottlenecks

| Layer | Common Issue | Severity | Fix |
|-------|--------------|----------|-----|
| Domain | Over-validation | Low | Cache validation results |
| Service | Excessive cloning | Medium | Use Arc for shared data |
| Repository | N+1 queries | High | Use JOIN or batch queries |
| HTTP | JSON parsing | Low | Consider simd-json |
| All | Excessive Arc cloning | Medium | Use references where safe |

## When NOT to Optimize

```
Don't optimize if:
- Request takes <100ms total (user won't notice)
- Hotspot is I/O bound (DB/network is the bottleneck)
- Optimization adds significant complexity
- You haven't measured (premature optimization is the root of all evil)
```

## Performance Budgets

Set budgets before optimizing:

```rust
// Example: API response time budget
const MAX_DB_TIME: Duration = Duration::from_millis(50);
const MAX_SERIALIZATION_TIME: Duration = Duration::from_millis(10);
const MAX_TOTAL_TIME: Duration = Duration::from_millis(100);

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn get_author_meets_budget() {
        let start = Instant::now();
        let result = service.get_author(&id).await;
        let elapsed = start.elapsed();

        assert!(elapsed < MAX_TOTAL_TIME, "Exceeded budget: {:?}", elapsed);
    }
}
```
