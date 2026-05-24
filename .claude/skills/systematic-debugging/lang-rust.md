# Rust Debugging Playbook

**Core principle:** When the compiler rejects your code, it has already completed Phase 1. Your job is to understand what it found, not to work around it.

## Classify First, Investigate Second

Rust bugs fall into two categories. Do not start investigating until you know which one you are dealing with:

| Type | Where it appears | Correct first response |
|------|-----------------|----------------------|
| **Compile-time** — borrow, lifetime, type mismatch, missing trait impl | `cargo check` fails | Read the compiler output; understand the design contradiction it found |
| **Runtime** — panic, wrong output, undefined behavior | `cargo test` fails or production crash | Collect execution-time evidence |

**NEVER use `.clone()`, `unsafe`, or `Rc<RefCell<>>` to make a compile-time error disappear.** That is not a fix — it hides a design contradiction and pushes it into runtime, where it is harder to investigate.

---

## Section 1: Compile-time Bugs

### Why compiler errors are help, not obstacle

When the compiler rejects your code, it has proven that your code would fail in some situation — a problem that Go or Python would only discover at runtime. It is saving you from debugging a runtime bug.

**This reframe is a prerequisite for investigation.** If you treat a compiler error as an obstacle to route around, you will look in the wrong direction.

### Investigation path

**Step 1: Read the full compiler output — do not truncate it**

```bash
cargo check 2>&1 | less    # page through it; never pipe to head
```

Compiler errors have three parts with different meanings:

```
error[E0502]: cannot borrow `data` as mutable because it is also borrowed as immutable
  --> src/lib.rs:12:5
   |
10 |     let view = &data[..];    ← cause line: immutable borrow starts here
11 |     println!("{}", view[0]);
12 |     data.push(42);           ← conflict line: mutable borrow here
   |     ^^^^^^^^^^^^^
   |
help: consider cloning `view`    ← compiler's suggested fix
note: ...                        ← additional context
```

**Reading rules:**
1. `help:` and `note:` **must be read completely** — they frequently give the correct fix directly
2. The cause line (where the borrow begins) matters more than the conflict line — the root cause is at the origin, not the conflict point
3. Error code `E0502` → run `rustc --explain E0502` for a full explanation with examples

**Step 2: When there are multiple errors, fix only the first one**

```bash
cargo check 2>&1 | head -30    # look at only the first error
```

Later errors are often cascading consequences of the first. Fix the first, re-run check, avoid chasing phantoms.

**Step 3: Evidence is sufficient when:**
You can explain in your own words what logical contradiction the compiler found. Cannot explain it = do not understand it yet = do not touch the code.

### Root cause thinking for common compile errors

**`cannot borrow as mutable, also borrowed as immutable`**

The real issue: you are holding a read view and a write view of the same data simultaneously. If allowed, the write could invalidate the memory the read view points to.

The investigation direction is not "how to get around the borrow checker" but "why do I need both views at the same time? Can I redesign so they do not overlap?"

**`value used after move`**

The real issue: ownership has transferred; the original variable is no longer valid. Rust is preventing you from accessing data that may have been destroyed.

Ask first: **do both places genuinely need this value?** If yes, `.clone()`. If no, refactor so ownership is transferred only once.

**`the trait Send is not implemented`**

Common cause: `Rc<T>` (instead of `Arc<T>`), `*mut T`, or `RefCell<T>` crosses an `.await` point or enters a thread.

Investigation:
```bash
grep -n "Rc<\|*mut\|RefCell<" src/    # find which type breaks Send
```

**Lifetime errors everywhere**

First, make all elided lifetimes explicit and check whether your mental model matches what the compiler infers:

```rust
// Elided version (your understanding may differ from the compiler's)
fn get_name(data: &Data) -> &str { &data.name }

// Explicit version (makes intent unambiguous)
fn get_name<'a>(data: &'a Data) -> &'a str { &data.name }
```

If the explicit version compiles: the elision rules inferred something different from your intent — add the explicit lifetimes.

If the explicit version also errors: read what lifetime constraint the error points to — that is the real conflict.

**Struct lifetimes growing complex:** Ask first — "can this struct hold owned data (`String` instead of `&str`)?" Most of the time the answer is yes, and it is simpler.

