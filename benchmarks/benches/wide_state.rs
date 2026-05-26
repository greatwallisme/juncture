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
#[derive(State, Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
struct WideState {
    #[reducer(append)]
    messages: Vec<serde_json::Value>,
    #[reducer(append)]
    trigger_events: Vec<serde_json::Value>,
    #[reducer(last_write_wins)]
    primary_issue_medium: Option<String>,
    autoresponse: Option<HashMap<String, serde_json::Value>>,
    issue: Option<HashMap<String, serde_json::Value>>,
    relevant_rules: Option<Vec<HashMap<String, serde_json::Value>>>,
    memory_docs: Option<Vec<HashMap<String, serde_json::Value>>>,
    #[reducer(append)]
    categorizations: Vec<HashMap<String, serde_json::Value>>,
    #[reducer(append)]
    responses: Vec<HashMap<String, serde_json::Value>>,
    user_info: Option<HashMap<String, serde_json::Value>>,
    crm_info: Option<HashMap<String, serde_json::Value>>,
    email_thread_id: Option<String>,
    slack_participants: Option<HashMap<String, serde_json::Value>>,
    bot_id: Option<String>,
    notified_assignees: Option<HashMap<String, serde_json::Value>>,
}

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

#[allow(
    clippy::unnecessary_wraps,
    reason = "node functions must return Result for IntoNode compatibility"
)]
fn node_one(state: &WideState) -> Result<WideStateUpdate, JunctureError> {
    let _ = state.messages.last();
    Ok(WideStateUpdate {
        trigger_events: Some(vec![serde_json::json!({
            "event": "triggered",
            "data": random_string(10)
        })]),
        primary_issue_medium: Some(Some("email".to_string())),
        ..Default::default()
    })
}

#[allow(
    clippy::unnecessary_wraps,
    reason = "node functions must return Result for IntoNode compatibility"
)]
fn node_two(state: &WideState) -> Result<WideStateUpdate, JunctureError> {
    let _ = state.trigger_events.last();
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

#[allow(
    clippy::unnecessary_wraps,
    reason = "node functions must return Result for IntoNode compatibility"
)]
fn node_three(state: &WideState) -> Result<WideStateUpdate, JunctureError> {
    let _ = &state.autoresponse;
    let mut rule = HashMap::new();
    rule.insert("id".to_string(), serde_json::json!("rule_1"));
    Ok(WideStateUpdate {
        relevant_rules: Some(Some(vec![rule])),
        ..Default::default()
    })
}

#[allow(
    clippy::unnecessary_wraps,
    reason = "node functions must return Result for IntoNode compatibility"
)]
fn node_four(state: &WideState) -> Result<WideStateUpdate, JunctureError> {
    let _ = state.trigger_events.last();
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

#[allow(
    clippy::unnecessary_wraps,
    reason = "node functions must return Result for IntoNode compatibility"
)]
fn node_five(state: &WideState) -> Result<WideStateUpdate, JunctureError> {
    let _ = state.categorizations.last();
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

#[allow(
    clippy::unnecessary_wraps,
    reason = "node functions must return Result for IntoNode compatibility"
)]
fn node_six(state: &WideState) -> Result<WideStateUpdate, JunctureError> {
    let _ = state.responses.last();
    Ok(WideStateUpdate {
        messages: Some(vec![serde_json::json!({"message": "completed"})]),
        ..Default::default()
    })
}

fn create_loop_router(n: usize) -> impl Fn(&WideState) -> &str + Send + Sync + 'static {
    move |state: &WideState| -> &str {
        if state.messages.len() <= n {
            "one"
        } else {
            "__end__"
        }
    }
}

fn create_wide_state_graph(n: usize) -> StateGraph<WideState> {
    let mut graph = StateGraph::new();

    graph
        .add_node_simple(
            "one",
            NodeFnUpdate(|state: &WideState| {
                let r = node_one(state);
                async move { r }
            }),
        )
        .expect("add_node_simple should succeed for unique names");
    graph
        .add_node_simple(
            "two",
            NodeFnUpdate(|state: &WideState| {
                let r = node_two(state);
                async move { r }
            }),
        )
        .expect("add_node_simple should succeed for unique names");
    graph
        .add_node_simple(
            "three",
            NodeFnUpdate(|state: &WideState| {
                let r = node_three(state);
                async move { r }
            }),
        )
        .expect("add_node_simple should succeed for unique names");
    graph
        .add_node_simple(
            "four",
            NodeFnUpdate(|state: &WideState| {
                let r = node_four(state);
                async move { r }
            }),
        )
        .expect("add_node_simple should succeed for unique names");
    graph
        .add_node_simple(
            "five",
            NodeFnUpdate(|state: &WideState| {
                let r = node_five(state);
                async move { r }
            }),
        )
        .expect("add_node_simple should succeed for unique names");
    graph
        .add_node_simple(
            "six",
            NodeFnUpdate(|state: &WideState| {
                let r = node_six(state);
                async move { r }
            }),
        )
        .expect("add_node_simple should succeed for unique names");

    graph.set_entry_point("one");
    graph.add_edge("one", "two");
    graph.add_edge("two", "three");
    graph.add_edge("two", "four");
    graph.add_edge("three", "five");
    graph.add_edge("four", "five");
    graph.add_edge("five", "six");

    let router = create_loop_router(n);
    graph.add_conditional_edges(
        "six",
        Arc::new(router) as Arc<dyn Router<WideState>>,
        PathMap::from(&[("one", "one"), ("__end__", "__end__")]),
    );

    graph
}

fn bench_config() -> RunnableConfig {
    RunnableConfig {
        recursion_limit: 20_000_000_000,
        ..RunnableConfig::new()
    }
}

fn create_wide_state_input() -> WideState {
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

    WideState {
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
    }
}

fn benchmark_wide_state(c: &mut Criterion) {
    let mut group = c.benchmark_group("wide_state");

    let config = bench_config();

    for &iterations in &[300_usize, 600, 1200] {
        let graph = create_wide_state_graph(iterations);
        let compiled = graph.compile().expect("compile should succeed");
        let input = create_wide_state_input();

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
