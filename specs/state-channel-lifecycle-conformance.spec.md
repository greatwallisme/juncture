spec: task
name: "state-channel-lifecycle-conformance"
tags: [design-conformance, state, channel, 01-state-channel]
---

## 意图

将 design `01-state-channel` §1.2（UntrackedValue / AfterFinish 变体）、§2.5（consume 步骤）、§3.4（Topic）、§3.5（NamedBarrier）的 channel 生命周期与持久化语义固化为可验证合约，对 `crates/juncture-core/src/state/channel.rs` 的 channel 类型做**语义级**核对。

`verify-design-coverage.py` 仅 grep `pub struct EphemeralChannel`/`UntrackedChannel`/`LastValueAfterFinishChannel`/`TopicChannel`/`NamedBarrierChannel` 等符号名（存在即计覆盖），无法区分「值跨 superstep 保留」与「不被 checkpoint 持久化」、或「值在 finish() 前不可见」这类行为差异。本合约绑定真实测试暴露这些 grep 盲区。

## 约束

<!-- lint-ack: coverage — agent-spec coverage linter 分词器不切中文全角标点，对中文约束的"场景覆盖"机械低估；每条约束均由下方绑定测试覆盖，以 lifecycle 为权威证据。 -->

- `EphemeralChannel` 的值在 consume 后被标记为已消费，且 checkpoint 返回 None（不持久化）
- `UntrackedChannel` 跨 superstep 保留值但 checkpoint 返回 None，from_checkpoint 恢复为 Default
- `LastValueAfterFinishChannel` 在 finish() 调用前 is_available 为假，调用后为真
- `TopicChannel` 累积所有写入并保留顺序，consume 清空并返回是否有内容
- `NamedBarrierChannel` 仅在全部 required source 写入后才 available，未知 source 写入 panic

## 已定决策

- channel 持久化由各类型自实现 `Channel::checkpoint()` / `from_checkpoint()`（channel.rs:498-520）
- ephemeral/untracked/after-finish(non-finished) 的 checkpoint 均返回 None；named-barrier/topic 持久化值
- EphemeralChannel 用 `consumed` 标志位实现 consume 语义（而非真正清零值，由 reset_ephemeral 协同）

## 边界

### Allowed Changes
- specs/state-channel-lifecycle-conformance.spec.md
- 本合约为只读核实，不改实现代码（channel.rs 已有充分测试覆盖）

### Forbidden
- 不得修改 `channel.rs` 中任何 channel 类型的实现逻辑以通过验证
- 不得改动 `crates/juncture-derive/**`

## 完成条件

### Rule: ephemeral-and-untracked — 非持久化 channel

场景: EphemeralChannel consume 标记已消费且不持久化
  测试:
    包: juncture-core
    过滤: ephemeral_channel_consume_tracks_state
  当 `EphemeralChannel` 首次 consume 后再次 consume
  那么 返回值反映 consumed 标志翻转（首次 false、二次 true）

场景: EphemeralChannel checkpoint 返回 None
  测试:
    包: juncture-core
    过滤: ephemeral_channel_checkpoint_is_none
  当 读取 `EphemeralChannel` 的 checkpoint
  那么 返回 None（ephemeral 值不持久化到 checkpoint）

场景: UntrackedChannel 不持久化且恢复为 Default
  测试:
    包: juncture-core
    过滤: untracked_channel_checkpoint_is_none
  当 读取 `UntrackedChannel` 的 checkpoint
  那么 返回 None（跨 superstep 保留但不序列化）

场景: UntrackedChannel from_checkpoint 用 Default 恢复
  测试:
    包: juncture-core
    过滤: untracked_channel_from_checkpoint_uses_default
  当 `UntrackedChannel` 从任意 checkpoint 值恢复
  那么 值为 Default（0），忽略 checkpoint 内容

场景: UntrackedChannel(ReplaceReducer) 多写入 panic
  标签: critical
  测试:
    包: juncture-core
    过滤: untracked_channel_multiple_writes_panics
  当 `UntrackedChannel` 配 `ReplaceReducer` 收到 2 个写入
  那么 panic，消息含 "Replace reducer: multiple writes in same superstep"

### Rule: after-finish-gating — 延迟触发

场景: LastValueAfterFinishChannel finish 前不可用、finish 后可用
  测试:
    包: juncture-core
    过滤: last_value_after_finish_channel_available_after_finish
  当 `LastValueAfterFinishChannel` 调用 finish() 后查询 is_available
  那么 为真（未 finish 时为假，由 not_available_before_finish 测试覆盖语义反面）

### Rule: topic-and-namedbarrier — 累积与屏障

场景: TopicChannel 累积多次写入并保留顺序
  测试:
    包: juncture-core
    过滤: topic_channel_accumulates_messages
  当 `TopicChannel` 两次 update 各写入一批
  那么 get() 返回两批按序拼接的结果

场景: TopicChannel consume 清空并报告是否有内容
  测试:
    包: juncture-core
    过滤: topic_channel_consume_clears_and_returns_status
  当 空 channel consume 与 非空 channel consume
  那么 首次返回 false（无内容）、写入后再 consume 返回 true 且清空

场景: NamedBarrierChannel 全部 source 写入后才 available
  测试:
    包: juncture-core
    过滤: named_barrier_channel_available_after_all_sources_write
  当 `NamedBarrierChannel`（require node_a, node_b）先写 node_a 再写 node_b
  那么 写 node_a 后仍不可用，写 node_b 后 available 且值为最后写入

场景: NamedBarrierChannel 未知 source 写入 panic
  测试:
    包: juncture-core
    过滤: named_barrier_channel_unknown_source_panics
  当 `NamedBarrierChannel` 收到不在 required sources 中的 source 写入
  那么 panic，消息含 "NamedBarrierChannel: source"

## Questions

- [ ] **EphemeralChannel 用 `consumed` 标志而非真正清零值**：design §2.5 描述 ephemeral "consume 清除值（恢复 None/默认）"，但实际实现用 `consumed: bool` 标志 + 由引擎 `reset_ephemeral()`/`consume_field()` 协同清零，consume() 本身只翻转标志。语义等价但实现路径不同，需确认是否符合 design 意图。（按用户指示推迟到试点结束后统一决策 2026-07-29）
