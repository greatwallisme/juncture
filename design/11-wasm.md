# 11. WASM Support Design

## 1. Overview

### 1.1 Goal

Enable Juncture to compile and run on `wasm32-unknown-unknown` target, allowing the framework to be used in browser environments and other WASM runtimes (Node.js, Deno, Cloudflare Workers).

### 1.2 Scope

- **Primary target**: `wasm32-unknown-unknown` (browser via wasm-bindgen/wasm-pack)
- **Secondary target**: `wasm32-wasip2` (server-side WASM via WASI)
- **Out of scope**: Full multi-threaded WASM execution (requires Web Workers + SharedArrayBuffer)

### 1.3 Non-Goals

- Multi-threaded parallel execution on WASM (future enhancement)
- Full OpenTelemetry support on WASM
- Database-backed checkpoint savers on WASM (SqliteSaver, PostgresSaver)

---

## 2. Technology Research Findings

### 2.1 Tokio WASM Support

Tokio provides **partial WASM support** on `wasm32-unknown-unknown`. The supported features are:

| Feature | WASM Support | Notes |
|---------|-------------|-------|
| `sync` | YES | mpsc, RwLock, Mutex, Semaphore, Notify, watch, broadcast |
| `macros` | YES | `#[tokio::main]`, `#[tokio::test]` |
| `io-util` | YES | AsyncRead, AsyncWrite, BufReader, etc. |
| `rt` | YES | Single-threaded runtime, `tokio::spawn` |
| `time` | PARTIAL | `sleep()` works via JS setTimeout; `Instant::now()` **panics** on wasm32-unknown-unknown |
| `rt-multi-thread` | **BLOCKED** | compile_error! on wasm32 |
| `fs` | **BLOCKED** | compile_error! on wasm32 |
| `net` | **BLOCKED** | compile_error! on wasm32 (unstable with tokio_unstable + wasi) |
| `process` | **BLOCKED** | compile_error! on wasm32 |
| `signal` | **BLOCKED** | compile_error! on wasm32 |

**Key finding**: `tokio::spawn` and `JoinSet` are available on WASM via the `rt` feature. This means the Pregel engine's concurrency model (`JoinSet` + `Semaphore`) can work on WASM in single-threaded mode.

**Critical issue**: `tokio::time::Instant::now()` panics on `wasm32-unknown-unknown`. The `Instant` type uses platform-specific time APIs that are unavailable in the browser. The `web-time` crate provides a WASM-compatible `Instant` implementation.

### 2.2 Reqwest WASM Support

Reqwest has **native WASM support** via the browser's Fetch API:

- `reqwest::Client::new()` works on WASM
- HTTP requests use the Fetch API internally
- Response streaming uses ReadableStream
- `blocking` module is NOT available
- No connection pooling (browser handles connections)
- No custom DNS, proxy, or TLS configuration

### 2.3 UUID / Rand / Getrandom

- `getrandom` requires the `js` feature on `wasm32-unknown-unknown` to use `Crypto.getRandomValues()`
- `uuid` re-exports getrandom's `js` feature; solution: `uuid = { features = ["v4", "v6", "js"] }`
- `rand` works on WASM once getrandom is configured

### 2.4 WASM Threading (Future Consideration)

- WASM threads use Web Workers + SharedArrayBuffer
- Requires COOP/COEP HTTP headers on the server
- `wasm-bindgen-rayon` enables Rayon-based parallelism
- Browser support: Chrome/Edge full, Safari partial, Firefox behind flag
- **Recommendation**: Single-threaded execution for initial WASM support

---

## 3. Dependency Audit

### 3.1 Per-Crate Dependency Analysis

#### juncture-core

