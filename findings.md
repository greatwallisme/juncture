# Findings: Design-to-Code Conformance Review (v3 - Strict Re-review)

## Methodology
Strict code-level re-review by sync-reviewer agents. No deferred items allowed.
Design docs updated to remove all P2/P3/TBD markers - everything is a REQUIREMENT.

## Findings Index
| Module | Review File | Conformance | Gaps | Critical/High |
|--------|-------------|-------------|------|---------------|
| M01 | review/01-state-channel.md | 95% | 4 | 1 HIGH |
| M02 | review/02-graph-builder.md | 92% | 1 | 0 HIGH |
| M03 | review/03-pregel-engine.md | 100% | 0 | 0 |
| M04 | review/04-checkpoint.md | 94.7% | 3 | 0 HIGH |
| M05 | review/05-streaming.md | 88% | 5 | 1 CRITICAL, 3 HIGH |
| M06 | review/06-hitl.md | 75% | 5 | 0 HIGH (5 MEDIUM) |
| M07 | review/07-subgraph.md | 90% | 2 | 0 HIGH |
| M08 | review/08-llm-tools.md | 92% | 3 | 0 HIGH |
| M09 | review/09-observability.md | ~60% | 14 | 2 A-level, 12 B-level |
| M10 | review/10-store.md | ~90% | 2 | 2 HIGH |

## Complete Gap List by Severity (39 items)

### Category A - Technical Direction Deviation (2)
1. M09-A1: DebugEvent type duplication - two different DebugEvent types in different modules
2. M09-A2: ServerInfo type duplication - same type defined twice with slight differences

### CRITICAL (1)
3. M05-001: Missing explicit Cancelled event variant - stream consumers cannot distinguish cancellation from completion

### HIGH (3)
4. M01-001: Missing NamedBarrierValue Channel - wait-all patterns impossible
5. M05-002: StreamPart is dead code - defined but zero usage anywhere
6. M05-003: StreamChannel is dead code - defined but zero usage anywhere
7. M10-001: SQL vector search missing - SqliteStore/PostgresStore hardcoded embedding: None
8. M10-002: Tool InjectedStore missing - no requires_store()/invoke_with_store() on Tool trait

### MEDIUM (19)
9. M01-002: Missing Topic Channel - pub/sub messaging primitive
10. M01-003: Delta Channel replay_writes missing Overwrite detection
11. M01-004: LastValueAfterFinishChannel checkpoint missing is_finished state
12. M02-001: Missing Functional API (@entrypoint, @task decorators)
13. M04-001: TTL checkpoint garbage collection not invoked
14. M04-002: Schema migration logic exists but never called
15. M04-003: DeltaSnapshot recovery - types exist, no ancestor walk algorithm
16. M05-004: MessageBatchConfig defined but batching logic not implemented
17. M05-005: Missing RunnableConfig::with_run_id() for stream resumption API
18. M06-001: Missing InterruptRecord audit trail with timestamps
19. M06-002: Missing timestamp field in interrupt payloads
20. M06-003: Missing extract_namespace() for namespace-based resume
21. M06-004: Missing validate_resume_coverage() for proactive validation
22. M06-005: Incomplete Scratchpad methods (record_interrupt, clear_transient)
23. M07-001: Missing StateSubset proc-macro code generation
24. M07-002: Missing add_subgraph convenience overload
25. M08-001: Missing event metadata (timestamps, success flags)
26. M08-002: Incomplete StatefulTool lifecycle event emission
27. M08-003: Simplified error variants (String instead of boxed trait object)

### M09 Observability Gaps (12)
28. M09-B01: Missing graph-level metrics emissions (invocations, errors)
29. M09-B02: Missing superstep duration metrics
30. M09-B03: Missing LLM metrics (tokens, cost, calls, duration)
31. M09-B04: Missing tool metrics (calls, errors, duration)
32. M09-B05: Missing graph duration histogram
33. M09-B06: Missing gauge metrics entirely (active invocations, budget)
34. M09-B07: Incomplete OpenTelemetry integration
35. M09-B08: Missing span attribute recording
36. M09-B09: No metrics testing infrastructure
37. M09-B10: No metrics/budget system integration
38. M09-B11: Missing context propagation for distributed traces
39. M09-B12: MetricsRegistry API deviation from design

## Comparison: v2 vs v3 Review

| Module | v2 Score | v3 Score | Delta | Notes |
|--------|----------|----------|-------|-------|
| M01 | 85% | 95% | +10% | Previous over-counted gaps |
| M02 | 85% | 92% | +7% | ToolNode and graph export now implemented |
| M03 | 95.3% | 100% | +5% | Confirmed zero gaps |
| M04 | 92% | 94.7% | +2.7% | Stable |
| M05 | 92% | 88% | -4% | Dead code now counted as gaps |
| M06 | 85% | 75% | -10% | Audit trail/timestamps no longer deferred |
| M07 | 85% | 90% | +5% | Fewer gaps than before |
| M08 | 92% | 92% | 0 | Stable |
| M09 | 95% | ~60% | -35% | Metrics emissions all flagged as missing |
| M10 | 94% | ~90% | -4% | SQL vector search no longer deferred |

## Top Priority Fixes

### Must Fix Immediately (blocking production)
1. M09: Implement all missing metrics emissions (LLM, tool, graph, superstep)
2. M05-001: Add Cancelled event variant to StreamEvent
3. M10-001: Implement SQL vector search for SqliteStore/PostgresStore
4. M10-002: Add requires_store()/invoke_with_store() to Tool trait

### Must Fix Before Release
5. M05-002/003: Remove dead code (StreamPart, StreamChannel)
6. M01-001: Implement NamedBarrierValue Channel
7. M06-001-005: Implement HITL audit trail and timestamps
8. M09-A1/A2: Resolve DebugEvent and ServerInfo type duplication

### Should Fix
9. M01-003: Fix DeltaChannel replay_writes Overwrite detection
10. M04-001-003: Implement TTL cleanup, schema migration, delta recovery
11. M07-001: Complete StateSubset proc-macro
12. M08-001-003: Add event metadata, lifecycle events, proper error types
