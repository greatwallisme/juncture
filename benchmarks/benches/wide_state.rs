//! Wide state benchmark: graph with many state fields.
//!
//! Port of `LangGraph`'s `bench/wide_state.py`. Measures framework overhead
//! when nodes read one field and write to multiple fields in a state with 15+
//! fields. Uses various reducer types (append, replace, `last_write_wins`).

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use juncture_core::edge::{PathMap, Router};
use juncture_core::node::NodeFnUpdate;
use juncture_core::{JunctureError, RunnableConfig, StateGraph};
use juncture_derive::State;
use rand::Rng;
use std::collections::HashMap;
use std::sync::Arc;

/// Wide state with 15+ fields matching the Python benchmark.
///
/// Field types:
/// - Vec fields with append reducer (`messages`, `trigger_events`, `categorizations`, `responses`)
/// - `Option<String>` fields with replace semantics (`primary_issue_medium`, `bot_id`)
/// - `Option<HashMap<String, serde_json::Value>>` for complex data (`user_info`, `crm_info`, etc.)
#[derive(State, Clone, Debug, serde::Serialize, serde::Deserialize)]
struct WideState {
    /// Messages exchanged during conversation
    #[reducer(append)]
    messages: Vec<serde_json::Value>,

    /// External events converted by the graph
    #[reducer(append)]
    trigger_events: Vec<serde_json::Value>,

    /// Primary medium for issue communication (email, slack, etc.)
    #[reducer(last_write_wins)]
    primary_issue_medium: Option<String>,

    /// Auto-response configuration
    autoresponse: Option<HashMap<String, serde_json::Value>>,

    /// Current issue details
    issue: Option<HashMap<String, serde_json::Value>>,

    /// SOPs from rulebook relevant to conversation
    relevant_rules: Option<Vec<HashMap<String, serde_json::Value>>>,

    /// Memory docs relevant to conversation
    memory_docs: Option<Vec<HashMap<String, serde_json::Value>>>,

    /// AI-generated issue categorizations
    #[reducer(append)]
    categorizations: Vec<HashMap<String, serde_json::Value>>,

    /// Draft responses recommended by AI
    #[reducer(append)]
    responses: Vec<HashMap<String, serde_json::Value>>,

    /// Current user state by email
    user_info: Option<HashMap<String, serde_json::Value>>,

    /// CRM info for user's organization
    crm_info: Option<HashMap<String, serde_json::Value>>,

    /// Current email thread ID
    email_thread_id: Option<String>,

    /// Growing list of Slack participants
    slack_participants: Option<HashMap<String, serde_json::Value>>,

    /// Bot user ID in Slack channel
    bot_id: Option<String>,

    /// Assignees that have been notified
    notified_assignees: Option<HashMap<String, serde_json::Value>>,
}

/// Helper to generate random string data matching Python's random string generation.
fn random_string(length: usize) -> String {
    const CHARS: &[u8] = b"abcdefghijklmnopqrstuvwxyz";
    let mut rng = rand::thread_rng();
    (0..length)
        .map(|_| {
            let idx = rng.gen_range(0..CHARS.len());
            CHARS[idx] as char
        })
        .collect()
}

/// Node one: reads `messages`, writes to `trigger_events` and `primary_issue_medium`.
async fn node_one(state: WideState) -> Result<WideStateUpdate, JunctureError> {
    // Read from messages (last element if exists)
    let _ = state.messages.last();

    // Write to trigger_events and primary_issue_medium
    Ok(WideStateUpdate {
        trigger_events: Some(vec![serde_json::json!({
            "event": "triggered",
            "data": random_string(10)
        })]),
        primary_issue_medium: Some(Some("email".to_string())),
        ..Default::default()
    })
}

/// Node two: reads `trigger_events`, writes to `autoresponse` and `issue`.
async fn node_two(state: WideState) -> Result<WideStateUpdate, JunctureError> {
    // Read from trigger_events (last element if exists)
    let _ = state.trigger_events.last();

    // Write to autoresponse and issue
    let mut autoresponse_map = HashMap::new();
    autoresponse_map.insert("enabled".to_string(), serde_json::json!(true));

    let mut issue_map = HashMap::new();
    issue_map.insert("type".to_string(), serde_json::json!("support"));

    Ok(WideStateUpdate {
        autoresponse: Some(Some(autoresponse_map)),
        issue: Some(Some(issue_map)),
        ..Default::default()
    })
}

/// Node three: reads `autoresponse`, writes to `relevant_rules`.
async fn node_three(state: WideState) -> Result<WideStateUpdate, JunctureError> {
    // Read from autoresponse
    let _ = &state.autoresponse;

    // Write to relevant_rules
    let mut rule = HashMap::new();
    rule.insert("id".to_string(), serde_json::json!("rule_1"));

    Ok(WideStateUpdate {
        relevant_rules: Some(Some(vec![rule])),
        ..Default::default()
    })
}

/// Node four: reads `trigger_events`, writes to `categorizations`, `responses`, `memory_docs`.
async fn node_four(state: WideState) -> Result<WideStateUpdate, JunctureError> {
    // Read from trigger_events (last element if exists)
    let _ = state.trigger_events.last();

    // Write to categorizations, responses, memory_docs
    let mut categorization = HashMap::new();
    categorization.insert("category".to_string(), serde_json::json!("billing"));

    let mut response = HashMap::new();
    response.insert("text".to_string(), serde_json::json!("Hello"));

    let mut memory_doc = HashMap::new();
    memory_doc.insert("doc_id".to_string(), serde_json::json!("doc_1"));

    Ok(WideStateUpdate {
        categorizations: Some(vec![categorization]),
        responses: Some(vec![response]),
        memory_docs: Some(Some(vec![memory_doc])),
        ..Default::default()
    })
}