| Dependency | WASM Issue | Solution |
|-----------|-----------|---------|
| `tokio` (rt-multi-thread) | **BLOCKED** | Feature-gate: use `rt` only on WASM |
| `tokio-util` (CancellationToken) | Works | Available via `sync` feature |
| `tokio-stream` (ReceiverStream) | Works | Wraps tokio mpsc, available on WASM |
| `reqwest` | Works | Fetch API backend on WASM |
| `uuid` (v4, v6) | Needs `js` feature | Add `js` feature for WASM |
| `rand` | Needs getrandom `js` | Transitive via uuid |
| `chrono` | Works | Pure Rust, no platform deps |
| `schemars` | Works | Pure Rust |
| `xxhash-rust` | Works | Pure Rust |
| `indexmap` | Works | Pure Rust |
| `sqlx` (optional) | **BLOCKED** | Disable `sqlite`/`postgres` features on WASM |
| `opentelemetry` (optional) | **BLOCKED** | Disable `otel` feature on WASM |

#### juncture

| Dependency | WASM Issue | Solution |
|-----------|-----------|---------|
| `tokio` (rt) | Works | Use `rt` only on WASM |
| `reqwest` (optional) | Works | Fetch API backend |
| `chrono` | Works | Pure Rust |

#### juncture-checkpoint

| Dependency | WASM Issue | Solution |
|-----------|-----------|---------|
| `tokio` (sync) | Works | Available on WASM |
| `sqlx` (optional) | **BLOCKED** | Disable on WASM |
| `aes-gcm` (optional) | Works | Pure Rust crypto |
| `lru` | Works | Pure Rust |
| `rmp-serde` | Works | Pure Rust |

#### juncture-tracing

| Dependency | WASM Issue | Solution |
|-----------|-----------|---------|
| `tracing` | Works | Pure Rust |
| `tracing-subscriber` | Partial | `env-filter` works; some writers may not |
| `opentelemetry` (optional) | **BLOCKED** | Disable `otel` feature on WASM |

#### juncture-derive

| Dependency | WASM Issue | Solution |
|-----------|-----------|---------|
| `syn`, `quote`, `proc-macro2` | N/A | Compile-time only, runs on host |

#### juncture-store

| Dependency | WASM Issue | Solution |
|-----------|-----------|---------|
| (re-exports juncture-core) | Inherits | No direct dependencies |

### 3.2 `Send` Bound Analysis

The `Node` trait and Pregel engine require `Send` bounds:

```rust
pub trait Node<S: State>: Send + Sync + 'static {
    fn run(&self, state: &S, config: RunnableConfig)
        -> Box<dyn Future<Output = Result<Command<S>, JunctureError>> + Send + '_>;
}
```

On `wasm32-unknown-unknown`:
- WASM is single-threaded, so all types are effectively `Send`
- `tokio::spawn` requires `Send + 'static` (same as native)
- `JoinSet` requires `Send` futures (same as native)
- **No changes needed** to `Send` bounds

This is a key advantage of targeting `wasm32-unknown-unknown` over `wasm32-wasip2`: the `Send` bound is trivially satisfied because there's only one thread.

---

## 4. Architecture Design

### 4.1 Feature Flag Strategy

Introduce a `wasm` feature flag that gates WASM-incompatible dependencies:

```toml
# juncture-core/Cargo.toml
[features]
default = []
wasm = []  # Enables WASM-compatible configuration
otel = ["opentelemetry"]
sqlite = ["sqlx/sqlite"]
postgres = ["sqlx/postgres"]

[dependencies]
tokio = { version = "1", features = ["sync", "macros", "rt", "time"] }
# On WASM, we exclude rt-multi-thread
# On native, we add it via a separate feature

[target.'cfg(target_family = "wasm")'.dependencies]
tokio = { version = "1", features = ["sync", "macros", "rt", "time"] }

[target.'cfg(not(target_family = "wasm"))'.dependencies]
tokio = { version = "1", features = ["sync", "macros", "rt", "rt-multi-thread", "time"] }
```

However, Cargo's `[target]` dependencies have limitations with feature interactions. A cleaner approach uses conditional features:

```toml
# juncture-core/Cargo.toml
[features]
default = ["native-rt"]
native-rt = []  # Enables rt-multi-thread for native targets
wasm = []       # Marks WASM target, disables incompatible features
otel = ["opentelemetry"]
sqlite = ["sqlx/sqlite"]
postgres = ["sqlx/postgres"]

[dependencies]
tokio = { version = "1", features = ["sync", "macros", "rt", "time"] }
# rt-multi-thread is added conditionally in build.rs or via feature
```

