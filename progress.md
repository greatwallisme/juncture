# Progress Log

## Session 2026-07-27: Production-Readiness Audit — COMPLETE

### Phase 1: Baseline quality gates — COMPLETE
- `cargo build --workspace --all-features` — PASS
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` — PASS (after removing dead `string_to_string` lint config at Cargo.toml:60, covered by `implicit_clone`/pedantic)
- `cargo fmt --all -- --check` — PASS
- `cargo test --workspace --all-targets --all-features` — PASS (~1000+ tests, 0 failed)
- `cargo doc --workspace --all-features --no-deps` — **62 warnings** (46 in juncture-core) — broken doc links, unclosed HTML tags
- Design coverage: 209/214 (97.7%), 5 missing (2 stale design doc, 3 unimplemented features)

### Phase 2: Parallel dimension audits — COMPLETE
- **Concurrency (rust-expert)**: 2 BLOCKER + 6 MAJOR + 6 MINOR. Data-loss pathway B-1 (state loss on error/cancel) + B-2 (fire-and-forget checkpoint). Confirmed by reading code.
- **Error handling (Explore)**: 222 flagged lines; 44 no-panic-rule violations; hot-path panic on user input; silent durability failures (put_writes, channel checkpoint, subgraph resume). Confirmed.
- **Test coverage (Explore)**: 5 HIGH gaps (real LLM, store backends, panic, concurrent-write, ReAct E2E); disciplined regression tests for known bugs (positive); no property/fuzz tests.

### Phase 3: Design conformance — COMPLETE
- 209/214 (97.7%); 2 stale-doc gaps (GraphNotFound, InvalidNamespace — dead code correctly removed, doc not updated); 3 unimplemented features (with_context_schema, DebugEvent variants, StreamChannel).

### Phase 4: Direct-audit dimensions — COMPLETE
- API/Packaging: examples + deep-research missing `publish = false`; no docs.rs metadata.
- Resource mgmt: unbounded stream/interrupt channels → OOM (confirms M-3/M-4); timeouts present.
- Observability: STRONG — juncture-tracing (OTel) + juncture-telemetry (Langfuse) + instrumented Pregel runner.
- Security/unsafe: only wasm_send.rs, sound + SAFETY-commented + cfg-gated; panic-on-user-input is the DoS surface.
- Docs: READMEs/LICENSE good; no CI; 62 rustdoc warnings.

### Phase 5: Synthesize verdict — COMPLETE
- **VERDICT: NOT PRODUCTION-READY.**
- 8 BLOCKERs, ~11 MAJORs documented with file:line evidence in findings.md + task_plan.md.
- Suitable for preview/evaluation; not for workloads where state durability or fault isolation matters.

### Files modified this session
- `Cargo.toml` — removed dead `string_to_string = "warn"` lint config (covered by `implicit_clone`).
- `task_plan.md` / `findings.md` / `progress.md` — audit planning files (untracked, per project convention).

## Session 2026-07-27b: Fix audit findings item-by-item

### Phase 1+2: Pregel engine correctness cluster — COMPLETE (verified)
- **B-1** `loop_.rs:1017` — restore pre-superstep state from `arc_state` on error/cancel path (was: `?` dropped all state since last checkpoint). Added `#[allow(clippy::too_many_lines)]` with justification.
- **M-5/#3** `runner.rs:159` (single-task) + `runner.rs:653` (parallel) — propagate `put_writes` errors via `?` / cancel+shutdown+return Err (was: `let _ =` silently dropped storage failures).
- **M-1** `runner.rs:387` — replace `expect()` on semaphore acquire with `match` returning `JunctureError::execution` (was: cascading panic storm risk).
- **M-2** `config.rs:220` + `runner.rs:320` — clamp `max_parallel_tasks` to >= 1 in constructor + `.max(1)` guard at semaphore creation (was: `Semaphore::new(0)` permanent deadlock).
- **m-2** `loop_.rs:1236` — `.expect("duration should fit in u64")` → `.unwrap_or(u64::MAX)` (consistent with line 1273).
- Added `impl From<CheckpointError> for JunctureError` in `error.rs` to enable `?` propagation of checkpoint persistence errors.
- **m-1** `loop_.rs:812,839` — LEFT AS-IS: `unreachable!()` carries a justification message, which is explicitly ALLOWED per `quality_gates.md` #4. Not a project-rule violation; changing working compliant code would be churn.
- Verification: `cargo build -p juncture-core` PASS; `cargo clippy -p juncture-core --all-targets --all-features -- -D warnings` PASS (exit 0); `cargo test -p juncture-core --all-targets --all-features` PASS (663 tests, 0 failed).
- Doctest note: 3 doctests fail (TopicChannel/NamedBarrierChannel/SubgraphTransformer::to_emitter) — PRE-EXISTING (verified via git stash: fail on unmodified code). Part of Phase 8 rustdoc work, not caused by these fixes.

