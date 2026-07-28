# Findings: Production-Readiness Audit (2026-07-27)

## Baseline Quality Gates (verified by running commands)

| Gate | Result | Notes |
|------|--------|-------|
| `cargo build --workspace --all-features` | PASS | Clean build |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | PASS (after fix) | Fixed: removed dead `string_to_string` lint config (line 60, covered by `implicit_clone`/pedantic) |
| `cargo fmt --all -- --check` | PASS | |
| `cargo test --workspace --all-targets --all-features` | PASS | ~1000+ tests, 0 failed (560+261+53+51+18+13+13+12+5+5+4+4...) |
| `cargo doc --workspace --all-features --no-deps` | **FAIL — 62 warnings** | Broken doc links + unclosed HTML tags. Docs.rs would publish broken docs. |

## Dimension 1: Build Quality Gates — MOSTLY PASS
- Build/clippy/fmt/test all green after removing dead lint config.
- **BLOCKER (minor)**: `cargo doc` produces 62 warnings (46 in juncture-core). Categories:
  - Unclosed HTML tags: `T`, `C`, `CheckpointTuple`, `PendingWrite`, `Value` (doc comments with `<T>` not escaped)
  - Unresolved links: `StreamEvent::Custom`, `RunnableConfig`, `JunctureError::node_timeout`, `StateGraph::compile`, `Command`, `TopologyValidator`, `JsonSchema`, `Meter`, `MetricsCollector`, `RetryPolicy`, `StreamConfig`, `web_time::Instant`, `runner::execute_superstep`, `crate::graph::builder::execute_with_retry`, `JunctureError::Checkpoint`, `JunctureError::empty_channel`, `TaskTrigger::Pull`, `ToolsEvent::ToolStarted/Finished`, `StructuredOutputModel`, `EventEmitter::should_emit/with_subgraph_ns`, `MessageStreamMetadata::tags`, `RunnableConfig::with_metrics_collector`, `StreamConfig::output_keys`, `mean_ms`, `juncture_tracing::callback::GraphCallbackHandler`, `DeserializeOwned`
  - Public doc links to private item: `compute_child_namespace` (SubgraphNode doc)
  - Redundant explicit link target

## Dimension 6: Documentation — PARTIAL
- All 7 crates have README.md + LICENSE (Apache-2.0). Root has README.md + README.zh.md. Good.
- 15 numbered examples + deep-research multi-agent app + 3 WASM examples. Strong.
- **No CI** (`.github/workflows/` empty) — no automated quality gate. Production gap.
- **No CHANGELOG, no SECURITY policy** — minor for 0.2.0.
- **62 rustdoc warnings** (see Dimension 1) — published docs would be broken.

## Dimension 10: Design Conformance — 97.7% (209/214)
5 missing items:
1. `StateGraph::with_context_schema` — UNIMPLEMENTED design feature
2. `DebugEvent::RouteDecision`, `DebugEvent::BudgetStatus` — UNIMPLEMENTED variants
3. `StreamChannel` struct — UNIMPLEMENTED entirely
4. `ClientError::GraphNotFound` — INTENTIONALLY removed as dead code (design doc NOT updated)
5. `StoreError::InvalidNamespace` — INTENTIONALLY removed as dead code (design doc NOT updated)
- Items 4-5 are stale design docs (dead code was correctly removed; doc should be updated).
- Items 1-3 are genuinely missing features per design spec.

## Dimension 5: API Stability / Packaging — PARTIAL
- **BLOCKER (packaging)**: `examples/Cargo.toml` (juncture-simple-example) and `examples/deep-research/Cargo.toml` lack `publish = false`. Only `benchmarks/` has it. `cargo publish --workspace` would attempt to publish example crates to crates.io (would fail or pollute registry). Must add `publish = false` to both.
- Workspace version 0.2.0, all crates inherit `version.workspace = true`, `rust-version.workspace = true` (1.87). Good.
- All path deps have version numbers (publishable). Good.
- **Minor**: No `package.metadata.docs.rs` in any crate — docs.rs won't auto-build with `--all-features`; combined with 62 rustdoc warnings, published docs would be broken.

