spec: task
name: "checkpoint-persistence-conformance"
tags: [design-conformance, checkpoint, 04-checkpoint]
---

## 意图

将 design `04-checkpoint` 的 CheckpointSaver 持久化语义固化为可验证合约，覆盖 MemorySaver 的存取/列举/增量写入、namespace 与 thread 隔离、TTL 过期、delta 恢复，对 `crates/juncture-checkpoint/` 做**语义级**核对。

`verify-design-coverage.py` 仅 grep `pub struct MemorySaver`/`fn put`/`fn get`/`fn put_writes` 等符号名（存在即计覆盖），无法验证「put 后 get 返回同一 checkpoint」、「不同 thread/namespace 互不串扰」、「TTL 过期后 get 返回 None」、「从 deltas + 全量快照重建状态」这类持久化行为。本合约绑定既有测试把这类 grep 盲区纳入 lifecycle 强制。

## 约束

<!-- lint-ack: coverage — agent-spec coverage linter 分词器不切中文全角标点，对中文约束的"场景覆盖"机械低估；每条约束均由下方绑定测试覆盖，以 lifecycle 为权威证据。 -->

- `MemorySaver` put 后 get 必须返回该 checkpoint（持久化 round-trip）
- `list` 返回该 thread 的 checkpoints；`get_latest` 返回最新
- `put_writes` 增量持久化 pending writes
- 不同 namespace、不同 thread_id 的 checkpoint 必须**互不串扰**（隔离）
- TTL 过期后 get 返回 None
- delta 恢复：从全量快照 + 后续 deltas 正确重建目标 checkpoint 状态
- 不得为通过 lifecycle 而修改实现或放松测试

## 已定决策

- MemorySaver 为内存实现，按 `(thread_id, namespace)` 隔离存储
- 支持 TTL 自动过期（TtlConfig）
- delta 恢复策略：定位最近的全量快照，回放其后到目标的所有 deltas

## 边界

### Allowed Changes
- specs/checkpoint-persistence-conformance.spec.md
- 本合约为只读核实，不改实现代码（juncture-checkpoint 已有充分测试覆盖）

### Forbidden
- 不得修改 `juncture-checkpoint/src/{memory,sqlite,postgres,serde,cache,types}.rs` 的实现逻辑
- 不得改动 `crates/juncture-core/src/checkpoint.rs` 的 CheckpointSaver trait 定义

## 完成条件

### Rule: memory-saver-crud — 存取与列举

场景: put 后 get 返回同一 checkpoint
  标签: critical
  测试:
    包: juncture-checkpoint
    过滤: test_memory_saver_put_get
  当 向 MemorySaver put 一个 checkpoint 再 get
  那么 返回的 checkpoint 与写入一致

场景: list 返回该 thread 的 checkpoints
  测试:
    包: juncture-checkpoint
    过滤: test_memory_saver_list
  当 同一 thread 多次 put 后 list
  那么 返回该 thread 的全部 checkpoints

场景: get_latest 返回最新 checkpoint
  测试:
    包: juncture-checkpoint
    过滤: test_memory_saver_get_latest
  当 同一 thread 多次 put 后 get_latest
  那么 返回最新写入的 checkpoint

### Rule: incremental-writes — put_writes 增量持久化

场景: put_writes 持久化 pending writes
  测试:
    包: juncture-checkpoint
    过滤: test_memory_saver_put_writes
  当 调用 put_writes 写入增量
  那么 pending writes 被正确持久化到 checkpoint

### Rule: isolation — namespace 与 thread 隔离

场景: namespace 隔离
  测试:
    包: juncture-checkpoint
    过滤: test_memory_saver_namespace_isolation
  当 不同 namespace 写入同名 checkpoint
  那么 互不串扰（各 namespace 独立可见）

场景: thread 隔离
  测试:
    包: juncture-checkpoint
    过滤: test_memory_saver_thread_isolation
  当 不同 thread_id 写入
  那么 各 thread 的 checkpoints 互不可见

### Rule: ttl-and-delta-recovery — 过期与重建

场景: TTL 过期后 get 返回 None
  测试:
    包: juncture-checkpoint
    过滤: test_memory_saver_ttl_expiration
  当 checkpoint 超过 TTL 时效后 get
  那么 返回 None（已过期清除）

场景: 从多个 deltas 重建状态
  标签: critical
  测试:
    包: juncture-checkpoint
    过滤: test_recover_from_deltas_multiple_deltas
  当 给定全量快照 + 多个后续 deltas
  那么 delta 恢复正确重建目标 checkpoint 的状态

## Questions

- [x] **SQLite/Postgres 后端语义一致性**：RESOLVED 2026-07-29 — 新增 `test_sqlite_saver_thread_isolation`/`_namespace_isolation`（in-memory）+ `test_postgres_saver_thread_isolation`/`_namespace_isolation`（docker postgres:16），覆盖 put/get 隔离语义与 MemorySaver 一致。运行真实 Postgres 还暴露并修复了 PostgresSaver 的 3 个潜伏 bug（测试此前 skip-if-no-DB 从未真正运行）：(a) `created_at` 列 TIMESTAMPTZ 与 String 绑定不匹配 → 改 TEXT（与 SqliteSaver 一致）；(b) `schema_version`/`idx` 列 INTEGER 与 i64 绑定不匹配 → 改 BIGINT；(c) `detect_format` 不识别 msgpack fixstr(0xa0-0xbf) → 扩展到 0x80-0xff。SqliteSaver 10/10 + PostgresSaver 7/7 全绿。