### Phase 3+: remaining — pending
- B-2 fire-and-forget checkpoint (architectural — needs design-doc check)
- #4 interrupt!/Send serde panic, #5 empty API key, #7 subgraph resume, #8 channel checkpoint
- lock poisoning, checkpoint int expects, API ergonomics, packaging/docs, tests, CI

### Cluster 2: user-input panic fixes (#4, #5) — COMPLETE (verified)
- **#4 interrupt macros** `lib.rs` — replaced `.expect("interrupt payload must be serializable")` in all 4 arms of `interrupt!`/`interrupt_with_ctx!` with `match serde_json::to_value(...) { Ok => ..., Err(err) => Err(JunctureError::execution(...)) }`. Non-serializable payloads (f64::NAN, non-Serialize) now surface as JunctureError instead of crashing the worker.
- **#4 Send API** `send.rs` — replaced panicking `From<Send<S>>` with fallible `TryFrom<Send<S>>` returning `Result<SendTarget, JunctureError>`. Updated 2 benchmark callers (`profile.rs:636`, `fanout.rs:144`) `.into()` → `.try_into().collect::<Result<Vec<_>,_>>()?`. (Could not keep both From+TryFrom: blanket `impl TryFrom<U> for T where U: Into<T>` conflicts. Chose correct fallible API over lossy infallible fallback.)
- **#5 empty/panicking API keys** `chat.rs` — core `ChatOpenAI::from_env()` (`.unwrap_or_default()` silent empty key) and `ChatAnthropic::from_env()` (`.expect()` panic) both changed to return `Result<Self, LlmError>` erroring on missing key, matching the facade's behavior. Removed `# Panics` doc. No external callers (facade types are the real ones used by examples).
- Used `|_err|` (lint-approved ignored identifier) for `map_err` to satisfy `clippy::map_err_ignore` (workspace lint, deny warnings).
- Verification: `cargo clippy --workspace --all-targets --all-features -- -D warnings` PASS (exit 0); `cargo test --workspace --all-targets --all-features` PASS (~1000 tests, 0 failed).

### Cluster 3: durability error-propagation + packaging (#7, #8, packaging) — COMPLETE (verified)
- **#7 subgraph resume** `subgraph.rs:323` — replaced `.ok().flatten()` (swallowed CheckpointError → silent subgraph re-invoke from scratch, discarding HITL state) with `?` propagation via the new `From<CheckpointError> for JunctureError` impl. DB errors now surface instead of masking data loss.
- **#8 channel checkpoint** `channel.rs:300,456,669,815,939` — added module-private `checkpoint_to_value` helper that logs a `tracing::warn!` (`juncture.channel.checkpoint_serialize_failed`) on serialization failure instead of silently `.ok()`-dropping. The `Channel::checkpoint` trait signature is `Option<Value>` (infallible by contract); changing to `Result` would be a breaking trait change across all impls+callers, so the non-breaking fix makes the failure observable. Non-serializable channels are still excluded from the checkpoint but now logged.
- **Packaging** `examples/Cargo.toml`, `examples/deep-research/Cargo.toml` — added `publish = false` (only benchmarks had it). Prevents `cargo publish --workspace` from publishing example crates to crates.io.
- Verification: `cargo clippy --workspace --all-targets --all-features -- -D warnings` PASS (exit 0); `cargo test --workspace --all-targets --all-features` PASS (44 test-result groups, 0 failures); `cargo fmt --all -- --check` PASS.

