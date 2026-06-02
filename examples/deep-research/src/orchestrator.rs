//! Multi-agent research orchestrator using LLM-driven delegation.
//!
//! This module builds a `ReAct` agent that uses `SubagentTool` to delegate
//! research tasks to specialized sub-agents. The orchestrator decides
//! when to delegate, what to research, and when to stop — replacing the
//! fixed planner → coordinator → writer pipeline with LLM-driven control flow.

use anyhow::Result;
use juncture::prebuilt::{
    AgentConfig, AgentMiddlewareChain, AgentRegistry, LoopDetectionMiddleware, MessagesState,
    ToolErrorHandlingMiddleware, create_agent_with_middleware,
};
use juncture::prebuilt::{InMemoryAgentRegistry, SubagentTool};
use juncture::tools::ThinkTool;
use juncture_core::store::Store;

use crate::config::ResearchConfig;
use crate::llm::build_model_with_middleware;
use crate::memory::FactStore;
use crate::tools::{Calculator, ReadFile, WebSearch};

/// System prompt for the research orchestrator.
const ORCHESTRATOR_SYSTEM_PROMPT: &str = "\
You are a research orchestrator. Your job is to conduct thorough research on the user's query \
by delegating tasks to specialized research sub-agents and synthesizing their findings.

## Workflow

1. **Analyze the query** — Break it into distinct research aspects
2. **Delegate research** — Use the `task` tool to send focused research tasks to sub-agents
3. **Reflect after each delegation** — Use the `think` tool to analyze what you learned and what's still missing
4. **Iterate** — Delegate additional tasks if gaps remain
5. **Synthesize** — Once you have sufficient information, write a comprehensive report

## Delegation Guidelines

- Start with 1-2 sub-agents for the main aspects
- Each sub-agent should receive a focused, specific research task
- Use `think` after receiving sub-agent results to assess quality and gaps
- Don't over-delegate — stop when you have enough to answer comprehensively
- Maximum 3 delegation rounds before synthesizing

## Report Format

