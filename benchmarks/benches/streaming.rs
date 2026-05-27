//! Streaming benchmark: stream events from a chain of N nodes.
//!
//! Measures streaming overhead by creating chains with different numbers
//! of nodes (100, 1000, 10000) and measuring the time to collect all events.

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use futures::StreamExt;
use juncture_core::node::NodeFnUpdate;
use juncture_core::state::messages::{MessagesState, MessagesStateUpdate};
use juncture_core::stream::{StreamEvent, StreamMode};
use juncture_core::{RunnableConfig, StateGraph};
use std::pin::Pin;

fn create_streaming_graph(num_nodes: usize) -> StateGraph<MessagesState> {
    let mut graph = StateGraph::new();

    let names: Vec<String> = (0..num_nodes).map(|i| format!("node_{i}")).collect();

    for name in &names {
        graph
            .add_node_simple(
                name.as_str(),
                NodeFnUpdate(|_state: &MessagesState| async move {
                    Ok(MessagesStateUpdate { messages: None })
                }),
            )
            .expect("add_node_simple should succeed for unique names");
    }

    graph.set_entry_point(names[0].as_str());
    for i in 0..names.len() - 1 {
        graph.add_edge(names[i].as_str(), names[i + 1].as_str());
    }
    graph.set_finish_point(names[num_nodes - 1].as_str());

    graph
}

fn bench_config() -> RunnableConfig {
    RunnableConfig {
        recursion_limit: 20_000_000_000,
        ..RunnableConfig::new()
    }
}

async fn count_events<S>(
    mut stream: Pin<
        Box<
            dyn futures::Stream<Item = Result<StreamEvent<S>, juncture_core::error::JunctureError>>
                + Send,
        >,
    >,
) -> usize
where
    S: juncture_core::State,
{
    let mut count = 0;
    while let Some(result) = stream.next().await {
        result.expect("stream event should not error");
        count += 1;
    }
    count
}

fn benchmark_streaming(c: &mut Criterion) {
    let mut group = c.benchmark_group("streaming");

    let config = bench_config();
    let runtime = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");

    for &num_nodes in &[100_usize, 1000, 10000] {
        let graph = create_streaming_graph(num_nodes);
        let compiled = graph.compile().expect("compile should succeed");
        let input = MessagesState { messages: vec![] };

        group.bench_with_input(BenchmarkId::new("stream", num_nodes), &num_nodes, |b, _| {
            b.to_async(&runtime).iter(|| async {
                let handle = compiled
                    .stream(input.clone(), &config, StreamMode::Values)
                    .await
                    .expect("stream should succeed");
                let _count = count_events(handle.stream).await;
            });
        });
    }

    group.finish();
}

criterion_group!(benches, benchmark_streaming);
criterion_main!(benches);

// Rust guideline compliant 2026-05-24