/// Node five: reads `categorizations`, writes to `user_info`, `crm_info`, `email_thread_id`,
/// `slack_participants`, `bot_id`, `notified_assignees`.
async fn node_five(state: WideState) -> Result<WideStateUpdate, JunctureError> {
    // Read from categorizations (last element if exists)
    let _ = state.categorizations.last();

    // Write to multiple fields
    let mut user_info = HashMap::new();
    user_info.insert("email".to_string(), serde_json::json!("user@example.com"));

    let mut crm_info = HashMap::new();
    crm_info.insert("org".to_string(), serde_json::json!("acme"));

    let mut slack_participants = HashMap::new();
    slack_participants.insert("count".to_string(), serde_json::json!(5));

    let mut notified = HashMap::new();
    notified.insert("agent".to_string(), serde_json::json!("assigned"));

    Ok(WideStateUpdate {
        user_info: Some(Some(user_info)),
        crm_info: Some(Some(crm_info)),
        email_thread_id: Some(Some("thread_123".to_string())),
        slack_participants: Some(Some(slack_participants)),
        bot_id: Some(Some("bot_abc".to_string())),
        notified_assignees: Some(Some(notified)),
        ..Default::default()
    })
}

/// Node six: reads `responses`, writes to `messages`.
async fn node_six(state: WideState) -> Result<WideStateUpdate, JunctureError> {
    // Read from responses (last element if exists)
    let _ = state.responses.last();

    // Write to messages
    Ok(WideStateUpdate {
        messages: Some(vec![serde_json::json!({"message": "completed"})]),
        ..Default::default()
    })
}
/// Create a loop router function for the given iteration count.
fn create_loop_router(n: usize) -> impl Fn(&WideState) -> &str + Send + Sync + 'static {
    move |state: &WideState| -> &str {
        if state.messages.len() <= n {
            "one"
        } else {
            "__end__"
        }
    }
}

/// Build the wide state graph matching Python topology.
fn create_wide_state_graph(n: usize) -> StateGraph<WideState> {
    let mut graph = StateGraph::new();

    // Add all nodes
    graph
        .add_node_simple("one", NodeFnUpdate(node_one))
        .expect("add_node_simple should succeed for unique names");
    graph
        .add_node_simple("two", NodeFnUpdate(node_two))
        .expect("add_node_simple should succeed for unique names");
    graph
        .add_node_simple("three", NodeFnUpdate(node_three))
        .expect("add_node_simple should succeed for unique names");
    graph
        .add_node_simple("four", NodeFnUpdate(node_four))
        .expect("add_node_simple should succeed for unique names");
    graph
        .add_node_simple("five", NodeFnUpdate(node_five))
        .expect("add_node_simple should succeed for unique names");
    graph
        .add_node_simple("six", NodeFnUpdate(node_six))
        .expect("add_node_simple should succeed for unique names");

    // Add edges matching Python topology
    graph.set_entry_point("one");
    graph.add_edge("one", "two");
    graph.add_edge("two", "three");
    graph.add_edge("two", "four");
    graph.add_edge("three", "five");
    graph.add_edge("four", "five");
    graph.add_edge("five", "six");

    // Add conditional edge from six: loop back to "one" or go to END
    let router = create_loop_router(n);
    graph.add_conditional_edges(
        "six",
        Arc::new(router) as Arc<dyn Router<WideState>>,
        PathMap::from(&[("one", "one"), ("__end__", "__end__")]),
    );

    graph
}

/// `RunnableConfig` with high recursion limit for looping graphs.
fn bench_config() -> RunnableConfig {
    RunnableConfig {
        recursion_limit: 20_000_000_000,
        ..RunnableConfig::new()
    }
}

fn benchmark_wide_state(c: &mut Criterion) {
    let mut group = c.benchmark_group("wide_state");

    let config = bench_config();

    for &iterations in &[300_usize, 600, 1200] {
        let graph = create_wide_state_graph(iterations);
        let compiled = graph.compile().expect("compile should succeed");

        // Pre-generate input data matching Python structure
        let mut input_messages = Vec::new();
        for _i in 0..50 {
            let mut inner_map = HashMap::new();
            for j in 0..50 {
                let key = (j.to_string()).repeat(10);
                let value = serde_json::json!([
                    "hi?".repeat(10),
                    true,
                    1,
                    6_327_816_386_138_i64,
                    serde_json::Value::Null
                ]);
                inner_map.insert(key, value);
            }
            input_messages.push(serde_json::json!(inner_map));
        }

        let input = WideState {
            messages: input_messages,
            trigger_events: vec![],
            primary_issue_medium: Some("email".to_string()),
            autoresponse: None,
            issue: None,
            relevant_rules: None,
            memory_docs: None,
            categorizations: vec![],
            responses: vec![],
            user_info: None,
            crm_info: None,
            email_thread_id: None,
            slack_participants: None,
            bot_id: None,
            notified_assignees: None,
        };

        group.bench_with_input(
            BenchmarkId::new("invoke", iterations),
            &iterations,
            |b, _| {
                b.iter(|| {
                    let _ = compiled.invoke(input.clone(), &config);
                });
            },
        );
    }

    group.finish();
}

criterion_group!(benches, benchmark_wide_state);
criterion_main!(benches);

// Rust guideline compliant 2026-05-24
