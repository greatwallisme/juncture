spec: task
name: "store-filter-vector-ttl-conformance"
tags: [design-conformance, store, 10-store]
---

## 意图

将 design `10-store` 的 FilterExpr 查询、向量搜索（cosine 相似度）、TTL 自动过期语义固化为可验证合约，对 `crates/juncture-core/src/store.rs` 做**语义级**核对。

`verify-design-coverage.py` 仅 grep `pub enum FilterExpr`/`fn cosine_similarity`/`fn search` 等符号名（存在即计覆盖），无法验证「$not 取反」、「cosine 相同=1.0/正交=0.0/相反=-1.0」、「搜索结果按相似度降序」、「TTL 过期项被 search 过滤」这类数学/查询行为。本合约绑定既有测试把这类 grep 盲区纳入 lifecycle 强制。

## 约束

<!-- lint-ack: coverage — agent-spec coverage linter 分词器不切中文全角标点，对中文约束的"场景覆盖"机械低估；每条约束均由下方绑定测试覆盖，以 lifecycle 为权威证据。 -->

- `FilterExpr::$not` 必须取反子表达式匹配结果（true↔false）
- `cosine_similarity` 相同向量=1.0、正交向量=0.0、相反向量=-1.0
- 向量搜索结果必须按相似度降序排列
- TTL 过期项：get 返回 None，search 过滤掉过期项
- 不得为通过 lifecycle 而修改实现或放松测试

## 已定决策

- FilterExpr 完整算子集：$eq/$ne/$gt/$gte/$lt/$lte/$and/$or/$not（design §4 H-2）
- 向量搜索用 cosine 相似度；无索引时返回无分数结果
- TTL 惰性清理（lazy cleanup）+ 读时刷新（refresh on read）

## 边界

### Allowed Changes
- specs/store-filter-vector-ttl-conformance.spec.md
- 本合约为只读核实，不改实现代码（store.rs 已有充分测试覆盖）

### Forbidden
- 不得修改 `juncture-core/src/store.rs` 中 FilterExpr / cosine_similarity / search / TTL 的实现逻辑
- 不得改动 Store trait 定义

## 完成条件

### Rule: filter-expr — 查询表达式

场景: $not 取反匹配
  测试:
    包: juncture-core
    过滤: test_filter_not_negates_match
  当 FilterExpr::$not 包裹一个匹配为真的子表达式
  那么 整体匹配结果为假（取反）

场景: 嵌套 $not
  测试:
    包: juncture-core
    过滤: test_filter_nested_not
  当 多层 $not 嵌套
  那么 按层数正确取反（双重否定回到原值）

### Rule: cosine-similarity — 向量相似度数学

场景: 相同向量相似度为 1.0
  测试:
    包: juncture-core
    过滤: test_cosine_similarity_identical_vectors
  当 两个完全相同的向量
  那么 cosine 相似度为 1.0

场景: 正交向量相似度为 0.0
  测试:
    包: juncture-core
    过滤: test_cosine_similarity_orthogonal_vectors
  当 两个正交向量
  那么 cosine 相似度为 0.0

场景: 相反向量相似度为 -1.0
  测试:
    包: juncture-core
    过滤: test_cosine_similarity_opposite_vectors
  当 两个方向相反的向量
  那么 cosine 相似度为 -1.0

### Rule: vector-search — 相似度搜索

场景: 搜索结果按相似度降序
  标签: critical
  测试:
    包: juncture-core
    过滤: test_search_ordering_respects_similarity
  当 对多个向量做相似度搜索
  那么 结果按相似度从高到低排列

### Rule: ttl-expiration — 自动过期

场景: TTL 过期后 get 返回 None
  测试:
    包: juncture-core
    过滤: test_ttl_expiration_on_get
  当 项超过 TTL 时效后 get
  那么 返回 None（已过期）

场景: search 过滤掉过期项
  测试:
    包: juncture-core
    过滤: test_ttl_search_filters_expired
  当 search 时存在已过期项
  那么 过期项被排除出结果（不返回）

## Questions

- [ ] **EmbeddingFunc 后端一致性**：cosine 与搜索语义在 MemoryStore 验证；其他 Store 后端是否一致需额外集成测试。EmbeddingFunc 由调用方注入，其正确性不在本合约范围。（按用户指示推迟到试点结束后统一决策 2026-07-29）