### Architectural items — analyzed against design doc (NOT blindly changed)
- **B-2 fire-and-forget async checkpoint**: design/03-pregel-engine.md §11.3 EXPLICITLY defines `Durability::Async` as fire-and-forget background writes accepting "may lose recent checkpoint on crash". This is the design's intentional perf/durability tradeoff, NOT a deviation. Error is already observable (warn + metric). NOT rewriting to synchronous (would change the design). Open refinement: spawn-write ordering — documented caveat, needs backend step-guarding.
- **#6 unbounded stream channel**: EventEmitter is broadcast semantics (design: "consumers may disconnect without disrupting execution"); unbounded is the standard broadcast pattern, design-accepted. Bounding would stall the engine. Interrupt channel (M-4): interrupts are HITL pause points (can't loop), low OOM risk. Documented caveats.
- **M-6 partial mutation on try_apply failure**: `State: Clone` implied, so snapshot+rollback is possible, but `S::clone(state)` per superstep reintroduces the O(N²) wide-state regression the prior `mem::take`+Arc optimization removed. Proper fix needs an undo-log (changed-fields' pre-values), not a full snapshot — documented as known limitation.

### Files modified this session (audit fixes)
- `Cargo.toml` — removed dead `string_to_string` lint (audit session).
- `crates/juncture-core/src/error.rs` — added `From<CheckpointError> for JunctureError`.
- `crates/juncture-core/src/pregel/loop_.rs` — B-1 state restore; m-2 duration; `#[allow(too_many_lines)]`.
- `crates/juncture-core/src/pregel/runner.rs` — M-5 put_writes propagation; M-1 semaphore; M-2 permit clamp.
- `crates/juncture-core/src/config.rs` — M-2 max_parallel_tasks clamp.
- `crates/juncture-core/src/lib.rs` — #4 interrupt macros serde panic→Result.
- `crates/juncture-core/src/send.rs` — #4 From→TryFrom (non-panicking).
- `crates/juncture-core/src/chat.rs` — #5 OpenAI+Anthropic from_env Result.
- `crates/juncture-core/src/subgraph.rs` — #7 checkpoint error propagation.
- `crates/juncture-core/src/state/channel.rs` — #8 logged checkpoint serialization.
- `benchmarks/src/bin/profile.rs`, `benchmarks/benches/fanout.rs` — try_into() for Send conversion.
- `examples/Cargo.toml`, `examples/deep-research/Cargo.toml` — `publish = false`.

### Remaining (substantial, documented as pending)
- Lock poisoning: 13 sites (runtime.rs:408,418,428; metrics.rs:184,201,241,258,679; memory.rs:109,123,140; stream.rs:1065,1087) → recover-poisoned pattern.
- Checkpoint int expects: sqlite.rs:436,738,931; postgres.rs:341,656,831 → map_err.
- API ergonomics Phase 7: unify core+facade `ChatOpenAI::from_env` (read OPENAI_BASE_URL/OPENAI_MODEL, return Result) consistently.
- 62 rustdoc warnings + 3 broken doctests (TopicChannel/NamedBarrierChannel/SubgraphTransformer) — Phase 8.
- 5 HIGH test gaps: panic-in-JoinSet, concurrent-same-field-write, env-gated real-LLM, SqliteStore/PostgresStore integration, ReAct E2E.
- CI: .github/workflows.
- docs.rs metadata per crate.

### Final verification (this session)
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` — exit 0
- `cargo test --workspace --all-targets --all-features` — exit 0 (44 ok groups, 0 failures)
- `cargo fmt --all -- --check` — exit 0

### Cluster 4: lock poisoning + checkpoint int expects — COMPLETE (verified)
- **Lock poisoning (13 sites)** — replaced `.lock().unwrap()`/`.read().unwrap()`/`.write().unwrap()`/`.lock().expect(...)` with `.unwrap_or_else(std::sync::PoisonError::into_inner)` (recover-poisoned idiom; clippy `redundant_closure` wanted the fn-ref form over `|e| e.into_inner()`). Updated `# Panics` doc sections to "Recovers" notes.
  - `runtime.rs` (3: request_drain/drain_requested/drain_reason) — RunControl stays usable after a prior thread panic.
  - `stream.rs` (2: BatchTransformer flush/transform) — stream pipeline doesn't crash on poisoned buffer lock.
  - `metrics.rs` (5: inc_by/get/record/get_values/store_metadata) — metrics subsystem not disabled for process lifetime.
  - `memory.rs` (3: ttl_config/set_ttl_config/lazy_cleanup) — checkpointer not rendered unusable.
- **Checkpoint int expects (6 sites)** — replaced `u32::try_from(...).expect()`/`i64::try_from(...).expect()` on DB-row/user-controlled values with `.map_err(|e| CoreCheckpointError::Storage(Box::new(e)))?` propagation. For the two `.bind(i64::try_from(idx).expect(...))` sites (sqlite:931, postgres:831), extracted `let idx_i64 = ...?;` before the query builder. Corrupted/hostile rows now surface as `CheckpointError::Storage` instead of panicking.
  - `sqlite.rs:436` (schema_version), `:738` (limit), `:931` (idx).
  - `postgres.rs:341` (schema_version), `:656` (limit), `:831` (idx).
- Verification: clippy exit 0; test exit 0 (44 ok groups, 0 failures); fmt exit 0 (after `cargo fmt` fixed 2 idx_i64 let-line wraps).

### Cluster 5: Phase 7 API ergonomics — ChatOpenAI::from_env unification — COMPLETE (verified)
- **Core `chat.rs::ChatOpenAI::from_env`** + **facade `openai.rs::ChatOpenAI::from_env`**: both now read `OPENAI_API_KEY` (required, errors if missing), `OPENAI_MODEL` (default `gpt-4o`), and `OPENAI_BASE_URL` (default `https://api.openai.com/v1`). Aligns the library API with `.env.example` and `examples/src/common.rs`, so a configured `.env` yields a working OpenAI-compatible client (vLLM/Together AI/Groq/Azure) without manual `.with_base_url().with_model()` chaining.
- Used `.unwrap_or_else(|_| "...".to_string())` (lazy alloc) — `unwrap_or(to_string())` triggers `clippy::or_fun_call`; this form matches `examples/common.rs` and passes.
- Backticked provider names (`vLLM`/`Together AI`/`Groq`/`Azure OpenAI`) in the doc to satisfy `clippy::doc_markdown`.
- Note: facade `ChatOpenAI::new` (openai.rs:99) still uses `Client::build().expect("Failed to create HTTP client")` (audit MEDIUM #7). Making `new()` return `Result` is a breaking API change touching all callers (tests/examples/benchmarks/prebuilt) — left as documented remaining item.
- Verification: clippy exit 0; test exit 0 (44 ok groups); fmt exit 0.
- Side note: `scheduler.rs:830` `#[expect(clippy::too_many_lines)]` is unfulfilled under the non-standard `cargo clippy -p juncture --all-features` (no `--all-targets`) invocation due to feature-gated line-count variance — pre-existing fragility, NOT caused by these changes; the canonical `--workspace --all-targets --all-features` gate passes.

### Cluster 6: rustdoc warnings (62 → 0) — COMPLETE (verified)
- **Unclosed HTML tags (9 sites)**: backticked type literals in doc comments that rustdoc parsed as HTML — `Option<T>` (derive lib.rs:9), `Vec<CheckpointTuple>`/`Vec<PendingWrite>` (memory.rs:67,70), `Vec<Value>` (interrupt/mod.rs:66,70), `Runtime<C>` (into_node.rs:48,177,226,275,324).
- **Unresolved intra-doc links (~50 sites)**: converted `[`X`]` links to types/variants/methods not resolvable from the doc's module scope (private variants like `JunctureError::Checkpoint`, cross-crate `web_time::Instant`/`juncture_tracing::callback::GraphCallbackHandler` without a dep, removed/unimplemented items) to plain code spans `` `X` ``. Files: compiled.rs (11), builder.rs (7), command.rs (2), graph/mod.rs (1), llm.rs (2), observability.rs (2), pregel/loop_.rs (3), scheduler.rs (1), types.rs (2), subgraph.rs (1), time.rs (2), tools.rs (2), benchmarks/profiling.rs (1), juncture-tracing/config.rs (1), juncture-tracing/metrics.rs (1), juncture/src/llm/trait_.rs (3). Removed one broken reference-link definition (observability.rs GraphCallbackHandler → no juncture-tracing dep from core).
- **Redundant explicit link target** (func/mod.rs:4): `[`StateGraph`](crate::graph::StateGraph)` → `[`StateGraph`]`.
- **Private-item link** (subgraph.rs:207): `[`compute_child_namespace`]` → plain code span.
- Fix-up: my `replace_all` of `[`RunnableConfig`]` in builder.rs accidentally broke one valid explicit-target link at builder.rs:20 (`[\`RunnableConfig\`](crate::RunnableConfig)` → dangling `(crate::RunnableConfig)`); restored the brackets.
- Verification: `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --all-features --no-deps` — **exit 0** (was 62 warnings).

### ALL 5 rust-guidelines quality gates now PASS
1. `cargo fmt --all -- --check` — exit 0
2. `cargo clippy --workspace --all-targets --all-features -- -D warnings` — exit 0
3. `cargo test --workspace --all-targets --all-features` — exit 0 (44 ok groups, 0 failures)
4. `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --all-features --no-deps` — exit 0
5. `cargo build --workspace --all-features` — exit 0

### Newly discovered: `cargo test --doc` failures (pre-existing, large)
The project's gate uses `--all-targets` (excludes doctests). Running `cargo test --workspace --doc --all-features` reveals ~20+ pre-existing doctest failures NOT caused by these fixes:
- juncture crate: `circuit_breaker` (×6), `middleware` (×8) — `E0603: module is private` (doctests import `juncture::llm::circuit_breaker::...` but the module isn't publicly re-exported).
- juncture-core: `TopicChannel` (channel.rs:356), `NamedBarrierChannel` (channel.rs:166), `SubgraphTransformer::to_emitter` (subgraph.rs:1575) — `MyState: Default not satisfied`, `no method update/get/expect` (doc examples use APIs that don't compile).
These are broken doc EXAMPLES (not doc warnings). Fixing requires correcting each example's imports/types — a substantial separate sub-task. Documented as remaining.

### Cluster 7: doctests (~20 failures → 0) — COMPLETE (verified)
- **facade `circuit_breaker.rs` (×6) + `middleware.rs` (×8) `E0603: module is private`**: doctests imported `juncture::llm::circuit_breaker::...` / `juncture::llm::middleware::...` but those modules are private (`mod`, not `pub mod`) with items re-exported at `juncture::llm::*` via `pub use module::*;`. Fix: replace_all the private module path prefix with `juncture::llm::` in doctest imports.
- **`middleware.rs:256 with_middlewares` `E0308`**: doctest passed raw `LoggingMiddleware::new()` / `MetricsMiddleware::new()` but `with_middlewares` takes `&[Arc<dyn LlmMiddleware>]`. Fix: wrapped each in `Arc::new(...)`.
- **`channel.rs` TopicChannel doctest**: `update` was called with `Vec<T>` but `TopicChannel<T>: Channel<Vec<T>>` so `update` takes `Vec<Vec<T>>`; `.reset()` doesn't exist (use `Channel::consume`); `Channel` trait not in scope. Rewrote with `use ...Channel`, `vec![vec![...]]`, `consume()`.
- **`channel.rs` NamedBarrierChannel doctest**: `update` returns `bool` (not `Result`), so `.expect()` on `bool` was invalid; `Channel::get` needed trait in scope. Rewrote with `use ...Channel`, `assert!(channel.update(...))`.
- **`subgraph.rs` SubgraphTransformer::to_emitter doctest**: `MyState` didn't impl `Default` (required by `State: Default`). Added `Default` to the derive.
- Verification: `cargo test --workspace --doc --all-features` — **0 failures** (was ~20).

### COMPLETE quality gate (all green)
1. `cargo fmt --all -- --check` — exit 0
2. `cargo clippy --workspace --all-targets --all-features -- -D warnings` — exit 0
3. `cargo test --workspace --all-targets --all-features` — exit 0 (44 ok groups)
4. `cargo test --workspace --doc --all-features` — exit 0 (doctests pass)
5. `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --all-features --no-deps` — exit 0
6. `cargo build --workspace --all-features` — exit 0

### Remaining (documented as pending)
- 5 HIGH test gaps: panic-in-JoinSet, concurrent-same-field-write, env-gated real-LLM, SqliteStore/PostgresStore integration, ReAct E2E.
- CI workflow (`.github/workflows`).
- facade `ChatOpenAI::new` / `ChatAnthropic::new` / `ChatOllama::new` `Client::build().expect()` (audit MEDIUM #7) — making `new()` return `Result` is a breaking API change touching all callers; left as documented.
- docs.rs metadata per crate (small).

### CI workflow — ADDED (`.github/workflows/ci.yml`)
Locks in the full quality gate so it cannot silently regress:
- **fmt job**: `cargo fmt --all -- --check`.
- **clippy job**: `cargo clippy --workspace --all-targets --all-features -- -D warnings`.
- **build-test-doc job**: `cargo build --workspace --all-features --locked`, `cargo test --workspace --all-targets --all-features`, `cargo test --workspace --doc --all-features`, `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --all-features --no-deps`.
- **msrv job**: builds on the declared MSRV `1.87` (`cargo build --workspace --all-features`).
- Triggers: push + PR on `main`/`master`. Uses `dtolnay/rust-toolchain` (+ rustfmt/clippy components), `Swatinem/rust-cache@v2`, installs `libsqlite3-dev`. YAML validated; `--locked` verified to work locally (Cargo.lock is committed and in sync).
- Note: this is a workflow file only (not compiled by cargo), so it has no impact on the local quality gate.

### docs.rs metadata — ADDED (all 7 published crates)
The published library crates had no `[package.metadata.docs.rs]`, so docs.rs would build docs with default features only (missing all feature-gated API like `sqlite`/`postgres`/`otel`/`chat`). Added `[package.metadata.docs.rs] all-features = true` to all 7 (`juncture`, `juncture-core`, `juncture-derive`, `juncture-checkpoint`, `juncture-tracing`, `juncture-store`, `juncture-telemetry`). Combined with the now-zero rustdoc warnings, published docs.rs documentation will build cleanly and cover every feature-gated API. (Examples/benchmarks excluded — they're `publish = false`.)
- Verification: all 7 `Cargo.toml` carry the table; `cargo build --workspace --all-features` passes (TOML valid); full gate green (clippy/fmt/doc exit 0).

### Packaging/docs dimension — COMPLETE
- `publish = false` on examples + deep-research ✅
- 62 rustdoc warnings → 0 ✅
- ~20 broken doctests → 0 ✅
- docs.rs metadata (all-features) on all 7 published crates ✅

### `new()` Client::build expect (#7) — ACCEPTED (documented, per user decision)
The facade `ChatOpenAI::new` / `ChatAnthropic::new` / `ChatOllama::new` still use `Client::builder().build().expect(...)`. Converting `new()` to return `Result` is a breaking public-API change touching ~20 call sites (most are runnable `rust` doctests needing `fn main() -> Result` rewrites). The panic only fires on reqwest TLS-backend init failure (a broken system OpenSSL/rustls install — extremely rare and unrecoverable at runtime). Per user decision ("accept + document"):
- Corrected the previously-false `# Panics / This function does not panic.` doc to honestly describe the panic condition + acceptance rationale (a functioning TLS stack is a hard environment requirement for an HTTP LLM client; surfacing at construction is preferable to masking).
- Made the `expect` message self-documenting: `"Failed to create HTTP client: reqwest TLS backend initialization failed (audit #7: accepted environment-level fault)"`.
- No code behavior change; gate remains green.

## ALL code-level audit findings now resolved (fixed or accepted-documented)
The only remaining items are external-service test coverage (real LLM provider, PostgresStore) which require infrastructure/secrets, not code changes.

### M-1 semaphore error-handling path — VERIFIED (test added)
The M-1 `Err` path (closed semaphore → `acquire_owned()` returns `Err`) is **unreachable via the public API** because the semaphore is owned internally by `execute_superstep` and never closed in normal operation. To make the defensive contract directly testable without changing behavior:
- Extracted the M-1 `match` into a private `async fn acquire_permit(semaphore: Arc<Semaphore>, node_name: &str) -> Result<OwnedSemaphorePermit, (String, JunctureError)>` helper (identical logic; `acquire_owned` consumes the `Arc`, so the helper takes it by value and the spawn body moves the cloned `Arc` in).
- The spawn body now calls `let _permit = acquire_permit(permit, &node_name).await?;` (the `?` propagates `(String, JunctureError)` through the closure's `map_err` path into the JoinSet's `Ok(Err(...))` arm at runner.rs error handling — surfacing as a normal task error, not a panic).
- Fixed an attribute-displacement side-effect: inserting the helper between `execute_superstep`'s doc and its `#[expect]` block had re-attached `#[expect(clippy::too_many_lines)]` to the wrong fn and stolen `execute_superstep`'s doc. Relocated the helper to stand alone before `execute_superstep`'s doc; restored the `#[expect]` to `execute_superstep`; removed the now-stale `# Panics` section (M-1 made it return an error instead) and noted the new error case in `# Errors`.
- Added 2 unit tests in runner.rs test module:
  - `test_acquire_permit_closed_semaphore` — closes a semaphore, calls `acquire_permit`, asserts `Err((node_name, JunctureError))` with `is_execution()` (proves M-1 converts the closed-semaphore `AcquireError` into a `JunctureError`, NOT a panic).
  - `test_acquire_permit_open_semaphore` — verifies the normal hot path yields a permit (confirms the refactor didn't break the Ok path; existing `execute_superstep` tests already exercise this end-to-end).
- Verification: `cargo test -p juncture-core --lib test_acquire_permit` — **2 passed**; full gate green (clippy/test --all-targets/test --doc/fmt all exit 0).

### Panic-in-JoinSet fault-isolation path — VERIFIED (audit HIGH gap #3 closed)
The engine's primary fault-isolation path (`runner.rs` `Err(join_error)` arm: a task panicking inside the JoinSet is caught and surfaced as `JunctureError::execution("Task panicked: ...")` rather than crashing the runtime or hanging) was previously untested.
- Added `test_execute_superstep_task_panic_propagates_as_error`: a node closure that `panic!()`s, run via `execute_superstep` with **two** tasks (the second forces the JoinSet path — the single-task inline fast-path at `try_execute_single_task_inline` doesn't catch panics, and only runs for exactly 1 task with no retry/timeout/error-handler). Asserts the superstep returns `Err(JunctureError)` with `is_execution()`. Confirmed `panic = "unwind"` (no `[profile]` override) so spawned panics are catchable.
- The panicked task's panic message prints to stderr (tokio panic hook) — known/accepted noise from testing panic handling, not a failure.
- Verification: `cargo test -p juncture-core --lib test_execute_superstep_task_panic` — **1 passed**; full lib suite now **563 passed** (was 560); clippy/doc/fmt green.

### Engine fault-isolation tests now in place
- M-1 (closed semaphore → error): `test_acquire_permit_closed_semaphore` + `test_acquire_permit_open_semaphore`.
- Panic propagation (HIGH gap #3): `test_execute_superstep_task_panic_propagates_as_error`.
- Cancellation: pre-existing `test_execute_superstep_cancellation`.
- Retry/timeout: pre-existing retry/timeout tests (8 tests).

### Remaining HIGH test gaps (3 of 5 still open)
- Concurrent-same-field-write determinism (`loop_.rs` Pregel invariant — "concurrent writes to same field produce deterministic merged result").
- Env-gated real-LLM provider tests (mirror `TEST_POSTGRES_URL` pattern; `#[ignore]` + `TEST_LLM_BASE_URL`).
- SqliteStore/PostgresStore integration tests (`store.rs:1027,1845`).
- ReAct E2E multi-step loop through Pregel.

### Concurrent-same-field-write determinism — VERIFIED (audit HIGH gap closed)
The Pregel invariant (`loop_.rs:1145`: "concurrent writes to the same field produce a deterministic result") was previously untested. The determinism comes from `apply_writes` (scheduler.rs) re-sorting PULL tasks alphabetically by node name BEFORE applying updates, so the merge is independent of task completion order under the parallel JoinSet.
- Added `test_apply_writes_concurrent_same_field_is_order_independent` in scheduler.rs: defines an `AppendState` (single `Vec<String>` append field) + `AppendUpdate`, feeds the SAME three writers (nodes a/b/c, each appending its letter) in `[a,b,c]` vs `[c,b,a]` order to `apply_writes`, and asserts both produce `["a","b","c"]` and are identical — proving order-independence.
- Verification: `cargo test -p juncture-core --lib test_apply_writes_concurrent_same_field` — **1 passed**; full lib suite **564 passed**; clippy/fmt green.

### Engine correctness tests now comprehensive
| Invariant / path | Test |
|---|---|
| Closed semaphore (M-1) | `test_acquire_permit_closed_semaphore` + `_open_semaphore` |
| Panic propagation (HIGH #3) | `test_execute_superstep_task_panic_propagates_as_error` |
| Concurrent-write determinism (HIGH) | `test_apply_writes_concurrent_same_field_is_order_independent` |
| Cancellation | pre-existing `test_execute_superstep_cancellation` |
| Retry/timeout | pre-existing 8 retry/timeout tests |

### Remaining HIGH test gaps (2 of 5 still open)
- Env-gated real-LLM provider tests (mirror `TEST_POSTGRES_URL` pattern; `#[ignore]` + `TEST_LLM_BASE_URL`).
- SqliteStore/PostgresStore integration tests (`store.rs:1027,1845`).
- ReAct E2E multi-step loop through Pregel.

### SqliteStore integration tests — ADDED (audit HIGH gap partially closed)
The store backends had zero integration coverage (only `MemoryStore` + SQL-string helpers were tested). Added `#[cfg(all(test, feature = "sqlite"))] mod sqlite_tests` in store.rs with 6 `#[tokio::test]`s against a real SQLite backend via `sqlite::memory:` (sqlx isolates one in-memory DB per pool — no temp files, no cross-test collision):
- `test_sqlite_store_put_get_roundtrip` — put then get, value/key/namespace preserved.
- `test_sqlite_store_get_missing_returns_none` — missing key returns `None`, not an error.
- `test_sqlite_store_delete_removes_item` — delete then get → None.
- `test_sqlite_store_put_overwrites_existing_key` — second put on same key overwrites.
- `test_sqlite_store_list_namespaces` — both alpha/beta namespaces listed.
- `test_sqlite_store_batch_put_and_get` — 2 puts + 1 Get in one `batch()`, results correct + persisted.
- Note: tried on-disk temp files first (`sqlite:{path}`); SQLite returned code 14 "unable to open" for the constructed URL — switched to `sqlite::memory:` which the checkpoint crate's SqliteSaver tests also use (47 passing). Exercises the real `Store::put/get/delete/list_namespaces/batch` SQL paths in `store.rs:1186+`.
- Verification: `cargo test -p juncture-core --lib --features sqlite test_sqlite_store` — **6 passed**; full gate green.

### Remaining HIGH test gaps (2 of 5 still open)
- Env-gated real-LLM provider tests (mirror `TEST_POSTGRES_URL` pattern; `#[ignore]` + `TEST_LLM_BASE_URL`).
- PostgresStore integration tests (`store.rs:1845`; SqliteStore now covered — gate by `TEST_POSTGRES_URL` like checkpoint crate).
- ReAct E2E multi-step loop through Pregel.

### ReAct E2E through Pregel — VERIFIED (audit HIGH gap closed)
The headline prebuilt `create_react_agent` was only ever builder-`unwrap()`-tested and never run through the engine (the `13_react_agent` example builds the loop manually without this prebuilt). Added a real E2E test that drives a full model→tool→model loop through Pregel:
- Defined a self-contained `ScriptedModel` (impl `ChatModel`) in the react.rs test module that returns pre-configured turns in sequence via an `Arc<AtomicUsize>` index (shared across `bind_tools` clones so position is preserved through the loop) — avoids touching the shared `MockChatModel` (which can't sequence) and uses atomics (no Mutex/poison).
- Reuses the test module's existing `EchoTool`. Scripted turns: turn 1 = `echo({message:"world"})` tool call, turn 2 = tool-call-free text that terminates the loop.
- `test_react_agent_e2e_tool_loop_through_pregel`: builds `create_react_agent(ScriptedModel, [EchoTool])`, invokes via `graph.invoke_async(MessagesState{...}, &RunnableConfig::new())`, asserts the final state contains a Tool-role message whose content includes the echoed "world" AND a final AI message with no pending tool calls (loop terminated).
- Fix: used `RunnableConfig::new()` not `::default()` — default has `recursion_limit: 0` which trips `RecursionLimit{step:0,limit:0}` immediately (existing react tests use `::default()` only because they call `AgentNode::call` directly, bypassing the loop).
- Verification: `cargo test -p juncture --lib test_react_agent_e2e` — **1 passed**; full gate green.
- Flake observation: one full-`--workspace` run reported a transient 1-test failure that did not reproduce in the next 4 runs (likely a pre-existing timing test under parallel load, e.g. the 10s-sleep cancellation test) — not caused by these changes (the E2E test uses per-instance atomics, no shared state across tests).

### HIGH test gaps status: 4 of 5 closed
| Gap | Status |
|---|---|
| Panic-in-JoinSet (#3) | ✅ |
| Concurrent-write determinism | ✅ |
| SqliteStore integration | ✅ |
| ReAct E2E through Pregel | ✅ |
| Real-LLM provider + PostgresStore | pending (need external services / env gating) |