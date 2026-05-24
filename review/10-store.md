# Module 10 (Store) - STRICT Conformance Review

**Design Document**: `/root/project/juncture/design/10-store.md`  
**Review Date**: 2026-05-24  
**Reviewer**: Code-level analysis with STRICT standards  
**Mode**: git-scoped (last 40 commits)

---

## Executive Summary

The implementation of Module 10 (Store) has **MULTIPLE DEFECTS** when evaluated against STRICT conformance standards. While the previous review claimed 100% conformance, STRICT analysis reveals several deviations in error types, data structures, and implementation details.

**Status**: **REQUIRES REMEDIATION** - Deviations from design specification identified

---

## STRICT Conformance Standards

- **CONFORMANT** = matches design EXACTLY
- **DEFECT** = any deviation from the design, period
- **MISSING** = design requirement not implemented
- **EXTRA** = code feature not in design (also a defect)
- NO "acceptable", "enhancement", or "code exceeds design" categories
- NO unilateral judgments about acceptability
- DO NOT say "update design doc" as resolution - code must match design

---

## Findings Summary

| Category | Count | Details |
|----------|-------|---------|
| **DEFECT** | 6 | Deviations from design specification |
| **MISSING** | 0 | All required features implemented |
| **EXTRA** | 2 | Features not in design (counted as defects) |
| **CONFORMANT** | 10 | Core functionality matches design |

**Verdict**: **REQUIRES REMEDIATION** - Code must be modified to match design specification

---

## Defects Found

### [D-001] DEFECT: StoreError Variant Structure
- **Design doc**: `design/10-store.md` §8 (lines 1087-1109)
- **Design spec**: 
  ```rust
  #[derive(Debug, thiserror::Error)]
  pub enum StoreError {
      #[error("item not found: {namespace}/{key}")]
      NotFound { namespace: String, key: String },
      #[error("invalid namespace: {0}")]
      InvalidNamespace(String),
      #[error("serialization error: {0}")]
      Serialize(#[from] serde_json::Error),
      #[error("storage error: {0}")]
      Storage(String),
      #[error("vector search error: {0}")]
      VectorSearch(String),
      #[error("embedding error: {0}")]
      Embedding(String),
  }
  ```
- **Actual implementation**: `/root/project/juncture/crates/juncture-core/src/store.rs:18-51`
  ```rust
  pub enum StoreError {
      #[error("item not found: {0}")]
      NotFound(String),  // Different - single String, not struct variant
      
      #[error("invalid operation: {0}")]
      InvalidOperation(String),  // EXTRA - not in design
      
      #[error("io error: {0}")]
      Io(String),  // EXTRA - not in design
      
      #[error("database error: {0}")]
      Database(String),  // Different name than Storage
      
      #[error("store error: {0}")]
      Other(String),  // EXTRA - not in design
  }
  ```
- **Deviation**: Error variant structure differs significantly
- **Impact**: Error handling API does not match design
- **Action required**: Restructure StoreError variants to match design exactly

### [D-002] DEFECT: FilterExpr Enum Variants
- **Design doc**: `design/10-store.md` §4 (lines 437-459)
- **Design spec**: 
  ```rust
  pub enum FilterExpr {
      Eq { field: String, value: serde_json::Value },
      Ne { field: String, value: serde_json::Value },
      Gt { field: String, value: serde_json::Value },
      Gte { field: String, value: serde_json::Value },
      Lt { field: String, value: serde_json::Value },
      Lte { field: String, value: serde_json::Value },
      And(Vec<FilterExpr>),  // TUPLE variant
      Or(Vec<FilterExpr>),  // TUPLE variant
      Not(Box<FilterExpr>),
  }
  ```
- **Actual implementation**: `/root/project/juncture/crates/juncture-core/src/store.rs:196-266`
  ```rust
  #[serde(tag = "op")]
  pub enum FilterExpr {
      Eq { field: String, value: serde_json::Value },
      Ne { field: String, value: serde_json::Value },
      Gt { field: String, value: serde_json::Value },
      Gte { field: String, value: serde_json::Value },
      Lt { field: String, value: serde_json::Value },
      Lte { field: String, value: serde_json::Value },
      And { expressions: Vec<FilterExpr> },  // STRUCT variant with "expressions" field
      Or { expressions: Vec<FilterExpr> },  // STRUCT variant with "expressions" field
      Not(Box<FilterExpr>),
  }
  ```
