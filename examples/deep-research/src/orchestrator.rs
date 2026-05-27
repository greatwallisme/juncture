//! Multi-agent research orchestrator using `StateGraph`.

use anyhow::Result;
use futures::future::{FutureExt, join_all};
use juncture::RunnableConfig;
use juncture_checkpoint::MemorySaver;
use juncture_core::error::JunctureError;
use juncture_core::graph::StateGraph;
use juncture_core::node::NodeFnUpdate;
use juncture_core::{END, START};
use std::sync::Arc;

use crate::agents::plan_research_node;
use crate::agents::research_sub_task;
use crate::agents::write_report;
use crate::config::ResearchConfig;
use crate::state::{ResearchState, ResearchStateUpdate, SubTask, TaskStatus};

/// Run the multi-agent research orchestrator.
///
/// # Arguments
///
/// * `config` - Research configuration
/// * `query` - Research query
/// * `thread_id` - Optional thread ID for checkpointing (enables session persistence)
///
/// # Errors
///
/// Returns error if:
/// - Graph execution fails
/// - Node execution fails
/// - Research agent fails
pub fn run_research(
    config: &ResearchConfig,
    query: &str,
    thread_id: Option<&str>,
) -> Result<String> {
    // Build the multi-agent graph
    let mut graph = StateGraph::<ResearchState>::new();

    // Add planner node
    let planner_node = plan_research_node(config.clone());
    graph.add_node_simple("planner", planner_node)?;

    // Add research coordinator node (uses parallel execution)
    let coordinator_config = config.clone();
    let coordinator_node = NodeFnUpdate(move |state: &ResearchState| {
        let config = coordinator_config.clone();
        let plan = state.plan.clone();
        async move {
            research_coordinator(&config, &plan)
                .await
                .map_err(|e| JunctureError::execution(e.to_string()))
        }
        .boxed()
    });
    graph.add_node_simple("research_coordinator", coordinator_node)?;

    // Add writer node
    let writer_config = config.clone();
    let writer_node_fn = NodeFnUpdate(move |state: &ResearchState| {
        let config = writer_config.clone();
        let findings = state.findings.clone();
        let query = state.query.clone();
        async move {
            writer_node_impl(&config, &query, &findings)
                .await
                .map_err(|e| JunctureError::execution(e.to_string()))
        }
        .boxed()
    });
    graph.add_node_simple("writer", writer_node_fn)?;

    // Add edges: START -> planner -> research_coordinator -> writer -> END
    graph.add_edge(START, "planner");
    graph.add_edge("planner", "research_coordinator");
    graph.add_edge("research_coordinator", "writer");
    graph.add_edge("writer", END);

    // Compile the graph with checkpointing for session persistence
    let checkpointer = MemorySaver::new();
    let compiled = graph.compile_with_checkpointer(Some(Arc::new(checkpointer)))?;

    // Build initial state
    let initial_state = ResearchState {
        messages: Vec::new(),
        query: query.to_string(),
        plan: Vec::new(),
        findings: Vec::new(),
        report: None,
    };

    // Build runnable config with optional thread_id for checkpointing
    let mut runnable_config = RunnableConfig::default();
    if let Some(tid) = thread_id {
        runnable_config = runnable_config.with_thread_id(tid.to_string());
    }

    // Execute the graph
    let output = compiled
        .invoke(initial_state, &runnable_config)
        .map_err(|e| anyhow::anyhow!("Graph execution failed: {e}"))?;

    // Extract the final report
    output
        .value
        .report
        .ok_or_else(|| anyhow::anyhow!("No report generated"))
}

/// Research coordinator node that executes sub-tasks in parallel.
///
/// # Arguments
///
/// * `config` - Research configuration
/// * `plan` - Current research plan
///
/// # Errors
///
/// Returns error if research execution fails.
async fn research_coordinator(
    config: &ResearchConfig,
    plan: &[SubTask],
) -> Result<ResearchStateUpdate> {
    // Filter for pending sub-tasks
    let pending_tasks: Vec<&SubTask> = plan
        .iter()
        .filter(|t| t.status == TaskStatus::Pending)
        .collect();

    if pending_tasks.is_empty() {
        // All tasks completed, return empty update
        return Ok(ResearchStateUpdate {
            messages: None,
            query: None,
            plan: None,
            findings: None,
            report: None,
        });
    }

    // Execute researchers in parallel using join_all
    let tasks: Vec<_> = pending_tasks
        .iter()
        .map(|task| research_sub_task(config, task))
        .collect();

    let findings = join_all(tasks)
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| anyhow::anyhow!("Research execution failed: {e}"))?;

    // Update plan with completed status
    let updated_plan: Vec<SubTask> = plan
        .iter()
        .map(|task| {
            if pending_tasks.iter().any(|pt| pt.id == task.id) {
                SubTask {
                    status: TaskStatus::Completed,
                    ..task.clone()
                }
            } else {
                task.clone()
            }
        })
        .collect();

    Ok(ResearchStateUpdate {
        messages: None,
        query: None,
        plan: Some(updated_plan),
        findings: Some(findings),
        report: None,
    })
}

/// Writer node that synthesizes findings into a final report.
///
/// # Arguments
///
/// * `config` - Research configuration
/// * `query` - Original research query
/// * `findings` - Research findings
///
/// # Errors
///
/// Returns error if report generation fails.
async fn writer_node_impl(
    config: &ResearchConfig,
    query: &str,
    findings: &[crate::state::Finding],
) -> Result<ResearchStateUpdate> {
    if findings.is_empty() {
        return Err(anyhow::anyhow!("No findings to synthesize"));
    }

    // Generate the report
    let generated_report: String = write_report(config, query, findings).await?;

    Ok(ResearchStateUpdate {
        messages: None,
        query: None,
        plan: None,
        findings: None,
        report: Some(Some(generated_report)),
    })
}

// Rust guideline compliant 2026-05-27
