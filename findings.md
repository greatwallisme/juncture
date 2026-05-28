# Findings: Juncture WASM Compatibility Research

## 1. WASM Target Types

### wasm32-unknown-unknown (Primary Target for Browser)
- The standard target for Rust -> WASM compilation for browsers
- Used by wasm-pack and wasm-bindgen
- No OS-level abstractions available
- Requires `getrandom` crate with `js` feature for random numbers
- Requires `wasm-bindgen` for JS interop

### wasm32-wasip1 / wasm32-wasip2 (Server-side WASM)
- WASI provides OS-like abstractions (filesystem, networking)
- Better std library support
- Suitable for server-side WASM (Wasmtime, WasmEdge, Wasmer)

### Decision: Target wasm32-unknown-unknown
- Browser interop is the primary use case
- wasm-bindgen ecosystem is mature
- wasm-pack provides excellent tooling

## 2. Tokio WASM Compatibility

### Supported Features on wasm32-unknown-unknown
- `sync` (mpsc, RwLock, Mutex, Semaphore, Notify, watch, broadcast) -- YES
- `macros` (#[tokio::main], #[tokio::test]) -- YES
- `io-util` -- YES
- `rt` (single-threaded runtime, tokio::spawn) -- YES
- `time` (sleep, interval, timeout) -- YES (but Instant::now() panics on wasm32-unknown-unknown)

### Blocked Features (compile_error!)
- `rt-multi-thread` -- BLOCKED
- `fs` -- BLOCKED
- `io-std` -- BLOCKED
- `net` -- BLOCKED
- `process` -- BLOCKED
- `signal` -- BLOCKED

### Key Finding: tokio::spawn works on WASM
- `tokio::spawn` is in the `rt` feature, which IS supported on WASM
- Spawned futures must be `Send + 'static` (same as native)
- On single-threaded WASM, `Send` is trivially satisfied

### Key Finding: JoinSet works on WASM
- `JoinSet` is in `tokio::task`, available with `rt` feature
- Should compile on WASM since `rt` is not blocked

### Key Finding: Instant::now() panics on wasm32-unknown-unknown
- `tokio::time::Instant::now()` panics on wasm32-unknown-unknown
- Need alternative: `web-time` crate or `js-sys::Date::now()`
- `tokio::time::sleep()` works via JavaScript setTimeout

## 3. Reqwest WASM Support

### Key Finding: reqwest supports WASM natively
- On wasm32 targets, reqwest uses the browser's Fetch API
- `reqwest::Client::new()` works on WASM
- `blocking` module is NOT available on WASM
- Streaming works via Fetch API's ReadableStream

### Differences from Native
- No connection pooling (browser handles connections)
- No custom DNS resolution
- No proxy support
- Limited TLS configuration (browser handles TLS)

## 4. UUID / Rand / Getrandom WASM Support

### getrandom
- wasm32-unknown-unknown requires `js` feature
- Uses `Crypto.getRandomValues()` via wasm-bindgen

### uuid
- `v4` and `v6` features require random number generation
- uuid re-exports getrandom's `js` feature as uuid's own `js` feature
- Solution: `uuid = { features = ["v4", "v6", "js"] }`

## 5. Async Runtime Alternatives

### wasm-bindgen-futures
- `spawn_local(future)` -- spawns a `!Send` future on the current thread
- `JsFuture` -- converts JS Promise to Rust Future
- `future_to_promise()` -- converts Rust Future to JS Promise

### Prokio
- Async runtime compatible with both WASM and native
- On WASM: uses wasm-bindgen-futures internally
- On native: uses tokio internally
- Accepts `?Send` futures

### Decision: Use tokio directly with feature gates
- tokio already works on WASM with `rt` feature
- Prokio adds unnecessary abstraction layer

## 6. WASM Threading

### Current State (2025)
- WASM threads use Web Workers + SharedArrayBuffer
- Requires COOP/COEP HTTP headers
- `wasm-bindgen-rayon` enables Rayon-based parallelism
- Not all browsers fully support SharedArrayBuffer

### Implications for Juncture
- Pregel engine runs single-threaded on WASM
- True parallelism requires Web Workers (complex setup)
- Recommendation: Single-threaded execution on WASM

## 7. Crate-by-Crate WASM Compatibility

| Crate | Status | Issues |
|-------|--------|--------|
| juncture-core | Needs changes | Instant::now(), sqlx, otel |
| juncture-derive | Compatible | proc-macro runs at compile time |
| juncture | Needs changes | reqwest streaming differences |
| juncture-checkpoint | Needs changes | sqlx, Instant::now() |
| juncture-tracing | Needs changes | otel not WASM-compatible |
| juncture-store | Compatible | Re-exports from juncture-core |
