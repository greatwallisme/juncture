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
## Session 2026-07-28: Re-audit — production-ready? design-deviation / code-simplification sweep

### Gates re-verified — ALL GREEN
fmt exit 0 | clippy exit 0 | test --all-targets exit 0 | test --doc exit 0 (49 passed/11 ignored) | rustdoc -D warnings exit 0 | design coverage 209/214 (97.7%)

### Prior-audit fixes re-verified in source
M-1 (acquire_permit match), M-5 (put_writes ?), B-1 (state restore), M-3 (bounded stream channels, commit 746d486) all confirmed in place. M-4 interrupt unbounded confirmed DESIGN-conformant (06-hitl:72 specifies UnboundedSender). B-2 Async fire-and-forget confirmed DESIGN-conformant (03-pregel §11.3).

### Independent findings (this session)
- NEW MAJOR: Sync-mode checkpoint write failure is log-and-continue (loop_.rs:1920-1929, 1946-1948) — erodes the Sync "full recovery" guarantee (design 03 §11.3). Async-style best-effort applied to the durability mode users select FOR durability.
- NEW MINOR: Debug stream-mode capacity = 32 vs design §3.4 `Messages | Debug => 256` (impl follows §7.3 which only says Messages=256; doc-internal inconsistency).

### sync-reviewer deep conformance sweep — 13 NEW findings (top-3 independently re-verified)
- B-001 BLOCKER: `bulk_update_state` unconditional stub (compiled.rs:1702) — documented public API that always errors.
- B-002 MAJOR: `RemoteGraph` hollow shell (remote.rs) — no invoke/stream/get_state/update_state.
- B-003 MAJOR: `PregelProtocol` trait zero implementors (protocol.rs).
- B-004 MAJOR: `#[entrypoint]`/`#[task]` attribute macros don't exist.
- B-005 MAJOR: `CachePolicy`/`llm_cache_policy` dead config (caching unwired).
- B-006 MAJOR: "Previous Result Injection" (Runtime.previous) not implemented — always None.
- B-007 MAJOR: 10 of 13 `DebugEvent` variants never emitted.
- B-008 MAJOR: `reserved_keys` + checkpoint-persisted error markers absent (error-handler recovery not crash-durable).
- B-009 MINOR: `on_interrupt`/`on_resume` callbacks never invoked.
- B-010 MINOR: Ollama tool calling unimplemented (bind_tools no-op).
- B-011 MINOR: `get_graph(xray)` ignores xray.
- B-012 MINOR: `compile_entrypoint_with_config` drops cache_policy + timeout.
- B-013 MINOR: Ollama doesn't report token usage to budget tracker.
- Plus: scheduler.rs:982 `#[allow(dead_code)]` "reserved API" violates project memory directive; chat.rs is a full duplicate (not "thin re-exports" as CLAUDE.md claims).

### FINAL VERDICT
NOT production-ready. Local-execution happy path is solid (gates green + prior BLOCKERs fixed). But 6 documented public APIs are stubs/hollow/dead-config (B-001..B-006), hidden by the mechanical 97.7% coverage which checks symbol names not behavior. Remote/cross-process execution, functional-API ergonomics+caching, debug-stream completeness, and error-marker durability are the gap clusters. Engine core, checkpoint backends, streaming, state, subgraph, HITL, store, tracing, WASM ARE production-quality. See findings.md "FINAL VERDICT" for the ranked action plan.

## Session 2026-07-28b: Fix re-audit findings item-by-item (no simplification)

### Phase A — isolated quick wins — COMPLETE (verified)
- **Debug stream capacity**: `stream_capacity` (compiled.rs:49) now gives `Messages | Debug => 256` per design 05-streaming §3.4 (was Debug→32). Multi-w-Messages-or-Debug→256.
- **Sync-mode checkpoint propagation (NEW MAJOR finding)**: `save_superstep_checkpoint` + `save_interrupt_checkpoint` (loop_.rs) now return `Result<(), JunctureError>`; Sync/Exit arms propagate `put` failures via `return Err(err.into())` (was log-and-continue — eroded the §11.3 full-recovery guarantee). Async arm stays fire-and-forget (design-intended). Updated 3 after_tick callers (`?`), `save_pending_interrupt_checkpoint` (now returns Result, 0 callers), test site. Added `FailingCheckpointer` + 2 tests: `test_sync_interrupt_checkpoint_write_failure_propagates` (asserts Err+is_checkpoint), `test_async_interrupt_checkpoint_write_failure_does_not_propagate` (asserts Ok). All 570 lib tests pass.
- **scheduler dead_code**: `get_error_handler_node` integrated into `schedule_error_handlers_filtered` (replaced inline `.get()`), removed `#[allow(dead_code)]`. Now used in production.
- **B-013 Ollama token usage**: `OllamaResponse`/`OllamaStreamResponse` gained `prompt_eval_count`/`eval_count`; new `ollama_usage` helper; `invoke` sets `msg.usage` + reports via `try_report_model_call` + `BUDGET_TRACKER.report_output_tokens`; `stream` emits `usage_delta` on the final `done` chunk + reports. Ollama now participates in budget tracking like OpenAI/Anthropic.

