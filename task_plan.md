# Task Plan: Verify All 39 Review Gaps (Re-Audit)

## Goal
Audit current codebase to verify every gap from the v3 strict re-review has been genuinely resolved.

## Verification Baseline
- Clippy: ZERO warnings, ZERO errors
- Tests: ALL passing (877 passed, 0 failed)
- Build: clean

## Audit Result: 34 PASS / 4 PARTIAL / 1 FAIL

### PASS (34 items)
| # | Gap ID | Description | Evidence |
|---|--------|-------------|----------|
| 1 | M09-A1 | DebugEvent type dedup | Single definition at core/src/stream.rs:208 |
| 2 | M09-A2 | ServerInfo type dedup | Single definition at tracing/src/types.rs:27 |
| 3 | M05-001 | Cancelled event variant | `Cancelled { step: usize }` in StreamEvent |
| 4 | M01-001 | NamedBarrierValue Channel | `NamedBarrierChannel<T,R>` in channel.rs:201 |
| 5 | M05-002 | StreamPart dead code removed | Not found anywhere in codebase |
| 6 | M05-003 | StreamChannel dead code removed | Not found anywhere in codebase |
| 7 | M01-002 | Topic Channel | `TopicChannel<T>` in channel.rs |
| 8 | M01-003 | Delta replay_writes Overwrite | Detects `__overwrite__` wrapper in channel.rs:765-808 |
| 9 | M01-004 | AfterFinish checkpoint is_finished | Saved/restored in checkpoint() methods |
| 10 | M02-001 | Functional API | func/mod.rs: compile_entrypoint, Runtime |
| 11 | M04-001 | TTL checkpoint GC | lazy_cleanup() in memory.rs:122-164 |
| 12 | M04-003 | DeltaSnapshot ancestor walk | recover_from_deltas() in types.rs:95-120 |
| 13 | M05-004 | MessageBatchConfig batching | BatchTransformer in stream.rs:889-947 |
| 14 | M05-005 | with_run_id() | config.rs:202 |
| 15 | M06-001 | InterruptRecord audit trail | interrupt/mod.rs:98-112 |
| 16 | M06-002 | Timestamp in payloads | InterruptSignal.timestamp: DateTime<Utc> |
| 17 | M06-003 | extract_namespace() | interrupt/mod.rs:142 |
| 18 | M06-004 | validate_resume_coverage() | interrupt/mod.rs:203-220 |
| 19 | M06-005 | Scratchpad methods | record_interrupt(), clear_transient() |
| 20 | M07-001 | StateSubset proc-macro | state_derive.rs:322-362 |
| 21 | M07-002 | add_subgraph convenience | builder.rs:870-1060, 3 overloads |
| 22 | M08-001 | Event metadata | ToolStarted.timestamp, ToolFinished.success |
| 23 | M08-002 | StatefulTool lifecycle | emit_tool_started/finished in tools.rs:197-240 |
| 24 | M08-003 | Proper error variants | LlmError::Other uses Box<dyn Error + Send + Sync> |
| 25 | M09-B01 | Graph metrics | inc_counter invocations/errors in compiled.rs:391/418 |
| 26 | M09-B02 | Superstep duration | emit_histogram in loop_.rs:733 |
| 27 | M09-B03 | LLM metrics | tokens/cost/calls/duration in budget.rs |
| 28 | M09-B04 | Tool metrics | calls/errors/duration in budget.rs |
| 29 | M09-B05 | Graph duration histogram | record_histogram in compiled.rs:438-441 |
| 30 | M09-B07 | OpenTelemetry integration | opentelemetry::metrics usage in metrics.rs |
| 31 | M09-B08 | Span attribute recording | span.record() in ollama/anthropic/openai providers |
| 32 | M09-B09 | Metrics testing | test_utils.rs + metrics.rs:749-1063 |
| 33 | M09-B10 | Metrics/budget integration | BudgetTracker.with_metrics_collector() |
| 34 | M09-B12 | MetricsRegistry API | counter/histogram/gauge builders in metrics.rs:532-690 |

### PARTIAL (4 items)
| # | Gap ID | Severity | Issue | Detail |
|---|--------|----------|-------|--------|
| 35 | M10-001 | HIGH | SqliteStore missing vector search | PostgresStore has full vector search (embeddings + cosine similarity). SqliteStore.get()/search() hardcode `embedding: None`, put() ignores `_index` param |
| 36 | M10-002 | HIGH | InjectedStore wrapper missing | Tool trait has requires_store() and invoke_with_store() methods. Missing: InjectedStore<T> declarative wrapper type |
| 37 | M04-002 | MEDIUM | Schema migration no auto-migrate | migrate_checkpoint_schema() exists but only returns error on version mismatch; tells user to call State::migrate() manually |
| 38 | M09-B06 | MEDIUM | Active invocations counter vs gauge | Uses inc_counter(u64::MAX-1) to decrement instead of set_gauge(). Semantically approximates gauge but isn't a true gauge |

### FAIL (1 item)
| # | Gap ID | Severity | Issue | Detail |
|---|--------|----------|-------|--------|
| 39 | M09-B11 | MEDIUM | No distributed trace context propagation | No W3C TraceContext, no propagator, no trace_id injection/extraction found anywhere |

## Remaining Work (5 items)

### HIGH priority (2)
1. M10-001: Implement SqliteStore vector search (embedding storage + cosine similarity)
2. M10-002: Add InjectedStore<T> wrapper type for declarative store injection

### MEDIUM priority (3)
3. M04-002: Add auto-migration or migration utilities to schema version handling
4. M09-B06: Replace counter-based active_invocations with proper set_gauge() calls
5. M09-B11: Add W3C TraceContext propagation for distributed tracing