---

## Section 2: Runtime Bugs

### Investigation path: Panic

**Step 1: Get a full backtrace**

```bash
RUST_BACKTRACE=1 cargo test 2>&1 | less       # standard
RUST_BACKTRACE=full cargo test 2>&1 | less    # use for async issues; includes executor frames
```

**Step 2: Start reading from your code frame, not from the top**

```
stack backtrace:
   0: rust_begin_unwind             ← panic infrastructure, skip
   1: core::panicking::panic_fmt    ← skip
   2: your_crate::db::query         ← ← ← start here: this is your first frame
        at src/db.rs:47:9
   3: your_crate::handler::get_user ← caller
        at src/handler.rs:23:5
   4: tokio::runtime::...           ← runtime, skip
```

Find the first frame belonging to your crate, then trace down the call chain asking "who passed the value that caused the panic?"

**Step 3: Special handling for `unwrap()` panics**

The error message `called unwrap() on a None value` only tells you where the unwrap is — it does not tell you why the value is `None`.

Trace up the call chain: who called this function, what precondition did it assume would give `Some`, and under what circumstances does that precondition fail?

**Step 4: Evidence is sufficient when:**
You can answer "where does this `None`/`Err` originate, and through which functions does it travel to reach here?"

### Investigation path: Async deadlock

**Symptom:** Async test or program hangs and never returns, with no panic.

**Most common cause:** `std::sync::Mutex` held across an `.await` point in an async function.

**Why this deadlocks:** `std::sync::Mutex` guards hold a thread-level lock. When the async executor yields at `.await`, the thread is released — but the guard is still on the stack. Other tasks waiting for that lock may run on the same thread, creating a deadlock.

```rust
// Wrong: guard spans an .await point
async fn process(mutex: &Mutex<Data>) {
    let guard = mutex.lock().unwrap();  // holds std::sync::Mutex guard
    some_async_fn().await;              // yields the thread; guard still alive
    use_data(&guard);
}
```

**Investigation steps:**

```bash
# Find std::sync::Mutex usage in async functions
grep -rn "std::sync::Mutex\|std::sync::RwLock" src/ --include="*.rs"
```

For each occurrence, check whether it is inside an `async fn` and whether the guard's lifetime spans an `.await`.

**Fix:** Replace `std::sync::Mutex` with `tokio::sync::Mutex`, which is designed for async contexts.

**Step 2: Add a timeout to confirm it is a deadlock, not infinite waiting**
```rust
tokio::time::timeout(
    Duration::from_secs(5),
    suspect_operation()
).await.expect("operation timed out — likely deadlock");
```

### Investigation path: Unsafe code and undefined behavior

**Symptom:** Does not reproduce in debug builds; crashes in release builds; behavior changes with optimization level.

**NEVER assume unsafe code is correct without running Miri first.**

```bash
rustup component add miri
cargo +nightly miri test
```

Miri detects: use-after-free, out-of-bounds access, reads of uninitialized memory, invalid pointer dereference.

When Miri cannot run (e.g., C FFI dependencies): comment out unsafe blocks one at a time, narrowing to the block containing the problem, then check line by line whether the invariants the block depends on are actually satisfied.

**Evidence is sufficient when:** you can state "this unsafe block assumes invariant X, and that invariant is violated when Y."

---

## Section 3: Done Criteria

**Before any Rust change is considered complete, both must pass:**

```bash
cargo check 2>&1 | grep "^error" | wc -l   # must be 0
cargo clippy -- -D warnings                  # must be 0
```

**NEVER consider a fix complete while warnings remain.** Clippy warnings frequently point directly at the bug.

---

## Architecture Red Flags (trigger Phase 4.5)

When these patterns appear, stop fixing the bug and discuss the architecture:

- **`.clone()` calls accumulating throughout the codebase** → Working around the borrow checker instead of fixing it; redesign ownership
- **`Rc<RefCell<T>>` or `Arc<Mutex<T>>` spread across data structures** → Too much shared mutable state; consider message passing
- **Fixing one `unsafe` block reveals a problem in another** → The safety abstraction boundary is unsound by design
- **Lifetime parameters propagating across multiple structs** → Consider having structs own their data, or use an arena allocator