//! WASI CLI Example: Juncture Graph Engine on WASI
//!
//! Demonstrates running a Juncture StateGraph on WASI (WebAssembly System Interface).
//! No browser required -- runs as a standalone CLI via wasmtime/wasmer.
//!
//! For real LLM interaction, use `wasm-edge-server` (Spin HTTP) which has
//! full HTTP support via spin-sdk.
//!
//! # Build
//!
//!   cargo build --target wasm32-wasip1 --release
//!
//! # Run
//!
//!   wasmtime run target/wasm32-wasip1/release/wasm_edge_cli.wasm -- "your text here"

use std::io::Write;

use juncture_core::node::NodeFnUpdate;
use juncture_core::{RunnableConfig, StateGraph};
use juncture_derive::State;

/// State for the text processing pipeline.
#[derive(State, Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
struct TextState {
    /// Original input text.
    input: String,
    /// Word count result.
    word_count: usize,
    /// Character count result.
    char_count: usize,
    /// Sentence count result.
    sentence_count: usize,
    /// Processing summary.
    summary: String,
}

/// Build the Juncture text analysis graph.
///
/// Demonstrates a multi-node pipeline:
///   input -> count -> analyze -> summary -> output
fn build_graph() -> StateGraph<TextState> {
    let mut graph = StateGraph::<TextState>::new();

    // Node 1: Count words and characters
    graph
        .add_node_simple(
            "count",
            NodeFnUpdate(|state: &TextState| {
                let input = state.input.clone();
                async move {
                    let word_count = input.split_whitespace().count();
                    let char_count = input.chars().count();
                    Ok(TextStateUpdate {
                        input: None,
                        word_count: Some(word_count),
                        char_count: Some(char_count),
                        sentence_count: None,
                        summary: None,
                    })
                }
            }),
        )
        .expect("failed to add count node");

    // Node 2: Analyze text structure
    graph
        .add_node_simple(
            "analyze",
            NodeFnUpdate(|state: &TextState| {
                let input = state.input.clone();
                async move {
                    let sentence_count = input
                        .split(|c: char| c == '.' || c == '!' || c == '?')
                        .filter(|s| !s.trim().is_empty())
                        .count();
                    Ok(TextStateUpdate {
                        input: None,
                        word_count: None,
                        char_count: None,
                        sentence_count: Some(sentence_count),
                        summary: None,
                    })
                }
            }),
        )
        .expect("failed to add analyze node");

    // Node 3: Generate summary
    graph
        .add_node_simple(
            "summary",
            NodeFnUpdate(|state: &TextState| {
                let input = state.input.clone();
                let word_count = state.word_count;
                let char_count = state.char_count;
                let sentence_count = state.sentence_count;
                async move {
                    let truncated = if input.len() > 80 {
                        format!("{}...", &input[..80])
                    } else {
                        input
                    };
                    let summary = format!(
                        "Text analysis: {char_count} chars, {word_count} words, \
                         {sentence_count} sentences. Preview: \"{truncated}\""
                    );
                    Ok(TextStateUpdate {
                        input: None,
                        word_count: None,
                        char_count: None,
                        sentence_count: None,
                        summary: Some(summary),
                    })
                }
            }),
        )
        .expect("failed to add summary node");

    graph.add_edge("count", "analyze");
    graph.add_edge("analyze", "summary");
    graph.set_entry_point("count");
    graph.set_finish_point("summary");

    graph
}

fn print_usage() {
    let mut stderr = std::io::stderr();
    let _ = writeln!(stderr, "Usage: wasm-edge-cli <text>");
    let _ = writeln!(stderr);
    let _ = writeln!(stderr, "Analyzes text using a Juncture StateGraph pipeline.");
    let _ = writeln!(stderr);
    let _ = writeln!(stderr, "Example:");
    let _ = writeln!(
        stderr,
        "  wasmtime run app.wasm -- \"Hello world. This is a test!\""
    );
}

fn main() {
    let args: Vec<String> = std::env::args().collect();

    // Skip program name and "--" separator (wasmtime passes "--" as an arg)
    let query_args: Vec<&String> = args
        .iter()
        .skip(1)
        .filter(|a| *a != "--")
        .collect();

    if query_args.is_empty() {
        print_usage();
        std::process::exit(1);
    }

    let input = query_args.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(" ");

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap_or_else(|e| {
            let mut stderr = std::io::stderr();
            let _ = writeln!(stderr, "Failed to create tokio runtime: {e}");
            std::process::exit(1);
        });

    match rt.block_on(run_graph(&input)) {
        Ok(result) => {
            let mut stdout = std::io::stdout();
            let _ = writeln!(stdout, "{result}");
        }
        Err(e) => {
            let mut stderr = std::io::stderr();
            let _ = writeln!(stderr, "Error: {e}");
            std::process::exit(1);
        }
    }
}

async fn run_graph(input: &str) -> Result<String, String> {
    let graph = build_graph()
        .compile()
        .map_err(|e| format!("Graph compile error: {e}"))?;

    let initial_state = TextState {
        input: input.to_string(),
        ..Default::default()
    };

    let config = RunnableConfig {
        recursion_limit: 25,
        ..Default::default()
    };

    let output = graph
        .invoke_async(initial_state, &config)
        .await
        .map_err(|e| format!("Graph execution error: {e}"))?;

    let result = &output.value;
    let json = serde_json::json!({
        "input": result.input,
        "word_count": result.word_count,
        "char_count": result.char_count,
        "sentence_count": result.sentence_count,
        "summary": result.summary,
        "runtime": "wasi",
        "target": "wasm32-wasip1",
    });

    serde_json::to_string_pretty(&json).map_err(|e| format!("JSON error: {e}"))
}

// Rust guideline compliant 2026-05-28
