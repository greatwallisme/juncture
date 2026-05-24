//! Checkpoint overhead benchmark: measure persistence cost of `MemorySaver`.
//!
//! Compares graph execution with and without checkpointing to isolate
//! the performance overhead of state persistence. Uses sequential graph
//! pattern (100-node linear chain) with no-op nodes.

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use juncture_checkpoint::MemorySaver;
use juncture_core::checkpoint::CheckpointSaver;
use juncture_core::node::NodeFnUpdate;
use juncture_core::state::messages::{MessagesState, MessagesStateUpdate};
use juncture_core::{JunctureError, RunnableConfig, StateGraph};
use std::hint::black_box;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

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

/// `RunnableConfig` with high recursion limit and unique `thread_id` for checkpoints.
fn bench_config(thread_id: &str) -> RunnableConfig {
    RunnableConfig {
        recursion_limit: 20_000_000_000,
        thread_id: Some(thread_id.to_string()),
        ..RunnableConfig::new()
    }
}

fn benchmark_checkpoint(c: &mut Criterion) {
    let mut group = c.benchmark_group("checkpoint");

    // Test with 100-node sequential graph (same as Python checkpoint.py)
    let num_nodes = 100_usize;
    let graph = create_sequential_graph(num_nodes);
    let input = MessagesState { messages: vec![] };

    // Benchmark WITHOUT checkpointing
    let compiled_no_checkpoint = graph
        .compile()
        .expect("compile should succeed without checkpointer");

    group.bench_with_input(
        BenchmarkId::new("checkpoint_off", num_nodes),
        &num_nodes,
        |b, _| {
            b.iter(|| {
                let config = bench_config("bench_no_checkpoint");
                let _ = compiled_no_checkpoint.invoke(black_box(input.clone()), &config);
            });
        },
    );

    // Benchmark WITH checkpointing
    // Create MemorySaver once outside the timed loop for fair comparison
    let checkpointer: Arc<dyn CheckpointSaver> = Arc::new(MemorySaver::new());
    let compiled_with_checkpoint = graph
        .compile_with_checkpointer(Some(checkpointer))
        .expect("compile should succeed with checkpointer");

    // Use atomic counter for unique thread_ids per iteration
    let counter = Arc::new(AtomicUsize::new(0));

    group.bench_with_input(
        BenchmarkId::new("checkpoint_on", num_nodes),
        &num_nodes,
        |b, _| {
            b.iter(|| {
                // Use unique thread_id per iteration to avoid checkpoint conflicts
                let iter_id = counter.fetch_add(1, Ordering::Relaxed);
                let thread_id = format!("bench_checkpoint_{iter_id}");
                let config = bench_config(&thread_id);
                let _ = compiled_with_checkpoint.invoke(black_box(input.clone()), &config);
            });
        },
    );

    group.finish();
}

criterion_group!(benches, benchmark_checkpoint);
criterion_main!(benches);

// Rust guideline compliant 2026-05-24