## Dimension 8: Resource Management — PARTIAL (pending rust-expert confirmation)
- Pregel runner (pregel/runner.rs): uses `Semaphore::new(max_parallel_tasks)` for bounded concurrency + `JoinSet` with `shutdown().await` on error paths (lines 693, 701). Cancellation path looks correct.
- **MAJOR (backpressure)**: Streaming output uses `tokio::sync::mpsc::unbounded_channel()` extensively — runtime.rs (×2), graph/compiled.rs (×3), pregel/runner.rs, pregel/loop_.rs (×2), tools/node.rs (×3). Under a slow consumer + high production rate, unbounded buffering → OOM risk. No backpressure on stream channels.
- Timeouts: `NodeTimeoutError` has `RunTimeout`/`IdleTimeout` variants; `execute_with_timeout` exists. Good — timeout support present.

## Dimension 7: Security — unsafe audit
- Only unsafe in `crates/juncture-core/src/wasm_send.rs` — `unsafe impl Send for WasmSend<T>` cfg-gated to `target_family = "wasm"` with complete SAFETY comment (WASM single-threaded). Sound. No other unsafe. Good.

## Versions / Packaging
- Workspace version 0.2.0, all crates inherit `version.workspace = true`, `rust-version.workspace = true` (1.87).
- All path deps have version numbers (for publishing). Good.
- No `publish = false` on examples/benchmarks — need to verify they're not accidentally publishable.

## (Awaiting parallel agents)
(all complete)

## Dimension 2: Error Handling Robustness — NOT READY (Explore audit, 113 non-test files)

**Aggregate (production-only)**: 222 flagged lines — 13 `.unwrap()`, 29 `.expect()`, 2 `unreachable!`, 54 `unwrap_or_default()`, 89 silent `let _ =`, 35 `.ok()`, 13 lock unwraps.
- Per-crate: juncture-core 113 (worst), juncture-telemetry 39, juncture 34, juncture-checkpoint 20, juncture-tracing 14, juncture-derive 2, juncture-store 0.
- **The project's own style guide bans `unwrap()`/`todo!()`/`unimplemented!()` in committed code — 13+29+2 = 44 violations of its own rule.** Not enforced via `clippy::restriction` (`unwrap_used`/`expect_used`/`panic`) in CI.

### Top dangerous occurrences (HIGH — block production)
1. **`pregel/runner.rs:159,653` — silent checkpoint write error swallow (HOT PATH)**: `let _ = cp.put_writes(...)` — same as concurrency M-5. Storage failure silently dropped; resume-from-crash replays wrong state. **Single most dangerous finding in codebase.**
2. **`pregel/runner.rs:387` — `expect()` on semaphore acquire inside spawned task (HOT PATH)**: same as concurrency M-1. Cascading panic storm if PregelLoop dropped early.
3. **`lib.rs:78,96,156,165` — `interrupt!`/`interrupt_with_ctx!` macros panic on non-serializable user payload (HOT PATH, user input)**: `serde_json::to_value(&$payload).expect("interrupt payload must be serializable")`. User-facing HITL entry point crashes worker on `f64::NAN`/non-Serialize. Should return `JunctureError::Serialization`.
4. **`send.rs:47` — `Send` API panics on non-serializable state (user input, dynamic fan-out)**: `serde_json::to_value(send.state).expect("state must be serializable for Send API")`. State types user-defined; serialization failure panics graph.
5. **`chat.rs:489` — empty `OPENAI_API_KEY` silently treated as success (credentials)**: `.unwrap_or_default()` produces client with empty key → confusing 401 at first call instead of construction error. Inconsistent with Anthropic path (`chat.rs:61` uses `.expect`).
6. **`subgraph.rs:327` — subgraph resume silently swallows checkpoint storage error (masks data loss)**: `.ok().flatten()` — DB error becomes "no tuple" → subgraph re-invoked from scratch, silently discarding prior HITL state.

