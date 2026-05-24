# Go Debugging Playbook

**Core principle:** Go's concurrency makes symptoms unreliable. Any behavior you observe may not reflect the real cause until concurrency is ruled out.

## Classify First, Investigate Second

This is the most important step in Go debugging. Different bug types require completely different investigation strategies — picking the wrong one wastes time.

```
Symptom appears
    │
    ├─ Results occasionally wrong / occasional crash / behavior varies across runs
    │   └─→ Concurrency bug (Section 1) — run race detector first,
    │        or all other observations may be misleading
    │
    ├─ Program or test hangs and never exits
    │   └─→ Goroutine leak or deadlock (Section 2)
    │
    ├─ panic: nil pointer dereference / interface conversion fails
    │   └─→ Type system trap (Section 3)
    │
    ├─ Error matching fails / error handling behaves unexpectedly
    │   └─→ Error chain trap (Section 4)
    │
    └─ Test passes alone, fails in the full suite
        └─→ Test pollution (Section 5)
```

**NEVER add logging or change code before classifying the bug.** Without knowing which type you are facing, the evidence you collect will likely point in the wrong direction.

---

## Section 1: Concurrency Bugs

### Why this category is the hardest

Concurrency bugs come in two forms that look similar but require completely different investigation approaches:

| Type | Definition | Detectable by `-race`? | Characteristics |
|------|-----------|----------------------|----------------|
| **Data race** | Multiple goroutines access the same memory concurrently, at least one is a write | ✅ Yes | Consistent symptom, crash location is stable |
| **Logic race** | Program correctness depends on event ordering, ordering is non-deterministic | ❌ No | Symptoms are random; `-race` passes but results are wrong |

**Common mistake: `-race` reports nothing, so there is no concurrency problem.** This is wrong. `-race` passing only proves there is no data race — it says nothing about logic races.

### Investigation path: Data race

**Step 1: Run the race detector and read the full output**
```bash
go test -race ./...
# or
go run -race main.go
```

How to read race detector output — **do not only look at the conflict lines; look at the goroutine creation sites**:
```
WARNING: DATA RACE
Write at 0x00c0000b4010 by goroutine 7:
  main.(*Cache).Set()
      /app/cache.go:23          ← write location (symptom site)

Previous read at 0x00c0000b4010 by goroutine 6:
  main.(*Cache).Get()
      /app/cache.go:17          ← read location (symptom site)

Goroutine 7 (running) created at:
  main.startWorker()
      /app/main.go:45           ← ← ← the real root cause site: who started this goroutine
```

The root cause is at the goroutine **creation site**, not at the conflict lines. The conflict lines tell you what is shared; the creation site tells you why it was not protected.

**Step 2: Evidence is sufficient when you can answer:**
- Which shared data structure is being raced on?
- Which two goroutines are involved?
- Where were they created, and who owns their lifecycle?

### Investigation path: Logic race (`-race` cannot detect this)

**Symptoms:** Results occasionally wrong, `-race` passes, adding locks does not help.

**Step 1: Amplify the probability so the bug appears consistently**
```bash
go test -count=200 -timeout=120s ./...   # run 200 times
GOMAXPROCS=1 go test ./...               # single core: eliminates parallelism
                                          # if bug disappears, it is scheduling-order dependent
GOMAXPROCS=16 go test ./...              # many cores: amplifies concurrency, increases trigger rate
```

If the bug disappears with `GOMAXPROCS=1`, the bug is confirmed to be scheduling-order dependent.

**Step 2: Log operation sequences, not just operations**
```go
// Wrong: only records "what happened"
log.Printf("worker %d processed task %d", workerID, taskID)

// Correct: records "the order things happened" — use atomic counter for global sequence
var seq int64
log.Printf("[seq=%d] worker %d started task %d", atomic.AddInt64(&seq, 1), workerID, taskID)
```

Collect all sequenced log lines, reconstruct the event order, and find which ordering triggers the error.

**Step 3: Evidence is sufficient when you can describe:**
"When A happens before B the result is correct; when B happens before A the result is wrong."

### Common false leads

**False lead 1: Added mutex but still have a race** → Check whether the mutex is being copied when passed (sync types must not be copied)

**False lead 2: Using channels but still have a race** → A channel only protects the value it carries, not the original data the sender still holds

**False lead 3: `sync.WaitGroup` looks correct but tests fail randomly** → Check whether `Add` is called outside the goroutine
```go
// Wrong: Add is inside the goroutine — WaitGroup may reach Wait before Add is called
go func() {
    wg.Add(1)       // too late
    defer wg.Done()
    work()
}()

// Correct: Add must be called before starting the goroutine
wg.Add(1)
go func() {
    defer wg.Done()
    work()
}()
```

---

## Section 2: Goroutine Leaks and Deadlocks

### Why leaks are hard to catch

Leaks are gradual — the program behaves normally at startup, then memory and goroutine count slowly grow until OOM or performance collapses. Development environments do not run long enough to expose them; production does.

### Investigation path

**Step 1: Get a goroutine dump — do not guess**

When a program hangs, SIGQUIT prints all goroutine states without killing the process:
```bash
kill -3 <pid>     # Linux/macOS, output goes to stderr
# or press Ctrl+\ in the terminal
```

**Step 2: Read the goroutine dump — find the blocking reason**

```
goroutine 18 [chan receive, 3 minutes]:   ← status: waiting on channel for 3 minutes
main.worker(0xc000018180)
    /app/worker.go:34
created by main.startWorker               ← who created it
    /app/main.go:19

goroutine 23 [semacquire]:                ← status: waiting for a mutex
sync.(*Mutex).Lock(...)
main.processRequest()
    /app/handler.go:67                    ← where it is waiting for the lock
```