**Recommended approach**: Use `cfg(target_family = "wasm")` in code, and let the user manage Cargo features. The `wasm` feature is a convenience flag that users enable when building for WASM.

### 4.2 Instant::now() Problem

The `tokio::time::Instant::now()` panic on WASM is the most impactful issue. It's used in:

- `juncture-core/src/pregel/runner.rs` -- task timing metrics
- `juncture-core/src/pregel/loop_.rs` -- superstep timing
- `juncture-core/src/store.rs` -- TTL sweep timing
- `juncture-core/src/runtime.rs` -- heartbeat timing

**Solution**: Abstract time access behind a platform-conditional wrapper:

```rust
// juncture-core/src/time.rs

/// Platform-compatible instant type.
/// On native: tokio::time::Instant
/// On WASM: web_time::Instant (uses Performance.now())
#[cfg(not(target_family = "wasm"))]
pub type Instant = tokio::time::Instant;

#[cfg(target_family = "wasm")]
pub type Instant = web_time::Instant;

/// Returns the current instant.
/// On native: tokio::time::Instant::now()
/// On WASM: web_time::Instant::now()
pub fn now() -> Instant {
    Instant::now()
}
```

The `web-time` crate provides `Instant` and `SystemTime` that work on WASM via `performance.now()` (for `Instant`) and `Date.now()` (for `SystemTime`).

### 4.3 Runtime Abstraction Layer

The Pregel engine uses tokio-specific APIs (`JoinSet`, `Semaphore`, `tokio::spawn`). For WASM compatibility:

**Option A: Direct tokio on WASM** (Recommended)
- tokio's `rt` feature works on WASM
- `JoinSet` and `Semaphore` are available
- Single-threaded execution (no parallelism)
- Minimal code changes

**Option B: Prokio runtime abstraction**
- Prokio wraps tokio (native) and wasm-bindgen-futures (WASM)
- Accepts `?Send` futures
- Adds dependency and abstraction overhead
- More future-proof for WASM threading

**Decision: Option A** -- Use tokio directly on WASM. The `rt` feature provides `tokio::spawn` and `JoinSet` on WASM. The Pregel engine runs single-threaded but functionally identically.

### 4.4 Feature Gate Matrix

| Feature | Native | WASM |
|---------|--------|------|
| `rt-multi-thread` | YES | NO (compile_error!) |
| `sync` (mpsc, Semaphore, RwLock) | YES | YES |
| `rt` (tokio::spawn, JoinSet) | YES | YES |
| `time` (sleep, interval) | YES | YES (via JS setTimeout) |
| `Instant::now()` | YES (tokio) | YES (web-time) |
| `reqwest` | YES | YES (Fetch API) |
| `uuid` v4/v6 | YES | YES (getrandom js) |
| `sqlx` (sqlite/postgres) | YES | NO (disabled) |
| `opentelemetry` | YES | NO (disabled) |
| `tracing` | YES | YES |
| `aes-gcm` (encryption) | YES | YES |

---

## 5. Implementation Plan

### Phase 1: Core WASM Compatibility (juncture-core)

**Goal**: Make `juncture-core` compile on `wasm32-unknown-unknown`.

#### 5.1.1 Time Abstraction

Create `juncture-core/src/time.rs` with platform-conditional `Instant`:

```rust
#[cfg(not(target_family = "wasm"))]
pub type Instant = tokio::time::Instant;

#[cfg(target_family = "wasm")]
pub type Instant = web_time::Instant;

pub fn now() -> Instant {
    Instant::now()
}
```

Replace all `tokio::time::Instant::now()` calls with `crate::time::now()`.

**Files affected**:
- `juncture-core/src/pregel/runner.rs`
- `juncture-core/src/pregel/loop_.rs`
- `juncture-core/src/store.rs`
- `juncture-core/src/runtime.rs`

