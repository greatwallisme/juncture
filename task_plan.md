# Task Plan: Fix FactStore Dead Code Integration

## Goal
Remove `#[allow(dead_code)]` annotations from `FactStore` by actually integrating `search_facts` and `store()` methods into the deep-research orchestrator.

## Problem
`examples/deep-research/src/memory/store.rs` has two methods marked as dead code:
1. `search_facts()` - search for existing facts by topic
2. `store()` - access underlying MemoryStore

These methods exist but are never called. This is incomplete implementation, not "reserved API".

## Phases

### Phase 1: Analyze Current Usage [in_progress]
- Read orchestrator.rs to understand current FactStore usage
- Identify where `search_facts` should be called
- Identify where `store()` should be called

### Phase 2: Implement Integration [pending]
- Add logic to search existing facts before research
- Use existing facts to inform/deduplicate research
- Remove `#[allow(dead_code)]` annotations

### Phase 3: Verify [pending]
- Run `cargo clippy -p deep-research --all-targets -- -D warnings`
- Run `cargo test -p deep-research`
- Confirm zero dead_code warnings

## Errors Encountered
| Error | Attempt | Resolution |
|-------|---------|------------|
| (none) | | |
