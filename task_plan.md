# Task Plan: Bilingual Documentation (Chinese + English)

## Goal
Create comprehensive usage documentation for Juncture in both Chinese and English, based on the 15 examples + deep-research + telemetry demo. All documentation saved in `doc/` directory.

## Current Phase
Phase 1: English Documentation

## Phases

### Phase 1: Setup & English Documentation
- [ ] Create `doc/` directory structure with `en/` and `zh/` subdirectories
- [ ] Write `doc/en/getting-started.md` -- installation, first graph, running examples
- [ ] Write `doc/en/core-concepts.md` -- State, StateGraph, Reducers, Edges, Conditional Routing, Pregel engine
- [ ] Write `doc/en/examples-guide.md` -- detailed walkthrough of all examples (01-15, deep-research, telemetry)
- [ ] Write `doc/en/advanced-features.md` -- Streaming, HITL, Checkpointing, Error Recovery, Tools, Structured Output, Telemetry
- **Status:** in_progress

### Phase 2: Chinese Documentation
- [ ] Write `doc/zh/getting-started.md` -- Chinese translation
- [ ] Write `doc/zh/core-concepts.md` -- Chinese translation
- [ ] Write `doc/zh/examples-guide.md` -- Chinese translation
- [ ] Write `doc/zh/advanced-features.md` -- Chinese translation
- **Status:** pending

### Phase 3: Index & Cross-links
- [ ] Write `doc/README.md` -- bilingual index with links to all docs
- [ ] Add links between Chinese and English versions in each doc
- **Status:** pending

## Document Structure

```
doc/
  README.md              -- bilingual index
  en/
    getting-started.md   -- quick start guide
    core-concepts.md     -- framework fundamentals
    examples-guide.md    -- all examples walkthrough
    advanced-features.md -- streaming, HITL, checkpointing, tools, telemetry
  zh/
    getting-started.md   -- quick start guide (Chinese)
    core-concepts.md     -- framework fundamentals (Chinese)
    examples-guide.md    -- all examples walkthrough (Chinese)
    advanced-features.md -- streaming, HITL, checkpointing, tools, telemetry (Chinese)
```

## Content Sources

| Source | What to Extract |
|--------|-----------------|
| `examples/src/01-09*.rs` | Core patterns (state, reducers, routing, chat, tools, streaming, HITL, checkpoint, errors) |
| `examples/src/10-15*.rs` | Real LLM patterns (basic chat, streaming chat, tool calling, react agent, multi-turn, structured output) |
| `examples/deep-research/` | Multi-agent orchestration, subagent delegation, middleware |
| `examples/src/telemetry_demo.rs` | OTel integration, metrics, callbacks |
| `README.md` | Project overview, benchmarks, features list |
| `CLAUDE.md` | Architecture, build commands, workspace structure |
| `examples/CLAUDE.md` | Run commands, examples overview |