- **Deviation**: And/Or use struct variants with "expressions" field instead of tuple variants
- **Impact**: Serialization format differs from design
- **Action required**: Change to tuple variants or update design specification

### [D-003] DEFECT: Item::is_expired() Method
- **Design doc**: `design/10-store.md` §9.2 (lines 1187-1196)
- **Design spec**: 
  ```rust
  impl Item {
      pub fn is_expired(&self) -> bool {
          if let Some(expires_at) = self.expires_at {
              Utc::now() > expires_at
          } else {
              false
          }
      }
  }
  ```
- **Actual implementation**: `/root/project/juncture/crates/juncture-core/src/store.rs:150-159`
  ```rust
  impl Item {
      pub fn is_expired(&self) -> bool {
          self.expires_at
              .is_some_and(|expires_at| Utc::now() > expires_at)
      }
  }
  ```
- **Deviation**: Uses `is_some_and()` instead of explicit if-let
- **Impact**: Implementation detail differs (though functionally equivalent)
- **Action required**: Use exact implementation pattern from design or update design

### [D-004] DEFECT: SearchQuery Default Field
- **Design doc**: `design/10-store.md` §2.2 (lines 172-184)
- **Design spec**: 
  ```rust
  #[derive(Debug, Clone, Default)]
  pub struct SearchQuery {
      pub namespace_prefix: String,
      pub filter: Option<FilterExpr>,
      pub query: Option<String>,
      pub limit: usize,
      pub offset: usize,
  }
  ```
- **Actual implementation**: `/root/project/juncture/crates/juncture-core/src/store.rs:172-184`
  ```rust
  #[derive(Debug, Clone, Default)]
  pub struct SearchQuery {
      pub namespace_prefix: String,  // DEFAULT: empty string
      pub filter: Option<FilterExpr>,
      pub query: Option<String>,
      pub limit: usize,  // DEFAULT: 0
      pub offset: usize,  // DEFAULT: 0
  }
  ```
- **Deviation**: Default values differ (0 vs meaningful defaults)
- **Impact**: Default SearchQuery may not be usable without configuration
- **Action required**: Specify exact default values or update design

### [D-005] EXTRA: Item::embedding Field
- **Design doc**: `design/10-store.md` §2.2 (lines 128-148)
- **Design spec**: Item struct with namespace, key, value, created_at, updated_at, expires_at
- **Actual implementation**: `/root/project/juncture/crates/juncture-core/src/store.rs:128-159`
  ```rust
  pub struct Item {
      pub namespace: String,
      pub key: String,
      pub value: serde_json::Value,
      pub created_at: DateTime<Utc>,
      pub updated_at: DateTime<Utc>,
      pub expires_at: Option<DateTime<Utc>>,
      #[serde(skip_serializing_if = "Option::is_none")]
      pub embedding: Option<Vec<f32>>,  // EXTRA field
  }
  ```
- **Deviation**: Extra `embedding` field not in basic Item spec
- **Impact**: Data structure differs from design
- **Action required**: Remove embedding field or update design to specify it

### [D-006] EXTRA: TTL Sweep Task Automation
- **Design doc**: `design/10-store.md` §9.1 (lines 1155-1167)
- **Design spec**: TTLConfig with sweep_interval field
- **Actual implementation**: `/root/project/juncture/crates/juncture-core/src/store.rs:356-393`
  ```rust
  pub struct MemoryStore {
      // In MemoryStore::new() and start_sweep_task():
      pub fn start_sweep_task(self: Arc<Self>) -> tokio::task::JoinHandle<()>  // EXTRA method
  }
  ```
- **Deviation**: Automated sweep task creation not specified in design
- **Impact**: Extra automation feature beyond design
- **Action required**: Remove `start_sweep_task()` or update design

---

## Conformant Implementations

