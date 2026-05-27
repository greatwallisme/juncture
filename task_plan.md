# Task Plan: Deep-Research Example Deep Analysis & Enhancement

## Goal

Analyze the current `examples/deep-research` against deer-flow and deepagents reference projects, identify gaps in both the example and juncture framework capabilities, and plan a comprehensive enhancement.

## Status: ALL PHASES COMPLETE

---

## Phase 1: Gap Analysis [COMPLETE]

### 1.1 Current Deep-Research Architecture

```
START -> planner -> research_coordinator -> writer -> END
```

- **Planner**: LLM decomposes query into 3-5 sub-tasks (JSON parsing)
- **Coordinator**: `tokio::join_all` runs sub-tasks in parallel, each creates ephemeral `create_react_agent`
- **Writer**: LLM synthesizes findings into report

### 1.2 Reference Project Patterns

#### deer-flow (`create_deerflow_agent`)
- **Middleware chain**: 14+ composable middlewares (DanglingToolCall, ToolErrorHandling, Summarization, Todo, Title, Memory, Vision, SubagentLimit, LoopDetection, Clarification)
- **Subagent delegation**: LLM-driven `task_tool` — the orchestrator agent DECIDES when/what to delegate
- **RuntimeFeatures**: Declarative feature flags (`memory`, `subagent`, `vision`, `summarization`, etc.)
- **Async subagent execution**: Background tasks with polling, cancellation, timeout, token usage tracking
- **Tool management**: Tool groups, skill allowlists, tool deduplication

#### deepagents (`create_deep_agent`)
- **Subagent config**: Simple dict-based subagent definitions with system_prompt + tools
- **think_tool**: Strategic reflection tool for research quality
- **Web content fetching**: Full webpage content (not just snippets)
- **Report workflow**: Plan -> Save request -> Delegate to sub-agents -> Synthesize -> Write report -> Verify
- **Citation system**: Inline [1], [2] format with consolidated Sources section

---

## Phase 2: Juncture Framework Gaps [COMPLETE]

### 2.1 Missing Framework Features (juncture crate)

| Feature | Priority | Description |
|---------|----------|-------------|
| `create_agent` factory | **P0** | Generic agent factory like `create_react_agent` but with middleware support. deer-flow has `create_deerflow_agent`, langchain has `create_agent`. |
| Agent Middleware system | **P0** | Composable middleware chain for agents (pre/post model hooks are insufficient — need full middleware with tool interception, state transformation, error handling). |
| `think_tool` / reflection tool | **P1** | Built-in reflection tool for research-style agents. deepagents uses this for quality control. |
| Subagent delegation via LLM | **P1** | `SubagentTool` exists but needs integration into a higher-level `create_supervisor_agent` or similar. Currently the orchestrator in deep-research uses hardcoded `tokio::join_all`, not LLM-driven delegation. |
| Web content fetcher | **P1** | Tool that fetches full webpage content (not just search snippets). deepagents uses `fetch_webpage_content`. |
| Structured report generation | **P2** | Helper for generating structured reports with citations. |

### 2.2 Missing Example Patterns

| Pattern | Current | Reference |
|---------|---------|-----------|
| **Orchestration** | Fixed pipeline (planner->coordinator->writer) | LLM-driven delegation (orchestrator decides when to delegate) |
| **Reflection** | None | `think_tool` after each search (deepagents) |
| **Sub-agent isolation** | Each sub-task creates ephemeral agent | Sub-agents run in isolated context with own tools/prompts |
| **Memory** | Custom FactStore | ConversationMemory + summarization middleware |
| **Streaming** | None | Real-time output via `stream()` |
| **Error recovery** | None | ToolErrorHandling middleware, retry logic |
| **Loop detection** | None | LoopDetectionMiddleware prevents infinite tool loops |
| **Human-in-the-loop** | PermissionGuard (basic) | Interrupt before tools, clarification middleware |
| **Todo tracking** | None | TodoMiddleware for complex multi-step tasks |
| **Citation system** | Basic URL extraction | Inline [1],[2] with consolidated Sources section |
| **Web content** | Search snippets only | Full webpage content fetch + markdown conversion |

---

## Phase 3: Implementation Plan [COMPLETE]

### 3.1 Framework Enhancements (juncture crate) [COMPLETE]

- [x] `AgentMiddleware` trait with 5 hooks (before_model, after_model, before_tool, after_tool, on_error)
- [x] `create_agent` / `create_react_agent` / `create_agent_with_middleware` factories
- [x] `LoopDetectionMiddleware` + `ToolErrorHandlingMiddleware`
- [x] `ThinkTool` for strategic reflection
- [x] `WebFetchTool` for full webpage content (feature-gated behind `reqwest`)

### 3.2 Deep-Research Example Enhancement [COMPLETE]

- [x] Orchestrator uses `create_agent_with_middleware` with `SubagentTool` + `ThinkTool`
- [x] Researcher sub-agents registered in `AgentRegistry`
- [x] LLM-driven delegation (orchestrator decides WHEN and WHAT)
- [x] Reflection loop via `ThinkTool`
- [x] Middleware chain: `LoopDetectionMiddleware` + `ToolErrorHandlingMiddleware`

---

## Errors Encountered

| Error | Attempt | Resolution |
|-------|---------|------------|
| (none yet) | | |

---

## Decisions

| Decision | Rationale |
|----------|-----------|
| Focus on `create_agent` factory first | All other enhancements depend on middleware system |
| Keep `create_react_agent` as-is | It works well; `create_agent` adds middleware on top |
| Use `SubagentTool` (existing) as base | Already implemented, needs integration into example |
| Defer streaming to Phase 2 | Core architecture change is more important |
