//! Researcher agent that executes individual sub-tasks.

use anyhow::Result;
use juncture::llm::Message;
use juncture::prebuilt::{MessagesState, ReactAgentConfig, create_react_agent_with_config};
use regex::Regex;
use std::collections::HashSet;

use crate::config::ResearchConfig;
use crate::llm::build_model_with_middleware;
use crate::memory::{FactStore, ResearchFactExtractor};
use crate::state::{Finding, SubTask};
use crate::tools::{Calculator, ReadFile, MemorySearch, WebSearch};

/// Execute a single research sub-task using an ephemeral agent.
///
/// # Arguments
///
/// * `config` - Research configuration
/// * `sub_task` - The sub-task to research
/// * `fact_store` - Fact store for persisting extracted facts
///
/// # Errors
///
/// Returns error if:
/// - LLM API call fails
/// - Tool execution fails
/// - Agent execution fails
pub async fn research_sub_task(
    config: &ResearchConfig,
    sub_task: &SubTask,
    fact_store: &FactStore,
) -> Result<Finding> {
    // Build model with middleware chain (logging + circuit breaker)
    let model = build_model_with_middleware(
        config.openai_api_key.clone(),
        config.openai_base_url.clone(),
        &config.model,
    );

    // Create tools for the agent
    let tools: Vec<Box<dyn juncture::tools::Tool>> = vec![
        Box::new(WebSearch::new(config.tavily_api_key.clone())),
        Box::new(Calculator::new()),
        Box::new(ReadFile::new()),
        Box::new(MemorySearch::new(Some(std::sync::Arc::clone(
            fact_store.store(),
        )))),
    ];

    // Build react agent config
    let agent_config = ReactAgentConfig {
        system_message: Some(
            "You are a research assistant. Use web_search to find current information \
             related to the sub-task. Provide a comprehensive answer with citations \
             to sources."
                .to_string(),
        ),
        max_iterations: Some(5),
        ..Default::default()
    };

    // Build react agent graph
    let graph = create_react_agent_with_config(model, tools, agent_config)?;

    // Build initial messages with the sub-task
    let initial_state = MessagesState {
        messages: vec![Message::human(&sub_task.description)],
    };

    // Execute the agent
    let output = graph
        .invoke(initial_state, &juncture::RunnableConfig::default())
        .map_err(|e| anyhow::anyhow!("Research agent execution failed: {e}"))?;

    // Extract the last AI message
    let last_message = output
        .value
        .messages
        .iter()
        .filter(|msg| matches!(msg.role, juncture_core::state::Role::Ai))
        .next_back()
        .ok_or_else(|| anyhow::anyhow!("No assistant message in research output"))?;

    // Extract the content
    let content = match &last_message.content {
        juncture::llm::Content::Text(text) => text.clone(),
        juncture::llm::Content::MultiPart(parts) => {
            // Concatenate text parts from multimodal content
            parts
                .iter()
                .filter_map(|part| {
                    if let juncture::llm::ContentPart::Text { text } = part {
                        Some(text.as_str())
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
                .join("\n")
        }
    };

    // Extract sources from messages (look for URLs in web_search tool calls)
    let sources = extract_sources(&output.value.messages);

    let finding = Finding {
        sub_task: sub_task.description.clone(),
        content,
        sources,
    };

    // Extract facts from finding using ResearchFactExtractor
    let extractor_model = build_model_with_middleware(
        config.openai_api_key.clone(),
        config.openai_base_url.clone(),
        &config.model,
    );
    let extractor = ResearchFactExtractor::new(extractor_model);
    let facts = extractor.extract_from_finding(&finding).await;
    for fact in &facts {
        if let Err(e) = fact_store.save_fact(fact).await {
            tracing::warn!("Failed to save extracted fact: {e}");
        }
    }

    Ok(finding)
}

/// Extract source URLs from conversation messages.
///
/// Looks for tool use messages that might contain URLs or references.
fn extract_sources(messages: &[Message]) -> Vec<String> {
    let mut sources = HashSet::new();
    let url_pattern = Regex::new(r#"https?://[^\s"'<>]+"#).map_err(|_e| ()).ok();

    for msg in messages {
        // Check tool calls in the message
        for tool_call in &msg.tool_calls {
            if tool_call.name == "web_search" {
                // Try to extract URL from the arguments
                if let Ok(input_str) = serde_json::to_string(&tool_call.arguments)
                    && let Some(re) = &url_pattern
                {
                    for cap in re.captures_iter(&input_str) {
                        if let Some(url) = cap.get(0) {
                            sources.insert(url.as_str().to_string());
                        }
                    }
                }
            }
        }
    }

    sources.into_iter().collect()
}

// Rust guideline compliant 2026-05-27
