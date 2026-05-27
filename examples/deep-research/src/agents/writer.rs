//! Writer agent that synthesizes research findings into a final report.

use anyhow::Result;
use juncture::llm::{CallOptions, ChatModel, Message};

use crate::config::ResearchConfig;
use crate::llm::build_model_with_middleware;
use crate::memory::FactStore;
use crate::state::Finding;

/// Synthesize all findings into a comprehensive research report.
///
/// # Arguments
///
/// * `config` - Research configuration
/// * `query` - Original research query
/// * `findings` - Research findings from sub-tasks
/// * `fact_store` - Fact store for archiving research findings
///
/// # Errors
///
/// Returns error if:
/// - LLM API call fails
/// - Response is empty
pub async fn write_report(
    config: &ResearchConfig,
    query: &str,
    findings: &[Finding],
    fact_store: &FactStore,
) -> Result<String> {
    // Build model with middleware chain (logging + circuit breaker)
    let model = build_model_with_middleware(
        config.openai_api_key.clone(),
        config.openai_base_url.clone(),
        &config.model,
    );

    // Build context from findings
    let findings_context = if findings.is_empty() {
        "No findings available.".to_string()
    } else {
        findings
            .iter()
            .enumerate()
            .map(|(i, finding)| {
                format!(
                    "{}. Sub-task: {}\n   Finding: {}\n   Sources: {}\n",
                    i + 1,
                    finding.sub_task,
                    finding.content,
                    finding.sources.join(", ")
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    // Create system prompt for synthesis
    let system_prompt = Message::system(
        "You are a research writer. Synthesize the provided findings into a comprehensive, \
         well-structured report addressing the original query. Organize the information logically, \
         cite sources, and provide a clear conclusion. The report should be informative, accurate, \
         and easy to read.",
    );

    // Create user message with query and findings
    let user_msg = Message::human(format!(
        "Original Query: {query}\n\nResearch Findings:\n{findings_context}\n\nPlease write a comprehensive report.",
    ));

    // Configure call options
    let options = CallOptions {
        max_tokens: Some(3000),
        ..Default::default()
    };

    // Invoke the model
    let response = model
        .invoke(&[system_prompt, user_msg], Some(&options))
        .await
        .map_err(|e| anyhow::anyhow!("LLM invocation failed: {e}"))?;

    // Extract the report content
    let report = response.content_text();

    // Validate report is not empty
    if report.trim().is_empty() {
        return Err(anyhow::anyhow!("LLM returned empty report"));
    }

    // Archive findings as facts for future research
    for finding in findings {
        let fact = juncture::memory::Fact::new(
            query.to_string(),
            finding.content.clone(),
            finding.sources.first().cloned().unwrap_or_default(),
            0.8,
        );
        if let Err(e) = fact_store.save_fact(&fact).await {
            tracing::warn!("Failed to archive finding: {e}");
        }
    }

    Ok(report.to_string())
}

// Rust guideline compliant 2026-05-27