#### 5.1.2 Feature-Gate Incompatible Dependencies

```toml
# juncture-core/Cargo.toml
[dependencies]
# ... existing deps ...

[target.'cfg(target_family = "wasm")'.dependencies]
web-time = "1"

[features]
otel = ["opentelemetry"]
sqlite = ["sqlx/sqlite"]
postgres = ["sqlx/postgres"]
# No changes needed for default features
```

Add compile-time guards:

```rust
// In store.rs
#[cfg(feature = "sqlite")]
mod sqlite_store;

#[cfg(feature = "postgres")]
mod postgres_store;
```

#### 5.1.3 UUID WASM Support

```toml
# juncture-core/Cargo.toml
[dependencies]
uuid = { version = "1", features = ["v4", "v6"] }

[target.'cfg(target_family = "wasm")'.dependencies]
uuid = { version = "1", features = ["v4", "v6", "js"] }
```

**Issue**: Cargo doesn't support per-target feature overrides for the same dependency. The solution is to use a feature flag:

```toml
[features]
wasm = ["uuid/js"]

[dependencies]
uuid = { version = "1", features = ["v4", "v6"] }
```

Users building for WASM enable the `wasm` feature: `cargo build --features wasm --target wasm32-unknown-unknown`.

#### 5.1.4 Tokio Feature Configuration

```toml
# juncture-core/Cargo.toml
[dependencies]
tokio = { version = "1", features = ["sync", "macros", "rt", "time"] }
tokio-util = "0.7"
tokio-stream = "0.1"
```

The key insight: **don't include `rt-multi-thread` in the default features**. Users who need multi-threaded runtime enable it explicitly:

```toml
[features]
default = ["multi-thread"]
multi-thread = ["tokio/rt-multi-thread"]
wasm = ["uuid/js"]
```

### Phase 2: Facade Crate WASM Compatibility (juncture)

**Goal**: Make the `juncture` crate compile on WASM.

#### 5.2.1 LLM Provider Feature Gates

The LLM providers use `reqwest` which works on WASM. No changes needed for basic HTTP.

However, streaming behavior differs on WASM. Add a note in documentation:

```rust
// juncture/src/llm/mod.rs
// On WASM, streaming uses the Fetch API's ReadableStream.
// Behavior is identical from the user's perspective, but internal
// buffering differs from native.
```

#### 5.2.2 Tool WASM Compatibility

`WebFetchTool` uses `reqwest` which works on WASM via Fetch API. No changes needed.

### Phase 3: Checkpoint WASM Compatibility (juncture-checkpoint)

**Goal**: `MemorySaver` works on WASM; database savers are feature-gated.

#### 5.3.1 Feature-Gate Database Savers

Already feature-gated (`sqlite`, `postgres`). Users building for WASM simply don't enable these features.

#### 5.3.2 Time Abstraction

Same as Phase 1 -- replace `tokio::time::Instant::now()` with `crate::time::now()`.

### Phase 4: Tracing WASM Compatibility (juncture-tracing)

**Goal**: Basic tracing works on WASM; OpenTelemetry is feature-gated.

#### 5.4.1 Feature-Gate OpenTelemetry

Already feature-gated (`otel`). `tracing` and `tracing-subscriber` work on WASM.

### Phase 5: Build Tooling & CI

#### 5.5.1 wasm-pack Integration

Add a `Makefile` target or script:

```bash
# Build WASM package
wasm-pack build --target web --features wasm --no-default-features

# Build for Node.js
wasm-pack build --target nodejs --features wasm --no-default-features
```

#### 5.5.2 CI Pipeline

Add WASM build check to CI:

```yaml
# .github/workflows/wasm.yml
- name: Install wasm32 target
  run: rustup target add wasm32-unknown-unknown

- name: Check WASM compilation
  run: cargo check --target wasm32-unknown-unknown -p juncture-core --features wasm --no-default-features

- name: Build WASM package
  run: wasm-pack build --target web --features wasm --no-default-features
```

### Phase 6: WASM Example

