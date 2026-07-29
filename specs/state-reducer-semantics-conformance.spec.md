spec: task
name: "state-reducer-semantics-conformance"
tags: [design-conformance, state, reducer, channel, 01-state-channel]
---

## 意图

将 design `01-state-channel` §2.3（Reducer trait）与 §3.6（多写入冲突语义表）的语义要求固化为可验证合约，对 `crates/juncture-core/src/state/channel.rs` 中的 reducer 实现做**语义级**（非符号级）核对。

`scripts/verify-design-coverage.py` 仅 grep 符号名（`pub struct ReplaceReducer` / `LastWriteWinsReducer` / `AnyValueReducer` 等存在即计 216/216），无法区分「多写入 panic」与「多写入允许最后获胜」这类行为差异。本合约绑定真实测试，暴露这些 grep 盲区——尤其 `AnyValueReducer`（仅 re-export、从未被使用或测试，其 debug_assert 等值检查零覆盖）与 `AppendReducer`/`LastWriteWinsReducer`（仅经 Topic/Delta 间接覆盖、无直接单测）。

## 约束

<!-- lint-ack: coverage — agent-spec 的 coverage linter 分词器仅按空格/ASCII 逗号/、/。 切分，不识别中文全角逗号「，」与 Markdown 加粗标记，故中文约束的"场景覆盖"会被机械低估（每条约束实际均由下方绑定测试覆盖，以 lifecycle 8/8 真实测试为权威证据）。用户 2026-07-29 选定方案 A：lifecycle 为权威质量门，lint coverage% 仅作参考，不以改标点/塞通用 token 的方式迎合 linter。 -->

- `ReplaceReducer` 在同一 superstep 收到 ≥2 个写入时必须 panic（不得静默丢弃或任取其一）
- `AppendReducer` 多写入不得 panic，且必须保留写入顺序（associative，分批应用等价于一次性）
- `LastWriteWinsReducer` 多写入不得 panic，最后一个写入获胜
- `AnyValueReducer` 全部相等的多写入不得 panic 且最后获胜；写入值不相等时触发 debug_assert panic
- `Overwrite<T>` 的 serde wire format 必须使用 `__overwrite__` 键（LangGraph checkpoint 兼容）

## 已定决策

- Reducer trait 签名为 `fn reduce(current: &mut T, values: Vec<T>)` + `reduce_one` 单值快速路径（channel.rs:36-47）
- 多写入检测在两个层面：reducer 内部 panic（`ReplaceReducer`，最后防线）+ 引擎 `check_replace_conflicts`（scheduler.rs:726，用 `S::replace_field_indices()` + `S::field_is_set()`）
- `Overwrite<T>` 采用包装类型方案（非 `#[reducer(overwrite)]` 字段属性），serde 自定义 `__overwrite__` 键

## 边界

### Allowed Changes
- specs/state-reducer-semantics-conformance.spec.md
- crates/juncture-core/src/state/channel.rs（仅 `mod tests` 内新增 4 个测试函数，不改实现）

### Forbidden
- 不得修改 `channel.rs` 中任何 reducer / channel 的实现逻辑以通过验证
- 不得移除或弱化 `#[should_panic]` 测试
- 不得改动 `crates/juncture-derive/**`（reducer 代码生成）

## 完成条件

### Rule: replace-single-writer — Replace reducer 禁止多写入

场景: ReplaceReducer 单值替换
  测试:
    包: juncture-core
    过滤: replace_reducer_single_value_succeeds
  当 `ReplaceReducer::reduce` 收到单个写入值 42
  那么 当前值被替换为 42

场景: ReplaceReducer 空值不变
  测试:
    包: juncture-core
    过滤: replace_reducer_empty_values_succeeds
  当 `ReplaceReducer::reduce` 收到空 Vec
  那么 当前值保持不变（99 不被改写）

场景: ReplaceReducer 多写入 panic
  标签: critical
  测试:
    包: juncture-core
    过滤: replace_reducer_multiple_values_panics
  当 `ReplaceReducer::reduce` 在同一 superstep 收到 2 个写入 `[1, 2]`
  那么 panic，消息含 "Replace reducer: multiple writes in same superstep"

### Rule: accumulating-reducers — 累积类 reducer 允许多写入且不 panic

场景: AppendReducer 按序累积且不 panic
  测试:
    包: juncture-core
    过滤: append_reducer_accumulates_in_order
  当 `AppendReducer::reduce` 依次收到 `[[1,2],[3,4]]`，且 `reduce_one` 收到 `[5,6]`
  那么 结果为 `[1,2,3,4,5,6]`，保留写入顺序（associative，分批等价一次性），且多写入不 panic

场景: LastWriteWinsReducer 多写入最后获胜且不 panic
  测试:
    包: juncture-core
    过滤: last_write_wins_reducer_allows_multiple_last_wins
  当 `LastWriteWinsReducer::reduce` 收到 `[1,2,3]`
  那么 结果为 3（最后获胜）且多写入不 panic（区别于 ReplaceReducer）

场景: AnyValueReducer 等值合并最后获胜
  测试:
    包: juncture-core
    过滤: any_value_reducer_equal_values_last_wins
  当 `AnyValueReducer::reduce` 收到全部相等的 `[7,7,7]`
  那么 结果为 7 且多写入不 panic（debug_assert 等值检查通过）

场景: AnyValueReducer 不等值触发 debug_assert
  测试:
    包: juncture-core
    过滤: any_value_reducer_unequal_values_debug_assert_panics
  当 `AnyValueReducer::reduce` 收到不相等的 `[1,2]`
  那么 debug 构建下 panic，消息含 "AnyValue reducer: all values should be equal"

### Rule: overwrite-bypass — Overwrite 绕过 reducer

场景: Overwrite 绕过 reducer 并使用 __overwrite__ wire format
  测试:
    包: juncture-core
    过滤: overwrite_serialize_round_trip
  当 `Overwrite(42)` 序列化再反序列化
  那么 JSON 为 `{"__overwrite__":42}` 且 round-trip 还原内值 42

## Questions

- [ ] **多写入值顺序（注册序 vs 完成序）**：design §2.3/§3.2 明确 reducer 按节点**注册顺序**应用写入以保证确定性；但 `Reducer::reduce` 文档注释（channel.rs:44）写 "Values are provided in the order tasks **completed** (not task spawn order)"。对 associative reducer（Append）无影响，但对非结合 reducer 会产生非确定性。需决策：以注册序为准修正实现/文档，还是接受完成序。（按用户指示推迟到试点结束后统一决策 2026-07-29）
- [ ] **结构化 `InvalidUpdateError::MultipleWriters` 未产出**：design §3.6/§3.9 承诺多写入返回带 `conflicting_nodes` 的结构化错误，但全代码库中 `MultipleWriters` 仅在 enum 定义与一处 doc-comment 出现，**从未被构造**。实际强制依赖 `ReplaceReducer` panic（本合约验证）+ 引擎 `check_replace_conflicts`。需决策：实现结构化错误路径，还是更新 design 移除该承诺。（按用户指示推迟到试点结束后统一决策 2026-07-29）
