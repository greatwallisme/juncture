# Findings: Module 02 Graph Builder Conformance

## Current State
- Branch: master, clean working tree
- Previous session completed Module 01 remediation (commit 4f02b72)

## Key Source Files
- `crates/juncture-core/src/graph/builder.rs` - StateGraph builder, NodeMetadata, RetryPolicy, wrappers (2766 lines)
- `crates/juncture-core/src/graph/topology.rs` - TopologyError, validation (380+ lines)
- `crates/juncture-core/src/command.rs` - Command, Goto, SendTarget, GraphTarget
- `design/02-graph-builder.md` - Design specification

## Remediation Strategy
The implementation is sound and well-tested. Most deviations are valid enhancements over the original design spec. The correct approach is to update the design doc to match the implementation, rather than downgrading the implementation.

### Code Changes Needed
1. **A-001**: Remove no-op `with_context_schema()` method (line 1070-1073) - misleading API

### Design Doc Changes Needed
All A-002, B-001-B-003, C-001-C-005 items require updating the design spec sections to match the actual implementation. The design doc already has "Implementation Note" annotations acknowledging these deviations, but the main spec sections haven't been updated.

## Observations
- StateGraph already has 3 type params (S, I, O). Adding a 4th for context would be extremely invasive.
- Runtime context injection via RunnableConfig is the pragmatic approach for Rust.
- The ErrorHandlerNode/RetryingNode/TimeoutNode wrapper pattern provides composable cross-cutting concerns.
- NodeMetadata struct consolidation is cleaner than 7+ individual parameters.
