# Findings

## 2026-05-27: Deep-Research Gap Analysis

### deer-flow Key Patterns

1. **`create_deerflow_agent` factory** — Single entry point accepting model, tools, system_prompt, middleware, features, extra_middleware, plan_mode, state_schema, checkpointer, name. Returns `CompiledStateGraph`.

2. **RuntimeFeatures** — Declarative feature flags: `sandbox`, `memory`, `summarization`, `subagent`, `vision`, `auto_title`, `guardrail`, `loop_detection`. Each can be `True`/`False`/custom `AgentMiddleware` instance.

3. **Middleware chain** (14 middlewares):
   - ThreadData → Uploads → Sandbox (infrastructure)
   - DanglingToolCall (always)
   - Guardrail (optional)
   - ToolErrorHandling (always)
   - Summarization (optional)
   - TodoMiddleware (plan_mode)
   - TitleMiddleware (auto_title)
   - MemoryMiddleware (memory)
   - ViewImageMiddleware (vision)
   - SubagentLimitMiddleware (subagent)
   - LoopDetectionMiddleware (loop_detection)
   - ClarificationMiddleware (always last)

4. **`task_tool`** — LLM-driven subagent delegation. Uses `SubagentExecutor` with async background execution, polling (5s intervals), cancellation, timeout, token usage tracking. Supports `general-purpose` and `bash` subagent types.

5. **`@Next`/`@Prev` decorators** — Middleware positioning system for inserting custom middlewares into the chain.

### deepagents Key Patterns

1. **`create_deep_agent`** — Simple factory: model + tools + system_prompt + subagents (list of dicts).

2. **Subagent config dicts**: `{name, description, system_prompt, tools}` — much simpler than deer-flow.

3. **`think_tool`** — Strategic reflection tool. Agent uses after each search to analyze results and plan next steps. Returns "Reflection recorded: {reflection}".

4. **Research workflow**: Plan → Save request → Delegate to sub-agents → Synthesize citations → Write report → Verify.

5. **Web content**: `fetch_webpage_content(url)` fetches full page + converts to markdown via `markdownify`.

6. **Citation format**: Inline [1], [2], [3] with consolidated ### Sources section at end.

### Juncture Current State

1. **`create_react_agent`** — Works well for single-agent ReAct pattern. Supports system_message, max_iterations, interrupt_before_tools, pre/post_model_hook, model_selector, store.

2. **`SubagentTool`** — Exists in `prebuilt/subagent.rs`. Registry-based. Works but not integrated into any example.

3. **`AgentRegistry` / `InMemoryAgentRegistry`** — Implemented and tested.

4. **Missing**: No middleware system, no `create_agent` factory, no think_tool, no web content fetcher, no loop detection.

### Juncture LLM Middleware (Already Exists)

juncture has `LlmMiddleware` trait in `llm/middleware.rs` — wraps `ChatModel::invoke()` with pre/post hooks. Used for logging, metrics, circuit breaker. This is LLM-level middleware only.

**Gap**: deer-flow's agent-level middleware operates at a different layer — intercepting tool calls, transforming state, handling errors across the entire agent loop, not just the LLM call. The two are complementary, not redundant.

### Key Insight

The deep-research example uses a **fixed pipeline** (planner → coordinator → writer), which is fundamentally different from the reference projects where the **LLM drives the orchestration**. The LLM decides when to delegate, what to research, and when to stop. This is the core architectural gap.

The juncture framework already has:
- `SubagentTool` + `AgentRegistry` (in prebuilt/subagent.rs)
- `LlmMiddleware` + `MiddlewareModel` (in llm/middleware.rs)
- `create_react_agent` with hooks (pre/post model, model_selector)

Missing pieces:
- Agent-level middleware (tool interception, state transformation)
- `create_agent` factory that composes agent-level middleware
- `think_tool` for reflection
- Web content fetcher tool