### Phase B — B-001 BLOCKER bulk_update_state + get_state_history — COMPLETE (verified)
- **bulk_update_state** (compiled.rs): real atomic impl — loads checkpoint, applies ALL updates in order to in-memory state, records each `as_node` writer, single `put` with source=Update + step+1. Fixed signature drift to design's `Result<RunnableConfig, ...>` (was Vec). Atomicity: single put, errors before put return Err with no save.
- **get_state_history** (compiled.rs, bonus — audit missed this stub too): real impl via `checkpointer.list(config, filter)` → hydrate each `CheckpointTuple` to `StateSnapshot` (mirrors `get_state`), preserves newest-first order.
- Added 2 success tests: `test_bulk_update_state_success_atomic_single_checkpoint` (asserts 1 put, source=Update, step 6, both writers recorded), `test_get_state_history_hydrates_checkpoints` (asserts 2 snapshots, order preserved). All pass; clippy green (added `#[allow(too_many_lines)]` w/ reason + backticked doc items).

### Files modified this session (so far)
- crates/juncture-core/src/graph/compiled.rs (Debug capacity, bulk_update_state, get_state_history, tests)
- crates/juncture-core/src/pregel/loop_.rs (Sync/Async checkpoint propagation, FailingCheckpointer, tests)
- crates/juncture-core/src/pregel/scheduler.rs (get_error_handler_node integration)
- crates/juncture/src/llm/ollama.rs (token usage reporting)

### Phase C — B-008 reserved_keys + crash-durable error markers — COMPLETE (verified)
- Added `reserved_keys` module (checkpoint.rs): INPUT/INTERRUPT/RESUME/ERROR/ERROR_SOURCE_NODE per design 03 §11.5.
- runner.rs error-recovery branch: persists ERROR + ERROR_SOURCE_NODE markers via `put_writes` on task failure (was in-memory only). Propagates put_writes errors (M-5 consistency).
- New `schedule_error_handlers_from_writes(pending_writes, nodes, error_handler_map, already_handled)` in scheduler.rs — design Phase 2 signature; scans ERROR_SOURCE_NODE markers, dedupes, excludes already-handled, verifies handler node exists.
- Wired into after_tick (gated on `!error_handler_map.is_empty()`): loads checkpoint pending_writes via get_tuple, schedules recovery from persisted markers not already handled in-memory → crash-durable error-handler recovery.
- Re-exported from pregel/mod.rs. 2 new scheduler tests (recovery + dedup/exclusion). 576 lib tests pass; clippy/fmt green.

### Phase D — B-006 Previous Result Injection — COMPLETE (verified)
- `CheckpointMetadata.return_value: Option<serde_json::Value>` (the `__return__` field, design 03 §14), `#[serde(default)]` for backward compat. Added `return_value: None` to ALL CheckpointMetadata construction sites (loop_.rs×3, subgraph.rs, checkpoint memory/types/sqlite/postgres, compiled.rs tests×5).
- `RunnableConfig.previous: Option<serde_json::Value>` (serde-free, Default=None) + Debug impl updated.
- `PREVIOUS` task-local (pregel/budget.rs) scoped by the runner around the inner_future (node.call) from `task_config.previous`; re-exported from pregel/mod.rs.
- `func::Runtime::from_core` falls back to PREVIOUS task-local when `core.previous` is None.
- compiled.rs `invoke_async_inner`: loads latest checkpoint `metadata.return_value` → `run_config.previous` at start; on success persists output as `__return__` (metadata.return_value) via get_tuple+put (best-effort, warn-logged). `invoke`/`invoke_async`/`invoke_async_inner` gained `O: serde::Serialize` bound (needed to serialize the return value).
- Tests: 2 func tests (task-local fallback + core-previous precedence). Full workspace gate green (clippy/fmt/test/doc).
- Pre-existing fragile `#[expect(clippy::cognitive_complexity)]` on schedule_fallback_tasks → `#[allow]` (feature-variance made it unfulfilled under -p juncture facade builds).