### MEDIUM
7. `openai.rs:99`/`anthropic.rs:100`/`ollama.rs:91` — `reqwest::Client::build().expect()` panics on TLS/proxy/cert init failure (cold init).
8. `loop_.rs:1236` — `u64::try_from(duration.as_millis()).expect()` in stream-emit hot path (same as concurrency m-2).
9. `loop_.rs:812,839` — `unreachable!()` on Result destructuring (same as concurrency m-1).
10. `checkpoint/sqlite.rs:436,738,931`, `postgres.rs:341,656,831` — `i64/u32::try_from(...).expect()` on DB-row/user-controlled values (corrupted/hostile row → panic). Should `map_err(...)?`.
11. `runtime.rs:408,418,428` — `drain_reason.lock().unwrap()` ×3 — lock poison panics in RunControl.
12. `tracing/metrics.rs:184,201,241,258,679` — lock unwraps in metrics registry; one panic disables metrics for process lifetime.
13. `checkpoint/memory.rs:109,123,140` — RwLock poison panics on TTL config.
14. `stream.rs:1065,1087` — `BatchTransformer` buffer lock `expect` on flush hot path.
15. `graph/topology.rs:156,244` — `.expect()` on Tarjan SCC stack/entry_point at compile time.
16. `interrupt/mod.rs:81` — `ResumeValue::Single(values.into_iter().next().unwrap())` brittle on user data.
17. `tools/node.rs:906,923` — `expect` after `is_object` guard in tool arg validation.
18. `llm/retry.rs:239,248` — `RetryingModel` `unwrap()` on its own bookkeeping invariant in retry hot path.
19. `pregel/runner.rs:802` — resume keys non-numeric silently dropped (user gets no malformed-payload feedback).
20. `telemetry/sqlite_store.rs:690-790` — 15× `.ok()`/`unwrap_or_default` on DB row conversion → corrupted row silently becomes `Id::nil()`/empty tags. Observability data loss.

### Systematic categories (lower severity, high volume)
- **F. `state/channel.rs:300,456,669,815,939` — `serde_json::to_value(...).ok()` in channel `checkpoint()`**: non-serializable channel silently excluded from checkpoint — durability gap at a different layer.
- A. `let _ = tx.send(event)` on unbounded broadcast — ~30 occ — by-design (EventEmitter doc says consumers may disconnect). BUT same pattern on bounded `mpsc::Sender::send().await` (`chat.rs:363,376,845,858,1173,1186`) silently swallows `SendError` on backpressure channel — concerning.
- B. `response.text().await.unwrap_or_default()` in Err construction — 15 occ — acceptable (loses detail, still errors).
- C. `unwrap_or_default()` on Option config — many occ — mostly safe; but `telemetry/config.rs:95-96` defaults empty Langfuse creds → silently disables cloud export instead of config error.
- E. `let _ = write!(...)` to String/Formatter — cannot fail — technically rule violation, not bug.

### Verdict: Error handling NOT production-ready
44 direct violations of the project's own no-panic rule in production code. The hot-path panic-on-user-input (interrupt/Send macros) and silent durability failures (put_writes, channel checkpoint, subgraph resume) are the critical blockers. Lock-poisoning panics across runtime/metrics/memory are a systemic concurrency-resilience gap.

## Dimension 4: Test Coverage & Quality — NOT READY (Explore audit)

**Volume**: ~430 inline + 90 integration test fns in juncture-core alone. Strong for pure-logic; **weak/absent for highest-risk production paths.**

