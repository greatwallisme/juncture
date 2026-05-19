# Technical Design Conformance Report

**Project:** Juncture (Rust LangGraph Implementation)
**Audit Date:** 2026-05-19
**Scope:** 6 crates, 96 Rust files, 10 design modules, 214 checklist items

---

## Executive Summary

The audit compared every source file against 10 authoritative technical design documents. Of 214 checklist items:

| Result | Count | Percentage |
|--------|-------|------------|
| PASS | 129 | 60.3% |
| DEVIATION | 53 | 24.8% |
| MISSING | 5 | 2.3% |
| EXTRA | 27 | 12.6% |

**Overall Verdict:** The implementation achieves strong structural conformance (60% exact match) but contains 9 High-severity deviations that render key features non-functional. The workspace structure, trait system, and type hierarchy are well-aligned with design. Critical gaps are concentrated in integration code (LLM providers, streaming, subgraph output, HITL resume) rather than foundational architecture.

### Severity Breakdown

| Severity | Count | Modules Affected |
|----------|-------|------------------|
| High | 9 | 02, 03, 05, 06, 07, 08 |
| Medium | 18 | 01, 02, 03, 04, 05, 06, 07, 08, 09, 10 |
| Low | 26 | 01, 02, 03, 04, 05, 06, 07, 08, 09 |

---

## Module-by-Module Results

### Module 01: State & Channel System (27 items)
- PASS: 16, DEVIATION: 6, MISSING: 1, EXTRA: 5
- Key issues: VersionsSeen uses HashMap (non-deterministic), Overwrite missing custom serde, InvalidUpdateError lacks structured fields