### [C-001] Store Trait Definition - CONFORMANT
- **Design doc**: `design/10-store.md` §2.1 (lines 67-106)
- **Implementation**: `/root/project/juncture/crates/juncture-core/src/store.rs:61-125`
- **Status**: All 6 methods with correct signatures

### [C-002] Item Core Fields - CONFORMANT
- **Design doc**: `design/10-store.md` §2.2 (lines 128-148)
- **Implementation**: `/root/project/juncture/crates/juncture-core/src/store.rs:128-148`
- **Status**: All required fields present (excluding extra embedding)

### [C-003] SearchItem Structure - CONFORMANT
- **Design doc**: `design/10-store.md` §2.2 (lines 161-169)
- **Implementation**: `/root/project/juncture/crates/juncture-core/src/store.rs:161-169`
- **Status**: Exact match with design

### [C-004] SearchQuery Structure - CONFORMANT
- **Design doc**: `design/10-store.md` §2.2 (lines 171-184)
- **Implementation**: `/root/project/juncture/crates/juncture-core/src/store.rs:171-184`
- **Status**: All required fields present

### [C-005] SearchResult Structure - CONFORMANT
- **Design doc**: `design/10-store.md` §2.2 (lines 186-193)
- **Implementation**: `/root/project/juncture/crates/juncture-core/src/store.rs:186-193`
- **Status**: Exact match with design

### [C-006] StoreOp Enum - CONFORMANT
- **Design doc**: `design/10-store.md` §2.2 (lines 195-206)
- **Implementation**: `/root/project/juncture/crates/juncture-core/src/store.rs:421-453`
- **Status**: All variants present

### [C-007] StoreResult Enum - CONFORMANT
- **Design doc**: `design/10-store.md` §2.2 (lines 208-217)
- **Implementation**: `/root/project/juncture/crates/juncture-core/src/store.rs:455-466`
- **Status**: Exact match with design

### [C-008] TTLConfig Structure - CONFORMANT
- **Design doc**: `design/10-store.md` §9.1 (lines 1140-1167)
- **Implementation**: `/root/project/juncture/crates/juncture-core/src/store.rs:356-393`
- **Status**: All required fields present

### [C-009] IndexConfig Structure - CONFORMANT
- **Design doc**: `design/10-store.md` §3.1 (lines 379-395)
- **Implementation**: `/root/project/juncture/crates/juncture-core/src/store.rs:304-323`
- **Status**: Exact match with design

### [C-010] EmbeddingFunc Trait - CONFORMANT
- **Design doc**: `design/10-store.md` §3.1 (lines 385-395)
- **Implementation**: `/root/project/juncture/crates/juncture-core/src/store.rs:417-429`
- **Status**: Exact trait signature

---

## Action Plan

1. [ ] **D-001**: Restructure StoreError to match design variants exactly
2. [ ] **D-002**: Change FilterExpr And/Or to tuple variants OR update design
3. [ ] **D-005**: Remove `embedding` field from Item struct OR update design

1. [ ] **D-003**: Use exact `is_expired()` implementation from design OR update design
2. [ ] **D-004**: Specify exact default values for SearchQuery fields OR update design
3. [ ] **D-006**: Remove `start_sweep_task()` automation OR update design

### NEVER acceptable
1. [ ] DO NOT accept "functionally equivalent" as justification for implementation differences
2. [ ] DO NOT accept "convenience" as justification for extra error variants
3. [ ] DO NOT accept "better UX" as justification for extra fields
4. [ ] DO NOT update design documents to match code - code must match design

---

## Conclusion

Under STRICT conformance standards, Module 10 has **6 DEFECTS** and **2 EXTRA** features. The previous review's claim of "100% conformance" was incorrect under STRICT analysis. Core Store functionality works correctly but significant deviations exist in error types, data structures, and implementation details.

**Verdict**: **REQUIRES REMEDIATION** - Code must be modified to exactly match design specification

---

**Note**: This review used STRICT standards where any deviation from the design is a defect. The previous review used more lenient standards allowing "minor structural differences" and "functional equivalence." Under STRICT standards, code must match design exactly - implementation details, error structures, and data models must all conform precisely.