### HIGH-risk coverage gaps
1. **No real-LLM or HTTP-mock provider tests** — `ChatAnthropic` has only SSE-parsing tests; `ChatOpenAI.rs` and `ChatOllama.rs` have ZERO inline tests. No `invoke`/`stream`/auth-header/error-mapping tests. No `wiremock`/`httpmock`/`mockito`. **Violates project memory directive** "Must run actual LLM interaction to verify integration." Every provider is one regex/serde mismatch from a runtime failure no test catches.
2. **No `SqliteStore`/`PostgresStore` integration tests** (`store.rs:1027,1845`) — only `MemoryStore` + SQL-string helpers tested. Real `Store::put/get/search/batch` never exercised against a real/in-memory DB. (Contrast: `juncture-checkpoint` does have `TEST_POSTGRES_URL`-gated integration tests — store crate does not follow that pattern.)
3. **No test for task-panic propagation** (`runner.rs:700-706`) — the engine's primary fault-isolation path (`JoinError` → cancel remaining → `JunctureError::execution("Task panicked")`) is entirely untested.
4. **No test pins concurrent-write determinism** (`loop_.rs:1145`) — comment claims "concurrent writes to same field produce deterministic result" but no test spawns multiple tasks writing same field in one superstep. Core Pregel invariant untested.
5. **No E2E test for multi-step ReAct loop** — `create_react_agent` tests (`prebuilt/react.rs:923,932,941`) only `result.unwrap()` the builder; never run model→tool→model across supersteps through Pregel. Headline prebuilt unverified at integration level. Only `fanout_perf_test.rs` is a true engine E2E.

### MEDIUM-risk gaps
6. Wide-state O(n²) perf regression test under-scaled — caps at 10 subjects; original bug manifested at 300-1200 iterations. Wall-time limit (120s/10) would pass with partially-regressed O(n²). (`fanout_perf_test.rs:274`)
7. No property/fuzz tests anywhere (no `proptest`/`quickcheck`/`cargo-fuzz`/`arbitrary`). Channel merge, namespace uniqueness, trigger routing, field_version deltas = state-machine logic that property testing surfaces.
8. No backpressure/semaphore-saturation test — no test spawns > permit count and verifies queueing vs deadlock/oversubscribe.
9. `graph/topology.rs` only 2 inline tests — routing-correctness logic thinly tested.
10. `graph/remote.rs` zero tests — remote-graph feature entirely untested.
11. `core/checkpoint.rs` trait + types (`CheckpointSaver`/`PendingWrite`/`DeltaOp`/`CheckpointTuple`) no unit tests — only transitive via backends.

### LOW-risk gaps
12. `func/mod.rs::compile_entrypoint` tests smoke-only (`result.unwrap()`, never execute entrypoint).
13. `prebuilt/react.rs::test_create_react_agent_*` smoke-only (builder unwrap, no graph run).
14. `tools_integration.rs` lacks `ToolNode` error-path tests (unknown tool, malformed args, tool-Err).
15. `chat.rs` core wrappers only test SSE parsing.

### Weak tests (merely present, not adequate)
- `func/mod.rs:446,457` — `result.unwrap();` only.
- `prebuilt/react.rs:923,932,941` — `result.unwrap();` only.
- `prebuilt/react.rs:824-869` — single `AgentNode::call` smoke tests, no Command/state-transition assertions.
- `runner.rs:941 test_execute_superstep_parallel_tasks` — 3 tasks return `Command::end()`, asserts `len==3`; no ordering/race/shared-field-write/fanout>permits assertions.