**Patterns to look for:**

| What you see | What it means |
|-------------|---------------|
| Many goroutines stuck on the same `[chan receive]` | No producer for that channel, or producer has exited |
| Goroutine count grows linearly over time | Goroutines are created but never stopped; context is not being propagated |
| Two goroutines each waiting on the other's mutex | Classic deadlock; find the lock acquisition order |
| `[chan send]` stuck | Channel is full with no consumer |

**Step 3: Verify no leak in tests**
```go
import "go.uber.org/goleak"

func TestMyWorker(t *testing.T) {
    defer goleak.VerifyNone(t)   // fail the test if goroutines remain after it ends
    // ...
}
```

**Step 4: Evidence is sufficient when you can say:**
"Goroutine X was created by Y, is waiting for Z, and is waiting because of W."

### Common false leads

**False lead: Added `context.Done()` check but still leaking** → Context is passed in, but the caller used `context.Background()` instead of passing the parent ctx

**False lead: Deadlock inside `String()` or `Error()` method** → `fmt.Printf` calls `String()` when printing a struct; if `String()` tries to acquire the same lock the caller holds, it deadlocks
```go
func (m *Manager) String() string {
    m.mu.Lock()         // if the caller already holds m.mu, this deadlocks
    defer m.mu.Unlock()
    return m.name
}
```

---

## Section 3: Type System Traps

### Typed nil: the most common "nil check passed but still panics"

**Symptom:** `if err != nil` evaluates to false, but calling `.Error()` panics.

**Why:** A Go interface holds two things: type information + value. A typed nil has non-nil type information, so it does not equal a nil interface.

```go
func findError() error {
    var e *MyError = nil
    return e              // returns (*MyError, nil), not (nil, nil)
}

err := findError()
if err != nil {           // true! because the type information is not nil
    err.Error()           // panic: nil pointer dereference
}
```

**Investigation:** When you see "nil check passed but still panics", print the type immediately:
```go
fmt.Printf("type=%T value=%v\n", err, err)
// If it prints: type=*main.MyError value=<nil>  → this is the problem
// A proper nil prints: type=<nil> value=<nil>
```

**Fix:** Functions returning `error` should `return nil` directly. Never return a nil pointer of a concrete type.

### Range loop variable capture (before Go 1.22)

**Symptom:** All goroutines print the same value — the last element of the loop.

```go
for _, item := range items {
    go func() {
        fmt.Println(item)  // all goroutines share the same item variable
    }()
}
```

**Investigation:** If all goroutine outputs are identical, this is the problem.

**Fix:** Create a copy with `item := item`, or pass `item` as a function argument. Fixed in Go 1.22.

---

## Section 4: Error Chain Traps

**Symptom:** `if err == ErrNotFound` is always false, even though the error genuinely came from `ErrNotFound`.

**Why:** `fmt.Errorf("context: %w", ErrNotFound)` creates a new error that wraps the original. Direct comparison fails because it is a different value.

```go
// Wrong: direct comparison does not unwrap
if err == ErrNotFound { }

// Correct: errors.Is unwraps the full chain
if errors.Is(err, ErrNotFound) { }

// Wrong: type assertion does not unwrap
if _, ok := err.(*NotFoundError); ok { }

// Correct: errors.As unwraps the full chain
var target *NotFoundError
if errors.As(err, &target) { }
```

**Investigation:** Print the error chain with `%+v` to see how many layers of wrapping exist:
```go
fmt.Printf("error chain: %+v\n", err)
// output: context1: context2: not found
```

**Evidence is sufficient when:** you can draw the complete error wrapping chain and name who added each layer.

---

## Section 5: Test Pollution

### Why this is hard to track down

Pollution is a two-test problem: test A corrupts global state, test B depends on that state being clean. Running either test alone is fine; the failure only appears when A runs before B.

**NEVER add cleanup code to the failing test before locating the pollution source.** You would be treating the symptom, not the cause.

### Investigation path

**Step 1: Confirm it is pollution, not something else**
```bash
# Run the failing test in isolation — does it pass?
go test -run ^TestFailing$ ./...

# If it passes alone but fails in the full suite, it is pollution
go test ./...
```

**Step 2: Use bisection to find the polluter**
```bash
# Randomize order — confirm the failure is ordering-dependent
go test -shuffle=on -count=10 ./...

# Use find-polluter.sh if available in this directory:
./find-polluter.sh 'global_var_name' './...'
```

**Step 3: Snapshot global state inside the failing test to find what changed**
```go
func TestFailing(t *testing.T) {
    before := captureGlobalState()
    t.Logf("state before: %+v", before)

    // ... test logic ...

    after := captureGlobalState()
    t.Logf("state after: %+v", after)
}
```

**Step 4: Evidence is sufficient when you can say:**
"Test X modified global variable Y. Test Z depends on Y having its initial value."

---

## Architecture Red Flags (trigger Phase 4.5)

When these patterns appear, stop fixing the bug and discuss the architecture:

- **Multiple goroutines racing on shared state even after adding locks** → The problem is not locking; the data ownership design is wrong. Consider passing ownership through channels.
- **Tests require `time.Sleep` to pass** → Missing proper synchronization. See `condition-based-waiting.md`.
- **`init()` functions have side effects** → Global state initialization is uncontrolled; test order affects results.
- **3+ goroutine leak fixes, each fix uncovers another leak** → No consistent pattern for goroutine lifecycle management across the codebase.