When writing the final report:
- Use clear section headings (## for sections, ### for subsections)
- Cite sources inline using [1], [2], [3] format
- End with a ### Sources section listing each numbered source with title and URL
- Write in paragraph form — be thorough, not just bullet points

## Available Tools

- `task` — Delegate research to a sub-agent (provide clear, focused task description)
- `think` — Reflect on progress and plan next steps (use after each delegation)
- `web_search` — Search the web for information directly
- `calculator` — Perform arithmetic calculations
- `read_file` — Read files from the current directory
";

/// System prompt for researcher sub-agents.
const RESEARCHER_SYSTEM_PROMPT: &str = "\
You are a research assistant. Your job is to gather information on the given topic \
using web search and provide comprehensive findings with source citations.

## Instructions

1. Search for relevant information using `web_search`
2. Use `think` after each search to analyze results and identify gaps
3. Continue searching until you have sufficient information
4. Return your findings with inline citations [1], [2], [3]
5. End with a ### Sources section listing each source

## Limits

- Maximum 5 search calls per task
- Stop when you have 3+ relevant sources
- Focus on quality over quantity
";

/// Run the multi-agent research orchestrator.
///
/// # Arguments
///
/// * `config` - Research configuration
/// * `query` - Research query
/// * `_thread_id` - Optional thread ID for checkpointing
///
/// # Errors
///
/// Returns error if graph execution fails.
pub async fn run_research(
    config: &ResearchConfig,
    query: &str,
    _thread_id: Option<&str>,
) -> Result<String> {
    // Build the model with middleware
    let model = build_model_with_middleware(
        config.openai_api_key.clone(),
        config.openai_base_url.clone(),
        &config.model,
    );

    // Create researcher sub-agent graph
    let researcher_model = build_model_with_middleware(
        config.openai_api_key.clone(),
        config.openai_base_url.clone(),
        &config.model,
    );
    let researcher_tools: Vec<Box<dyn juncture::tools::Tool>> = vec![
        Box::new(WebSearch::new(config.tavily_api_key.clone())),
        Box::new(ThinkTool::new()),
    ];
    let researcher_config = juncture::prebuilt::ReactAgentConfig {
        system_message: Some(RESEARCHER_SYSTEM_PROMPT.to_string()),
        max_iterations: Some(8),
        ..Default::default()
    };
    let researcher_graph = juncture::prebuilt::create_react_agent_with_config(
        researcher_model,
        researcher_tools,
        researcher_config,
    )?;

    // Register the researcher sub-agent
    let mut registry = InMemoryAgentRegistry::new();
    registry.register(
        "researcher".to_string(),
        juncture::prebuilt::AgentEntry::from_graph(researcher_graph),
    );

    // Build orchestrator tools
    let fact_store = FactStore::new("research_facts".to_string());

    // Search for existing facts before starting research
    let existing_facts = fact_store.search_facts(query, 5).await.unwrap_or_default();
    tracing::info!(
        "[FactStore] search_facts returned {} results for query: '{}'",
        existing_facts.len(),
        query
    );
    let system_message = if existing_facts.is_empty() {
        ORCHESTRATOR_SYSTEM_PROMPT.to_string()
    } else {
        let facts_context = existing_facts
            .iter()
            .enumerate()
            .map(|(i, fact)| {
                format!(
                    "{}. [{}] (confidence: {}) - {}",
                    i + 1,
                    fact.topic,
                    fact.confidence,
                    fact.claim
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        format!(
            "{ORCHESTRATOR_SYSTEM_PROMPT}\n\n\
            ## Previous Research Context\n\n\
            The following facts were found from previous research sessions on this topic. \
            Use them to avoid duplicate work and build upon existing findings:\n\n\
            {facts_context}"
        )
    };

    let tools: Vec<Box<dyn juncture::tools::Tool>> = vec![
        Box::new(SubagentTool::new(registry)),
        Box::new(ThinkTool::new()),
        Box::new(WebSearch::new(config.tavily_api_key.clone())),
        Box::new(Calculator::new()),
        Box::new(ReadFile::new()),
    ];

    // Build middleware chain
    let middleware = AgentMiddlewareChain::new()
        .with(LoopDetectionMiddleware::new(3))
        .with(ToolErrorHandlingMiddleware::new());

    // Build the orchestrator agent
    let agent_config = AgentConfig {
        system_message: Some(system_message),
        middleware,
        ..Default::default()
    };
    let graph = create_agent_with_middleware(model, tools, agent_config)?;

    // Build initial state
    let initial_state = MessagesState {
        messages: vec![juncture::llm::Message::human(query)],
    };

    // Execute the agent
    let output = graph
        .invoke_async(initial_state, &juncture::RunnableConfig::new())
        .await
        .map_err(|e| anyhow::anyhow!("Agent execution failed: {e}"))?;

    // Extract the final report from the last AI message
    let report = output
        .value
        .messages
        .iter()
        .rev()
        .find(|m| matches!(m.role, juncture_core::state::messages::Role::Ai))
        .map_or_else(
            || "No report generated".to_string(),
            |m| m.content_text().to_string(),
        );

    // Archive facts from the research session
    if let Err(e) = archive_research_facts(&fact_store, query, &report).await {
        tracing::warn!("Failed to archive research facts: {e}");
    }

    Ok(report)
}

/// Archive research facts from the completed session.
async fn archive_research_facts(fact_store: &FactStore, query: &str, report: &str) -> Result<()> {
    // Save the fact
    let fact = juncture::memory::Fact::new(
        query.to_string(),
        format!("Research completed on: {query}"),
        "deep-research-agent".to_string(),
        0.9,
    );
    fact_store.save_fact(&fact).await?;
    tracing::info!("[FactStore] save_fact completed for query: '{}'", query);

    // Save the full report to the store for later retrieval
    let report_key = format!("report:{}", fact.timestamp.timestamp());
    let report_value = serde_json::json!({
        "query": query,
        "report": report,
        "timestamp": fact.timestamp.to_rfc3339(),
    });
    fact_store
        .store()
        .put(fact_store.namespace(), &report_key, report_value, None)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to save report: {e}"))?;
    tracing::info!("[FactStore] store().put() completed, key: '{}'", report_key);

    Ok(())
}

// Rust guideline compliant 2026-05-27