### Module 02: Graph Builder & Compilation (25 items)
- PASS: 17, DEVIATION: 5, MISSING: 2, EXTRA: 3
- Key issues: Functional API entirely missing (#entrypoint/#task macros), validate_keys is no-op

### Module 03: Pregel Execution Engine (25 items)
- PASS: 11, DEVIATION: 9, MISSING: 2, EXTRA: 5
- Key issues: PregelLoop ignores ExecutionContext/ExecutionConfig, execute_superstep missing checkpoint/stream, Send fan-out not implemented

### Module 04: Checkpoint Persistence (28 items)
- PASS: 17, DEVIATION: 5, MISSING: 2, EXTRA: 4
- Key issues: SqliteSaver/PostgresSaver are stubs, CheckpointSaver returns JunctureError not CheckpointError

### Module 05: Streaming System (18 items)
- PASS: 9, DEVIATION: 7, MISSING: 0, EXTRA: 3
- Key issues: StreamEventWriter.send() always errors, EventEmitter.should_emit() skips End event

### Module 06: HITL (13 items)
- PASS: 5, DEVIATION: 7, MISSING: 0, EXTRA: 3
- Key issues: Heartbeat is no-op, should_interrupt missing version-gating, Command<S> missing resume field

### Module 07: Subgraph Composition (10 items)
- PASS: 4, DEVIATION: 6, MISSING: 0, EXTRA: 2
- Key issues: output_map always passes default update (subgraph output broken)

### Module 08: LLM & Tools (36 items)
- PASS: 23, DEVIATION: 9, MISSING: 2, EXTRA: 3
- Key issues: All 3 LLM providers are stubs, tools_condition always returns END

### Module 09: Observability (13 items)
- PASS: 7, DEVIATION: 5, MISSING: 0, EXTRA: 1
- Key issues: MetricsRegistry has no metric creation methods, JunctureClient methods are stubs

### Module 10: Store (15 items)
- PASS: 13, DEVIATION: 1, MISSING: 0, EXTRA: 1
- Key issues: SqliteStore/PostgresStore not implemented

---

## Must-Fix Items (High Severity)

These 9 findings represent features that are architecturally present but functionally broken.

### H1. Subgraph output mapping always passes default update
- **ID:** D-07-5, D-07-6
- **Files:** `crates/juncture-core/src/graph/builder.rs:644-648`, `crates/juncture-core/src/graph/builder.rs:574`
- **Problem:** `output_map` wrapper creates `Sub::Update::default()` ignoring actual subgraph output
- **Impact:** Subgraph composition produces no state changes in parent graph

### H2. All 3 LLM providers are stubs
- **ID:** D-08-1, D-08-2, D-08-3
- **Files:** `crates/juncture-core/src/chat.rs:110-378`
- **Problem:** ChatAnthropic, ChatOpenAI, ChatOllama invoke/stream return empty responses
- **Impact:** No LLM provider works. Any agent requiring model calls is non-functional

### H3. StreamEventWriter.send() always returns error
- **ID:** D-05-5
- **File:** `crates/juncture-core/src/stream.rs:362`
- **Problem:** `send()` is `const fn` that always returns `Err(SendError(event))`
- **Impact:** StreamMode::Custom is non-functional. Nodes cannot emit custom stream events

### H4. EventEmitter.should_emit() omits End event for individual modes
- **ID:** D-05-4
- **File:** `crates/juncture-core/src/stream.rs:304-317`
- **Problem:** Individual modes (Values, Updates, etc.) fall through to `_ => false` for End event
- **Impact:** Single-mode stream consumers cannot detect stream completion

### H5. Heartbeat is no-op (timeout mechanism broken)
- **ID:** D-06-4
- **File:** `crates/juncture-core/src/runtime.rs:163-188`
- **Problem:** `Heartbeat::ping()` does nothing. No UnboundedSender channel.
- **Impact:** idle_timeout cannot detect live nodes. Long-running nodes always appear idle

### H6. should_interrupt missing version-gating
- **ID:** D-06-5
- **File:** `crates/juncture-core/src/interrupt/mod.rs:141-185`
- **Problem:** No channel_versions comparison. Direct node name check only.
- **Impact:** Can cause infinite interrupt loops after checkpoint restore

### H7. Command<S> missing resume field
- **ID:** D-06-7
- **File:** `crates/juncture-core/src/command.rs:7-16`
- **Problem:** No `resume: Option<ResumeValue>` field
- **Impact:** Resume values cannot be passed through Command. Resume flow broken

### H8. tools_condition always returns END
- **ID:** D-08-6
- **File:** `crates/juncture-core/src/tools.rs:411-413`
- **Problem:** `tools_condition()` ignores state, always returns `END`
- **Impact:** ReAct agent routing loop broken. Agents terminate instead of calling tools

### H9. PregelLoop does not integrate checkpoint or streaming
- **ID:** D-03-1, D-03-2
- **Files:** `crates/juncture-core/src/pregel/loop_.rs:25-61`, `crates/juncture-core/src/pregel/runner.rs:58-142`
- **Problem:** No checkpointer field in PregelLoop. No per-task checkpoint/stream emission.
- **Impact:** No crash recovery during superstep. Stream events batched, not per-task

---

## Recommendations

### Priority 1: Fix Broken Core Features

1. **Subgraph output mapping** (H1) -- Replace `Sub::Update::default()` with actual subgraph output extraction. Requires fixing the output_map type signature to receive `Sub::Update` not `&Sub`.

2. **tools_condition** (H8) -- Implement actual state inspection to check if last message has tool_calls. One-line fix that unblocks ReAct agents.

3. **Command<S>.resume** (H7) -- Add `resume: Option<ResumeValue>` field to Command struct and wire it through the Pregel execution path.

4. **should_interrupt version-gating** (H6) -- Add channel_versions parameter and compare against versions_seen before firing interrupt.

### Priority 2: Complete Stub Implementations

5. **LLM Providers** (H2) -- Implement at least one provider (ChatOpenAI recommended as most universal) with actual HTTP calls. Consider using `reqwest` + `eventsource-stream` for SSE.

6. **StreamEventWriter** (H3) -- Add `tx: mpsc::Sender` field and implement real send(). Remove const fn.

7. **EventEmitter.should_emit** (H4) -- Add End event to individual mode match arms.

8. **Heartbeat** (H5) -- Implement with UnboundedSender per design.

### Priority 3: Architectural Alignment

9. **PregelLoop checkpoint integration** (H9) -- Add checkpointer Arc to PregelLoop. Call put_writes after each task in execute_superstep.

10. **VersionsSeen: IndexMap** (D-01-1) -- Switch HashMap to IndexMap for deterministic scheduling.

11. **Overwrite custom serde** (D-01-5) -- Implement `{"__overwrite__": value}` wire format for checkpoint compatibility.

### Priority 4: Missing Features

12. **Functional API** (M-02-1) -- Implement `#[entrypoint]` and `#[task]` proc macros in juncture-derive.

13. **RunControl** (M-03-1) -- Implement graceful shutdown with drain support.

14. **Persistent store backends** (D-10-1) -- Implement SqliteStore with sqlx.

15. **Persistent checkpoint backends** (M-04-2) -- Complete SqliteSaver/PostgresSaver implementations.

---

## Appendix: Checklist Item Status

See `findings.md` for the complete per-item audit with source file references and line numbers for every finding.
