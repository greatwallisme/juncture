# Juncture Project - Design-to-Code Conformance Review

## Overall Summary

| Module | A (Critical) | B (Major) | C (Minor) | Verdict |
|--------|-------------|-----------|-----------|---------|
| 01 State & Channel | 0 | 3 | 4 | Acceptable |
| 02 Graph Builder | 0 | 3 | 8 | Acceptable |
| 03 Pregel Engine | 3 | 6 | 5 | Needs Work |
| 04 Checkpoint | 3 | 5 | 4 | Needs Work |
| 05 Streaming | 5 | 8 | 3 | Poor |
| 06 HITL | 8 | 7 | 3 | Poor (52%) |
| 07 Subgraph | 4 | 12 | 2 | Poor (~60%) |
| 08 LLM & Tools | 4 | 1 | 0 | Needs Work |
| 09 Observability | 8 | 12 | 5 | Poor |
| 10 Store | 2 | 3 | 2 | Needs Work |
| **TOTAL** | **37** | **60** | **36** | |

Detailed reports: `review/01-state-channel.md` through `review/10-store.md`

---

## Cross-Cutting Issues (Appear in Multiple Modules)

### 1. I/O Schema Separation (Modules 01, 02)
- `IntoState<S>` and `FromState<S>` traits exist in `state/trait_.rs` but have ZERO implementations
- `StateGraph<S>` uses only 1 type parameter instead of 3 (`<S, I=S, O=S>`)
- `CompiledGraph<S>` same issue
- `invoke()` takes `S` directly, not `I`
- **Impact**: Cannot hide private fields or use different input/output schemas

### 2. Functional API Missing (Modules 02, 03)
- Design specifies `#[entrypoint]` and `#[task]` macros
- `SyncAsyncFuture<T>` type referenced but not implemented
- No alternative to StateGraph builder API for function-based workflows

### 3. RetryPolicy/TimeoutPolicy Structs Only (Module 03)
- Struct definitions exist with all fields
- No actual retry/timeout execution logic in Pregel engine
- Nodes never retry or timeout despite having configuration

### 4. Streaming Not Implemented in LLM Providers (Modules 05, 08)
- "Streaming-first" is a core design principle
- Anthropic, OpenAI, Ollama providers return errors or empty streams
- Messages mode cannot function without LLM streaming hooks

### 5. Interrupt Propagation Incomplete (Modules 06, 07)
- Core interrupt mechanism works
- Checkpoint-based interrupt persistence missing
- Subgraph interrupt propagation uses error bubbling, not checkpoint resumption
- update_state() and get_state() missing (needed for HITL workflows)

### 6. MetricsRegistry Mock Implementation (Module 09)
- Uses HashMap instead of OpenTelemetry Meter
- Metrics never exported to OTLP
- Two conflicting implementations (juncture-core vs juncture-tracing)
- Metric name constants defined but never used for actual instrumentation

### 7. Crate Duplication (Module 10)
- juncture-store (standalone) vs juncture-core/src/store.rs
- Standalone is more complete but core has database backends
- API inconsistency, maintenance burden, import confusion

---

## Priority Action Items

### Blockers (Must fix before release)
1. Implement I/O Schema generics on StateGraph/CompiledGraph
2. Wire up interrupt checkpoint persistence and resumption
3. Fix put_writes() to pass actual writes (not empty vec)
4. Implement at least one LLM provider with real streaming
5. Implement RetryPolicy and TimeoutPolicy execution logic
6. Consolidate MetricsRegistry to single OTel-based implementation

### High Priority
7. Implement update_state() and get_state() on CompiledGraph
8. Add juncture.graph.complete/llm.call/tool.call spans
9. Complete SubgraphTransformer namespace transformation
10. Implement subgraph interrupt propagation with checkpoint resumption
11. Add #[subset_of(..)] proc-macro for StateSubset auto-generation
12. Resolve crate duplication for Store

### Medium Priority
13. Add SuperstepEnd to DebugEvent enum
14. Implement GraphCallbackHandler integration with PregelLoop
15. Wire up LlmCachePolicy to RunnableConfig
16. Implement persistence mode behaviors for subgraphs
17. Add TTL support to core Store implementation
18. Fix FilterExpr to use tuple variants and add Not operator