Create `examples/wasm-example/` demonstrating:

1. Building a Juncture graph for WASM
2. Running it in a browser
3. JavaScript interop via wasm-bindgen

---

## 6. User-Facing API

### 6.1 Building for WASM

```bash
# Install WASM target
rustup target add wasm32-unknown-unknown

# Install wasm-pack
cargo install wasm-pack

# Build
wasm-pack build --target web --features wasm --no-default-features
```

### 6.2 Cargo.toml for WASM Users

```toml
[dependencies]
juncture = { version = "0.1", default-features = false, features = ["wasm"] }
```

### 6.3 JavaScript Usage

```javascript
import init, { run_graph } from './pkg/juncture_wasm.js';

async function main() {
    await init();
    const result = await run_graph("What is 2+2?");
    console.log(result);
}

main();
```

### 6.4 Rust WASM Entry Point

```rust
use wasm_bindgen::prelude::*;
use juncture::prelude::*;

#[wasm_bindgen]
pub async fn run_graph(input: &str) -> Result<String, JsValue> {
    // Build and run a Juncture graph
    let graph = build_graph();
    let result = graph.invoke(input.into()).await
        .map_err(|e| JsValue::from_str(&e.to_string()))?;
    Ok(serde_json::to_string(&result).unwrap_or_default())
}
```

---

## 7. Limitations & Future Work

### 7.1 Current Limitations

| Limitation | Impact | Mitigation |
|-----------|--------|-----------|
| Single-threaded execution | No parallel node execution | Document; users can still use async concurrency |
| No SqliteSaver/PostgresSaver | No persistent checkpoints on WASM | Use MemorySaver; future: IndexedDB saver |
| No OpenTelemetry | No distributed tracing on WASM | Use tracing-subscriber with console output |
| Instant::now() via web-time | Slightly different precision | Negligible for most use cases |
| No file system access | No file-based tools | Use HTTP-based alternatives |

### 7.2 Future Enhancements

#### WASM Threading (Phase 7+)
- Use `wasm-bindgen-rayon` for parallel node execution
- Requires SharedArrayBuffer + COOP/COEP headers
- Browser support matrix: Chrome/Edge YES, Safari PARTIAL, Firefox FLAG

#### IndexedDB CheckpointSaver (Phase 7+)
- Implement `CheckpointSaver` using IndexedDB via `idb` crate
- Persistent checkpoints in browser

#### WASI Support (Phase 7+)
- Target `wasm32-wasip2` for server-side WASM
- Full std library support
- File system access via WASI

---

## 8. Checklist

### Phase 1: juncture-core WASM Compatibility
- [ ] Add `web-time` dependency for WASM
- [ ] Create `juncture-core/src/time.rs` abstraction
- [ ] Replace all `tokio::time::Instant::now()` calls
- [ ] Feature-gate `sqlx` (already done via `sqlite`/`postgres` features)
- [ ] Feature-gate `opentelemetry` (already done via `otel` feature)
- [ ] Add `wasm` feature with `uuid/js`
- [ ] Remove `rt-multi-thread` from default tokio features
- [ ] Verify `cargo check --target wasm32-unknown-unknown -p juncture-core --features wasm`

### Phase 2: juncture Facade WASM Compatibility
- [ ] Verify LLM providers compile on WASM
- [ ] Verify tools compile on WASM
- [ ] Add `wasm` feature propagation

### Phase 3: juncture-checkpoint WASM Compatibility
- [ ] Verify MemorySaver compiles on WASM
- [ ] Replace `tokio::time::Instant::now()` calls

### Phase 4: juncture-tracing WASM Compatibility
- [ ] Verify basic tracing works on WASM
- [ ] Document otel feature unavailability

### Phase 5: Build Tooling
- [ ] Add WASM CI pipeline
- [ ] Add wasm-pack build scripts
- [ ] Update CLAUDE.md with WASM build instructions

### Phase 6: Example
- [ ] Create `examples/wasm-example/`
- [ ] Demonstrate browser usage
- [ ] Document JavaScript interop