### Regression-test discipline (POSITIVE)
Disciplined regression tests for known bugs, referenced by bug-ID doc-comments:
- Fanout Send dedup: `subgraph.rs:602 send_fan_out_produces_unique_namespaces` (10 distinct namespaces).
- state_json deserialization: `fanout_perf_test.rs:119-130` (each subject processed independently).
- Wide-state O(n²): `fanout_perf_test.rs:130-133` (but under-scaled, see #6).
- B-06-006 scratchpad, B-06-003 bubble-up interrupt, B-04-002 superstep checkpoint, B-03-003 durability, B-05-002 stream_data, B-08-001 budget Arc-sharing — all pinned in `loop_.rs`.

### Concurrency test coverage
- Covered: sequential happy path, cancellation mid-superstep, retry-after-transient, retry-skips-Cancelled/Interrupt, timeout-on-slow-node, timeout-wraps-retry.
- NOT covered: task panic in JoinSet, high fanout (100s), backpressure under sustained load, concurrent same-field writes, graceful shutdown mid-superstep with in-flight tasks.

### Verdict: Tests NOT production-ready
Pure-logic core well-tested with disciplined regression practice. But the three highest-risk production surfaces — **real LLM providers, persistent store backends, panic/concurrent-write in parallel runner** — have zero/near-zero coverage. Project memory's own LLM-runtime directive is unmet.

### Clarification on "no real-LLM tests" (2026-07-27, user follow-up)
The finding is accurate for the **automated test suite**, but `.env` IS configured and IS used — by manually-run examples, not by `cargo test`:

- `.env` + `.env.example` document `OPENAI_API_KEY` / `OPENAI_BASE_URL` / `OPENAI_MODEL` for OpenAI-compatible providers.
- `dotenvy` is a dependency ONLY of `examples/` and `examples/deep-research/` — NOT of any library crate. So `cargo test --workspace` never loads `.env`; library tests' `env::var("OPENAI_API_KEY")` always see nothing.
- `examples/src/common.rs::load_llm()` reads all three vars and chains `.with_base_url().with_model()` — `cargo run --example 10_basic_chat` with a configured `.env` makes a REAL LLM call. This honors the memory directive, but as a MANUAL developer activity — not repeatable, not in CI, no regression protection.
- Zero `#[ignore]` tests in the entire repo. Only env-gated integration pattern is `TEST_POSTGRES_URL` (juncture-checkpoint, 5 sites). NO equivalent `TEST_OPENAI_*` / `TEST_LLM_BASE_URL` for LLM providers. `deep-research/tests/integration_tests.rs` uses `MockChatModel` exclusively.

**Related defects uncovered:**
- **API ergonomics gap**: `ChatOpenAI::from_env()` reads ONLY `OPENAI_API_KEY` — ignores `OPENAI_BASE_URL`/`OPENAI_MODEL` that `.env.example` documents and `examples/common.rs` reads. To use a compatible endpoint via the library, caller must manually `.with_base_url().with_model()`.
- **Duplicate + inconsistent `ChatOpenAI`**: facade `crates/juncture/src/llm/openai.rs:136` `from_env() -> Result<Self, LlmError>` (returns Err if key missing — correct semantics, but ignores BASE_URL/MODEL) vs core `crates/juncture-core/src/chat.rs:488` `from_env() -> Self` with `.unwrap_or_default()` (silently produces empty-key client — the error-handling #5 bug). Two types, two semantics — should be unified to the Result-returning variant.

**Recommendation**: add `#[ignore]` env-gated real-LLM integration tests (mirror `TEST_POSTGRES_URL` pattern), make `from_env()` read all three vars (or add `from_env_full()`), unify the two `ChatOpenAI` types, run provider tests in CI with secrets.

## Dimension 3: Concurrency Correctness — NOT READY (rust-expert audit, confirmed by reading code)

### BLOCKERS
- **B-1 State loss on superstep error/cancel** (`pregel/loop_.rs:982,1032`): `execute_superstep` moves `self.state` into Arc via `mem::take`, leaving `self.state = S::default()`. Recovery (`Arc::try_unwrap`) only on success path. The `?` at 1032 returns early on any error (node failure w/o handler, task panic, user cancellation). Result: all state accumulated since last checkpoint is DROPPED. Data-loss bug.
- **B-2 Fire-and-forget async checkpoint saves** (`pregel/loop_.rs:1813,2013`): Under `Durability::Async`, checkpoints saved via bare `tokio::spawn` — never awaited, never tracked in JoinSet, never abortable. Errors only `tracing::warn!`'d (swallowed). Lost on shutdown. No ordering (out-of-order SQL writes corrupt history). B-1 + B-2 together = unrecoverable data-loss pathway: cancel/fail mid-run → state dropped → last checkpoint possibly never persisted.

### MAJORS
- **M-1** `expect()` panic in spawned task on semaphore acquire (`runner.rs:387-390`) — violates zero-`expect` rule; if semaphore ever closed, panics inside spawn → misleading "Task panicked" error.
- **M-2** `max_parallel_tasks == 0` → permanent deadlock (`config.rs:220`, `runner.rs:320`): no validation; `Semaphore::new(0)` → all tasks block forever on `acquire_owned` → superstep hangs with no timeout/error/recovery.
- **M-3** Unbounded stream channel → OOM under slow consumer (`loop_.rs:148`, `compiled.rs:742,1001`): `StreamEvent::Values` clones full state every superstep; slow/stalled consumer → unbounded buffering → OOM.
- **M-4** Unbounded interrupt channel → OOM from buggy/malicious node (`runner.rs:294`): `interrupt!()` macro callable from any node; no bound/backpressure.
- **M-5** `put_writes` errors silently discarded (`runner.rs:159,653`): `let _ = cp.put_writes(...)` — checkpointer failure (DB lost, disk full) silently drops per-task writes; in-memory state updated but checkpoint missing them → re-execution of non-deterministic nodes (LLM/tools) produces different results on recovery. Silent durability divergence.
- **M-6** Partial state mutation if `try_apply` fails mid-`apply_writes` (`scheduler.rs:591`): earlier writes applied, later skipped via `?`; no rollback to pre-superstep snapshot; state left half-merged.

### MINORS
- m-1 `unreachable!()` in production (`loop_.rs:812,839`) — violates no-panic rule.
- m-2 `.expect("duration should fit in u64")` hot path (`loop_.rs:1236`) — inconsistent with adjacent `unwrap_or(u64::MAX)` at 1273.
- m-3 No atomicity for node-internal side effects on cancel (`runner.rs:423,693`) — undocumented.
- m-4 `Relaxed` memory order on budget counters undocumented (`budget.rs:*`).
- m-5 Cancellation masked by error handlers within superstep (`runner.rs:659-696`) — surprising, undocumented.
- m-6 Detached forwarder task, lost panic (`compiled.rs:746,1006`).

### Positives (confirmed)
- Send/Sync: no Rc/RefCell leak; state shared as immutable `Arc<S>`; nodes return `Update`s merged sequentially under single `&mut S`. No data race.
- Permit RAII: permits held as `OwnedSemaphorePermit`, dropped on all paths. No leak.
- JoinSet drop semantics: tasks aborted on drop (B-2 detached spawns are the exception).
- COW state: `CowState<S>` uses `Arc`/`Arc::make_mut` correctly; `Clone` resets pending. Correct.
- Task-locals: `BUDGET_TRACKER`/`INTERRUPT_CONTEXT` properly scoped per task.
- Cancellation propagation: `CancellationToken` cloned into every task, polled via biased `select!`.

**Verdict: Concurrency is NOT production-ready.** B-1+B-2 data-loss pathway must be resolved before any production use. M-2 (zero-permit deadlock) and M-3/M-4 (unbounded OOM) are also production blockers.

## Dimension 9: Observability — STRONG
- `juncture-tracing` crate: full OpenTelemetry integration (callback, config, metrics, propagation, spans, types) — node-level spans, token metrics.
- `juncture-telemetry` crate: Langfuse-compatible (batch_writer, collector, langfuse, otlp, sqlite_store, trace_store, web UI).
- Pregel runner properly instrumented: `info_span!` with `juncture.step` attribute, `.instrument(span)`, output-type + duration recorded in span, `tracing::warn!` on errors. 15 tracing refs in runner.rs, 33 in loop_.rs, 11 in tracing_wasm.rs.
- This is a production strength — observability is first-class.