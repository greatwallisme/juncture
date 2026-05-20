# Module 10: Store - Conformance Review

## Summary
- A findings (Critical): 2
- B findings (Major): 3
- C findings (Minor): 2
- Fully conformant: 14 items

## A Findings (Critical - Missing)

### [A-001] Missing expires_at field in core Item type
- **Design doc**: design/10-store.md S2.2 (line 617)
- **Design spec**: Item struct must include `expires_at: Option<DateTime<Utc>>` for TTL support
- **Actual impl**: crates/juncture-core/src/store.rs:122-133 - missing this field entirely
- **Impact**: TTL feature broken. Item::is_expired() cannot function. Background sweep cannot identify expired items.
- **Note**: The standalone juncture-store crate correctly implements this field, but core does not.

### [A-002] Missing EmbeddingFunc trait
- **Design doc**: design/10-store.md S3.1 (line 326)
- **Design spec**: `EmbeddingFunc` trait with async `embed()` method for vector search
- **Actual impl**: crates/juncture-core/src/store.rs:303-309 - IndexConfig has `fields` but no `embed: Box<dyn EmbeddingFunc>`
- **Impact**: Vector search non-functional as designed.

## B Findings (Major - Partial/Wrong)

### [B-001] Core store missing TTL support entirely
- **Design doc**: design/10-store.md S9.1
- **Design spec**: TTLConfig with default_ttl, refresh_on_read, sweep_interval, sweep_max_items
- **Actual impl**: Core store has no TTLConfig type. Standalone crate has it but core does not.

### [B-002] FilterExpr missing Not operator in core
- **Design doc**: design/10-store.md S4 (line 391)
- **Design spec**: `Not(Box<FilterExpr>)` variant for logical negation
- **Actual impl**: crates/juncture-core/src/store.rs:169-233 - no Not variant

### [B-003] FilterExpr And/Or struct variant vs tuple variant
- **Design doc**: design/10-store.md S4
- **Design spec**: `And(Vec<FilterExpr>)` tuple variant
- **Actual impl**: crates/juncture-core/src/store.rs:221-233 - `And { expressions: Vec<FilterExpr> }` struct variant

### Architecture: Crate Duplication
Two separate Store implementations exist:
1. `juncture-store` standalone crate - more complete, correct expires_at, EmbeddingFunc, TTL, tuple variants
2. `juncture-core/src/store.rs` - less complete, missing features, struct variants

This creates API inconsistency, maintenance burden, and import confusion.

## C Findings (Minor)

### [C-001] Missing serde(tag = "op") on standalone FilterExpr
- Serialization format inconsistency with design

### [C-002] Core Store trait missing Debug bound
- Inconsistency: core claims Debug impossible, standalone successfully implements it

## Verified Items
1. Store trait methods (get/put/delete/search/list_namespaces/batch) - all 6 correct
2. Item basic fields (namespace, key, value, created_at, updated_at)
3. SearchQuery and SearchResult structures
4. StoreOp/StoreResult enums - all variants correct
5. FilterExpr comparison operators (Eq, Ne, Gt, Gte, Lt, Lte)
6. FilterExpr logical operators (And, Or) - correct semantics, wrong variant style
7. MemoryStore basic structure with Arc<RwLock<HashMap>>
8. TTLConfig fields in standalone crate
9. Item::is_expired() in standalone crate
10. FilterExpr::matches() evaluation engine in standalone
11. TTL sweep task in standalone MemoryStore
12. Namespace separator convention ("/")
13. Batch operations
14. Standalone crate test coverage (11 test functions)

## Verdict: Requires Remediation
Critical gaps in TTL and vector search. Two-crate duplication must be resolved.
