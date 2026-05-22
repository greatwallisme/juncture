# Task Plan: Fix B-08-006 -- ToolCallTransformer and ToolExecutionTrace

## Goal
Wire up `ToolCallTransformer` and `ToolExecutionTrace` in the tool execution path. Both are currently defined but unused in `ToolNode.execute()` / `execute_single_tool()`.

## Files modified
- `crates/juncture/src/tools/node.rs` -- ToolNode, execute(), execute_single_tool(), ToolExecutionTrace

## Phases

### Phase 1: Research [complete]
- Read node.rs, transformer.rs, interceptor.rs, runtime.rs, trait_.rs, error.rs, mod.rs
- Understanding: transformer is stored but never applied; trace struct exists but never instantiated

### Phase 2: Extend ToolExecutionTrace struct [complete]
- Added `input: serde_json::Value`, `output: Option<String>`, `error: Option<String>` fields
- Updated `new()` to accept `input` parameter
- Updated `complete()` to accept `output` and `error`

### Phase 3: Wire up ToolCallTransformer in execute() [complete]
- Transformer applied to cloned tool_call before spawning task in the loop
- Transformer errors handled consistently: returned as tool result when handle_errors=true, propagated when false

### Phase 4: Wire up ToolExecutionTrace in execute_single_tool() [complete]
- Trace created at start of execution with input
- Trace completed after execution with duration, output/error
- Trace logged via tracing::debug! in both success and error paths

### Phase 5: Add tests [complete]
- test_tool_node_with_transformer: verifies transformer modifies arguments before execution
- test_tool_node_with_transformer_error_handling: verifies transformer errors become tool results
- test_tool_node_with_transformer_no_error_handling: verifies transformer errors propagate
- test_tool_execution_trace_with_fields: verifies trace captures input, output, error

### Phase 6: Verify [complete]
- cargo build -p juncture: OK
- cargo clippy -p juncture --all-targets -- -D warnings: OK (zero warnings)
- cargo test -p juncture -- tools --nocapture: OK (69/69 passed)
- Commit: 4843806, 1 file changed, 255 insertions, 10 deletions

## Status
- [x] Phase 1: Research
- [x] Phase 2: Extend ToolExecutionTrace
- [x] Phase 3: Wire up transformer
- [x] Phase 4: Wire up trace
- [x] Phase 5: Add tests
- [x] Phase 6: Verify
