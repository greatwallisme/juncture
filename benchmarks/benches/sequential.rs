//! Sequential benchmark: linear chain of N no-op nodes.
//!
//! Port of `LangGraph`'s `bench/sequential.py`. Measures pure framework overhead
//! per node by using no-op node functions that return empty state updates.

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use juncture_core::node::NodeFnUpdate;
use juncture_core::state::messages::{MessagesState, MessagesStateUpdate};
use juncture_core::{JunctureError, RunnableConfig, StateGraph};

/// No-op node matching `LangGraph`'s `def noop(state): pass`.
/// Returns an empty update (no field changes).
async fn noop_node(_state: MessagesState) -> Result<MessagesStateUpdate, JunctureError> {
    Ok(MessagesStateUpdate { messages: None })
}

/// Build a linear chain of `num_nodes` no-op nodes:
/// `START -> node_0 -> node_1 -> ... -> node_{N-1} -> END`
fn create_sequential_graph(num_nodes: usize) -> StateGraph<MessagesState> {
    let mut graph = StateGraph::new();

    let names: Vec<String> = (0..num_nodes).map(|i| format!("node_{i}")).collect();

    for name in &names {
        graph
            .add_node_simple(name.as_str(), NodeFnUpdate(noop_node))
            .expect("add_node_simple should succeed for unique names");
    }

    graph.set_entry_point(names[0].as_str());
    for i in 0..names.len() - 1 {
        graph.add_edge(names[i].as_str(), names[i + 1].as_str());
    }
    graph.set_finish_point(names[num_nodes - 1].as_str());

    graph
}

/// `RunnableConfig` with high recursion limit for deep graphs.
fn bench_config() -> RunnableConfig {
    RunnableConfig {
        recursion_limit: 20_000_000_000,
        ..RunnableConfig::new()
    }
}

fn benchmark_sequential(c: &mut Criterion) {
    let mut group = c.benchmark_group("sequential");

    let config = bench_config();

    for &num_nodes in &[10_usize, 100, 1000, 3000] {
        let graph = create_sequential_graph(num_nodes);
        let compiled = graph.compile().expect("compile should succeed");
        let input = MessagesState { messages: vec![] };

        group.bench_with_input(BenchmarkId::new("invoke", num_nodes), &num_nodes, |b, _| {
            b.iter(|| {
                let _ = compiled.invoke(input.clone(), &config);
            });
        });
    }

    group.finish();
}

criterion_group!(benches, benchmark_sequential);
criterion_main!(benches);

// Rust guideline compliant 2026-05-24
