# Task Plan: Fix Conformance Review Findings

## Goal
Fix A/B findings from conformance review (one at a time via rust-expert) and update design docs with Category C findings.

## Strategy
1. Phase 1: Fix Critical (A) findings -- one per rust-expert agent
2. Phase 2: Fix Major (B) findings -- one per rust-expert agent
3. Phase 3: Update design docs with Category C findings
4. Build/test after every code change

---

## Phase 1: Critical (A) Finding Fixes [in_progress]
Priority order (highest impact first):

1. [A-01-001] Proc-macro try_apply() for multi-write detection (Module 01)
2. [A-01-002] finish_field() implementation (Module 01)
3. [A-01-003] field_versions()/bump_versions() integration (Module 01)
4. [A-02-001] StateGraph I/O Schema generics (Module 02)
5. [A-02-002] add_node() return type doc update (Module 02)
6. [A-02-003] compile() signature doc update (Module 02)
7. [A-02-004] validate_keys() field-level validation (Module 02)
8. [A-03-001] apply_writes merge order in after_tick() (Module 03)
9. [A-04-001] DeltaChannel ancestor walk (Module 04)
10. [A-04-002] CheckpointNamespace separator doc update (Module 04)
11. [A-08-001] StatefulTool trait (Module 08)
12. [A-08-002] ToolRuntime.emit_output_delta() (Module 08)
13. [A-10-001] Store crate duplication (Module 10)
14. [A-10-002] FilterExpr serialization (Module 10)
15. [A-10-003] TTL background sweep (Module 10)
16. [A-10-004] Store Debug bound (Module 10)

## Phase 2: Major (B) Finding Fixes [pending]
32 items total -- to be determined after Phase 1.

## Phase 3: Design Doc Updates (Category C) [pending]
Update all 58 Category C findings into their respective design documents.

## Errors Encountered
| Error | Attempt | Resolution |
|-------|---------|------------|
| (none yet) | | |
