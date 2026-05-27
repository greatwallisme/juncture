//! Planner agent that decomposes research queries into sub-tasks.

use anyhow::Result;
use futures::future::{BoxFuture, FutureExt};
use juncture::llm::{CallOptions, ChatModel, Message};
use juncture_core::error::JunctureError;
use juncture_core::node::NodeFnUpdate;
use serde::Deserialize;

use crate::config::ResearchConfig;
use crate::llm::build_model_with_middleware;
use crate::memory::FactStore;
use crate::state::{ResearchState, ResearchStateUpdate, SubTask, TaskStatus};

/// Helper struct for deserializing LLM planning responses.
#[derive(Deserialize)]
struct PlanResponse {
    /// Sub-tasks decomposed from the research query.
    sub_tasks: Vec<SubTaskData>,
}

/// Sub-task data from LLM response.
#[derive(Deserialize)]
struct SubTaskData {
    /// Description of the sub-task.
    description: String,
}

/// Create a planner node that decomposes queries into sub-tasks.
///
/// # Arguments
///
/// * `config` - Research configuration containing LLM settings
/// * `fact_store` - Fact store for retrieving prior research context
#[must_use]
pub fn plan_research_node(
    config: ResearchConfig,
    fact_store: FactStore,
) -> NodeFnUpdate<
    impl Fn(&ResearchState) -> BoxFuture<'static, Result<ResearchStateUpdate, JunctureError>>
    + Clone
    + Send
    + Sync
    + 'static,
> {
    NodeFnUpdate(move |state: &ResearchState| {
        let config = config.clone();
        let query = state.query.clone();
        let fact_store = fact_store.clone();
        async move {
            plan_research(&config, &query, &fact_store)
                .await
                .map_err(|e| JunctureError::execution(e.to_string()))
        }
        .boxed()
    })
}

/// Decompose a research query into 3-5 sub-tasks using the LLM.
///
/// # Arguments
///
/// * `config` - Research configuration
/// * `query` - The research query to decompose
/// * `fact_store` - Fact store for retrieving prior research context
///
/// # Errors
///
/// Returns error if:
/// - LLM API call fails
/// - Response parsing fails
async fn plan_research(
    config: &ResearchConfig,
    query: &str,
    fact_store: &FactStore,
) -> Result<ResearchStateUpdate> {
    // Build model with middleware chain (logging + circuit breaker)
    let model = build_model_with_middleware(
        config.openai_api_key.clone(),
        config.openai_base_url.clone(),
        &config.model,
    );

    // Search for relevant prior facts
    let prior_facts = fact_store
        .search_facts(query, 5)
        .await
        .unwrap_or_default();

    let prior_context = if prior_facts.is_empty() {
        String::new()
    } else {
        let facts_text = prior_facts
            .iter()
            .map(|f| {
                format!(
                    "- {} (source: {}, confidence: {:.1})",
                    f.claim, f.source, f.confidence
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        format!("\n\nPrior research context:\n{facts_text}")
    };

    // Create system prompt for planning
    let system_prompt = Message::system(
        "You are a research planner. Decompose the user's research query into 3-5 \
         specific sub-tasks. Each sub-task should be actionable and focused on a \
         particular aspect of the research. Return a JSON object with a 'sub_tasks' \
         array containing objects with 'description' fields. Output ONLY the JSON, \
         no other text.",
    );

    // Create user message with the query and prior context
    let user_msg = Message::human(format!("{query}{prior_context}"));

    // Configure call options
    let options = CallOptions {
        max_tokens: Some(1000),
        ..Default::default()
    };

    // Invoke the model
    let response = model
        .invoke(&[system_prompt, user_msg], Some(&options))
        .await
        .map_err(|e| anyhow::anyhow!("LLM invocation failed: {e}"))?;

    // Extract and clean JSON response
    let raw_response = response.content_text();
    let json_str = clean_json_response(raw_response);

    // Parse plan response
    let plan_response: PlanResponse = serde_json::from_str(&json_str)
        .map_err(|e| anyhow::anyhow!("failed to parse plan as JSON: {e}\nResponse: {json_str}"))?;

    // Convert to SubTask structs with Pending status
    let sub_tasks: Vec<SubTask> = plan_response
        .sub_tasks
        .into_iter()
        .enumerate()
        .map(|(id, data)| SubTask {
            id: id + 1,
            description: data.description,
            status: TaskStatus::Pending,
        })
        .collect();

    // Validate we have at least 2 sub-tasks
    if sub_tasks.len() < 2 {
        return Err(anyhow::anyhow!(
            "Expected at least 2 sub-tasks, got {}",
            sub_tasks.len()
        ));
    }

    // Validate we have at most 6 sub-tasks
    if sub_tasks.len() > 6 {
        return Err(anyhow::anyhow!(
            "Expected at most 6 sub-tasks, got {}",
            sub_tasks.len()
        ));
    }

    Ok(ResearchStateUpdate {
        messages: None,
        query: None,
        plan: Some(sub_tasks),
        findings: None,
        report: None,
    })
}

/// Clean JSON response from LLM by removing markdown code blocks.
///
/// LLMs often wrap JSON responses in markdown code blocks like:
/// ```json
/// [...]
/// ```
/// This function strips those wrappers to return clean JSON.
fn clean_json_response(raw: &str) -> String {
    let mut s = raw.trim().to_string();

    // Remove ```json prefix
    if let Some(stripped) = s.strip_prefix("```json") {
        s = stripped.to_string();
    } else if let Some(stripped) = s.strip_prefix("```") {
        s = stripped.to_string();
    }

    // Remove ``` suffix
    if let Some(stripped) = s.strip_suffix("```") {
        s = stripped.to_string();
    }

    s.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clean_json_response_with_json_wrapper() {
        let raw = "```json\n{\"sub_tasks\": [{\"description\": \"test\"}]}\n```";
        let cleaned = clean_json_response(raw);
        assert_eq!(cleaned, "{\"sub_tasks\": [{\"description\": \"test\"}]}");
    }

    #[test]
    fn test_clean_json_response_with_plain_wrapper() {
        let raw = "```\n{\"sub_tasks\": [{\"description\": \"test\"}]}\n```";
        let cleaned = clean_json_response(raw);
        assert_eq!(cleaned, "{\"sub_tasks\": [{\"description\": \"test\"}]}");
    }

    #[test]
    fn test_clean_json_response_no_wrapper() {
        let raw = "{\"sub_tasks\": [{\"description\": \"test\"}]}";
        let cleaned = clean_json_response(raw);
        assert_eq!(cleaned, "{\"sub_tasks\": [{\"description\": \"test\"}]}");
    }
}

// Rust guideline compliant 2026-05-27