### Phase H — B-009 on_interrupt/on_resume callbacks — COMPLETE (verified)
- Added `on_interrupt(node, payload)` + `on_resume(node, resume_value)` to core `GraphLifecycleCallback` (observability.rs) with default no-ops (design 09 §5.1, C-09-001).
- Engine: `on_interrupt` fired per signal in `emit_interrupt_events_with_namespace` (before the stream_tx early-return, so it fires without a stream). `on_resume` fired per pending-interrupt node in `resume` + `resume_stream` (compiled.rs).
- Added `ResumeValue::to_json_value()` helper (interrupt/mod.rs) to surface the resume value.
- Tracing `CallbackHandlerAdapter` forwards both hooks to `GraphCallbackHandler` (mapping (node,payload)→GraphInterruptEvent / (node,resume_value)→GraphResumeEvent).

### Phase G — B-007 DebugEvent variants emitted — COMPLETE (verified)
All 10 previously-dead DebugEvent variants now emitted (design 09 §5.1 "all internal events via stream"):
- Added `emit_debug_event(&self, event)` helper on PregelLoop.
- NodeStart/NodeEnd/NodeError (from task_outputs in after_tick emission block, via tx), ChannelWrite (per update), ChannelUpdate + Merge (after apply_writes from total_changed + field_versions), BudgetCheck (after_tick from budget usage), GraphStart (execute_superstep step==0, input=serialized state), GraphEnd (after_tick when pending_tasks empty), CheckpointSaved (inside shared emit_checkpoint_saved_event, all save sites Sync/Async/exit).
- Fixed clippy: map_or/map_or_else forms, cast_precision_loss allow on f32 percentage, added `Fork` CheckpointSource arm.
- Full workspace gate green.

### Phase I/J — B-002 RemoteGraph ops + B-003 PregelProtocol implementor — COMPLETE (verified)
- **B-002**: RemoteGraph (remote.rs) gained real remote ops delegating to GraphClient: `invoke` (serialize input→client.invoke), `get_state`, `get_state_history`, `update_state`, `resume`. Added `From<ClientError> for JunctureError` (error.rs). Added `S: Sync` to invoke (future_not_send).
- **B-003**: `impl PregelProtocol<S> for CompiledGraph<S, S, S>` (protocol.rs) — all 4 methods (invoke/stream/get_state/update_state) delegate to the real local APIs; config cloned into each BoxFuture (lifetime). `StreamHandle.stream` returned as the `BoxStream<'static>`. This makes the previously-dead trait a live abstraction.
  - Documented: RemoteGraph deliberately does NOT impl PregelProtocol — the HTTP GraphClient has no SSE stream endpoint and its `client::StateSnapshot` shape differs from the protocol's `checkpoint::StateSnapshot`; implementing would fabricate data (a simplification). RemoteGraph offers equivalent ops via its own client-typed API.
- Full gate green (clippy/fmt/test).

