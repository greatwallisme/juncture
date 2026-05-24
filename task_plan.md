# Task Plan: Module 02 Graph Builder Conformance Remediation

## Goal
Fix all defects identified in `review/02-graph-builder.md` to achieve CONFORMANT status.

## Findings from Review
- 2 technical direction deviations (A-001, A-002)
- 3 feature simplifications (B-001, B-002, B-003)
- 5 extra features not in design (C-001 through C-005)
- 8 fully conformant items

## Phases

### Phase 1: Fix A-001 with_context_schema() no-op (CRITICAL)
**Status**: pending
**Action**: Remove the no-op method. The design doc section 3.5 already acknowledges runtime injection via RunnableConfig. The method provides no value and misleads users.
**Files**: `crates/juncture-core/src/graph/builder.rs`
**Design update**: Update design section 3.5 to clarify context injection is runtime-only via RunnableConfig, not compile-time type change.

### Phase 2: Update design doc for A-002 ErrorHandlerNode wrapper
**Status**: pending
**Action**: Update design section 2.4 to formally specify the ErrorHandlerNode wrapper pattern instead of direct registration.
**Files**: `design/02-graph-builder.md`

### Phase 3: Update design doc for B-001 NodeMetadata consolidation
**Status**: pending
**Action**: Update design section 1 to specify NodeMetadata struct instead of individual parameters.
**Files**: `design/02-graph-builder.md`

### Phase 4: Update design doc for B-002 TimeoutNode wrapper
**Status**: pending
**Action**: Add TimeoutNode wrapper specification to design section 2.4.
**Files**: `design/02-graph-builder.md`

### Phase 5: Update design doc for B-003 RetryPolicy extra fields
**Status**: pending
**Action**: Update design section 1 to specify complete RetryPolicy with backoff_factor, max_interval, jitter, retry_on.
**Files**: `design/02-graph-builder.md`

### Phase 6: Update design doc for C-001 through C-005
**Status**: pending
**Action**: Add to design doc:
- C-001: CompileConfig struct and compile_with_config() in section 1
- C-002: Extra TopologyError variants in section 5.2
- C-003: Extra compile() method variants in section 1
- C-004: Command.stream_data field in section 4.2
- C-005: SendTarget.timeout field in section 4.2
**Files**: `design/02-graph-builder.md`

### Phase 7: Update review file to mark all items resolved
**Status**: pending
**Action**: Update `review/02-graph-builder.md` to reflect remediation status.
**Files**: `review/02-graph-builder.md`

### Phase 8: Verify - build and test
**Status**: pending
**Action**: Run `cargo build`, `cargo test`, `cargo clippy` to ensure zero warnings/errors.

## Errors Encountered
| Error | Attempt | Resolution |
|-------|---------|------------|