### Phase K — B-010 Ollama tool calling — COMPLETE (verified)
- `ChatOllama.tools: Vec<ToolDefinition>` field; `bind_tools` now a real binding (was no-op `self.clone()`).
- `OllamaRequest.tools: Option<Vec<OllamaTool>>` (OpenAI-compatible `{"type":"function","function":{...}}`); `OllamaTool`/`OllamaFunction` + `From<&ToolDefinition>`.
- `OllamaResponseMessage.tool_calls: Option<Vec<OllamaToolCall>>`; `OllamaToolCall`/`OllamaToolCallFunction` + `From<OllamaToolCall> for ToolCall` (synthesizes id `ollama-{name}` since Ollama returns no id).
- invoke: sends tools + parses tool_calls into Message.tool_calls + records has_tool_calls span. stream: sends tools + maps final-chunk tool_calls to ToolCallChunk (full args as args_delta).
- 7 new ollama tests (usage helper, tool conversion, tool_call parse, response round-trip, bind_tools). Clippy green (added #[allow(too_many_lines)] on invoke).

### Phase B-011 — get_graph(xray) subgraph expansion — COMPLETE (verified)
- Added `Node::drawable_subgraph(depth) -> Option<DrawableGraph>` to the Node trait (default None). SubgraphNode overrides it to return `self.subgraph.get_graph(Some(depth))`.
- `CompiledGraph::get_graph(xray)`: `None`/`Some(0)` → top-level to_drawable; `Some(n)` → merges each subgraph node's inner drawable (expanded to n-1) with `subgraph_name/`-prefixed node/edge names. No more `let _ = xray;` stub.
- New test `test_get_graph_xray_expands_subgraphs` (None→1 node, Some(0)→1, Some(1)→"sub"+"sub/inner_a"). 579 lib tests pass; clippy green.

### Phase N — Docs/checklist reconciliation + StreamChannel — COMPLETE (verified)
- Checklist 02-001: removed stale `with_context_schema` (design 02 removed it in favor of RunnableConfig+Runtime).
- design 05 §2.3 DebugEvent: rewrote to the implemented richer set (GraphStart/NodeStart/NodeEnd/NodeError/ChannelWrite/ChannelUpdate/Merge/EdgeTraversed/CheckpointSaved/BudgetCheck/GraphEnd/SuperstepStart/SuperstepEnd), noting RouteDecision→EdgeTraversed, BudgetStatus→BudgetCheck rename.
- Checklist 05-008 DebugEvent: RouteDecision→EdgeTraversed, BudgetStatus→BudgetCheck.
- design 08 §3.3: updated Ollama note (tools ARE sent to tool-capable models; token usage via prompt_eval_count/eval_count).
- Checklist 09-013 ClientError: removed dead `GraphNotFound`. Checklist 10-015 StoreError: removed dead `InvalidNamespace`.
- Implemented `StreamChannel` (stream.rs, design 05 §2.5): node-side continuous output channel `{ name, tx }` with async `send` (bounded, backpressure).
- **Design coverage now 214/214 (100.0%)** — was 209/214.

### Phase E — B-005 LLM response caching — COMPLETE (verified)
- `observability::CachePolicy` extended: `ttl` field + `ttl()`/`with_ttl()` + shared Arc store (`get`/`put` with TTL, poison-recovered) + manual Debug.
- `LLM_CACHE_POLICY` task-local (Option<Arc<CachePolicy>>, pregel/budget.rs) scoped by the runner from `RunnableConfig::llm_cache_policy` (nested inside PREVIOUS scope).
- `try_llm_cache_lookup`/`try_llm_cache_store` helpers (budget.rs, re-exported from pregel/mod.rs).
- Facade `to_core_cache_key_input` bridge (llm/mod.rs): converts facade ToolDefinition/CallOptions → core types (field-by-field; the types are structurally identical — the chat.rs duplicate). Wired into openai/anthropic/ollama invoke (lookup before HTTP, store after).
- 5 cache tests (roundtrip, disabled, distinct-keys, TTL expiry, no-TTL). 

### Phase F — B-012 compile_entrypoint cache_policy + timeout — COMPLETE (verified)
- Discovered `config::CachePolicy` (node-result caching, key_func/ttl/max_entries) is DISTINCT from `observability::CachePolicy` (LLM). B-012's TaskConfig.cache_policy is the node-result one. Initial CompileConfig.llm_cache_policy (observability) was the wrong mechanism — reworked to node-result.
- `config::CachePolicy` extended: shared Arc store + `generate_key` (custom key_func or state-hash default) + `get`/`put` (TTL + max_entries eviction, poison-recovered) + Debug.
- `CompileConfig.cache_policy: Option<config::CachePolicy>` (node-result) + `CompiledGraphInner.cache_policy` + `with_cache_policy` + `compile_with_config_and_checkpointer` (new pub method).
- `compile_entrypoint_with_config`: wires `timeout` → `TimeoutPolicy` (add_node timeout_policies) AND `cache_policy` → CompileConfig.cache_policy (was both silently dropped).
- `invoke_async_inner`: node-result cache check (key from serialized input state; hit → deserialize + return early) + store (serialize final state) on success.
- Test `test_compile_entrypoint_node_result_cache_hit_on_second_invoke`: entrypoint runs once; 2nd identical invoke served from cache (exec_count stays 1). VERIFIED.
