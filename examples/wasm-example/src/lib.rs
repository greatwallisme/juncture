//! WASM Example: Juncture Graph Engine + LLM Chat
//!
//! Demonstrates running a Juncture state machine graph in the browser via WASM,
//! including real LLM API calls through the chat module.
//!
//! Build: wasm-pack build --target web
//! Open:  index.html in a browser (serve via HTTP, not file://)

use juncture::llm::{CallOptions, ChatModel};
use juncture::state::{Content, Message, Role};
use juncture_core::node::NodeFnUpdate;
use juncture_core::{RunnableConfig, StateGraph};
use juncture_derive::State;
use wasm_bindgen::prelude::*;

/// State for the text processing pipeline.
#[derive(State, Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
struct TextState {
    /// Original input text.
    input: String,
    /// Word count result.
    word_count: usize,
    /// Character count result.
    char_count: usize,
    /// Processing summary.
    summary: String,
}

/// Build and compile the text processing graph.
fn build_graph() -> StateGraph<TextState> {
    let mut graph = StateGraph::<TextState>::new();

    graph
        .add_node_simple(
            "count",
            NodeFnUpdate(|state: &TextState| {
                let input = state.input.clone();
                let word_count = input.split_whitespace().count();
                let char_count = input.chars().count();
                async move {
                    Ok(TextStateUpdate {
                        input: None,
                        word_count: Some(word_count),
                        char_count: Some(char_count),
                        summary: None,
                    })
                }
            }),
        )
        .expect("failed to add count node");

    graph
        .add_node_simple(
            "summary",
            NodeFnUpdate(|state: &TextState| {
                let input = state.input.clone();
                let word_count = state.word_count;
                let char_count = state.char_count;
                async move {
                    let summary = format!(
                        "Analyzed {} chars and {} words in: \"{}\"",
                        char_count,
                        word_count,
                        if input.len() > 50 {
                            format!("{}...", &input[..50])
                        } else {
                            input
                        }
                    );
                    Ok(TextStateUpdate {
                        input: None,
                        word_count: None,
                        char_count: None,
                        summary: Some(summary),
                    })
                }
            }),
        )
        .expect("failed to add summary node");

    graph.add_edge("count", "summary");
    graph.set_entry_point("count");
    graph.set_finish_point("summary");

    graph
}

/// Analyze text through the Juncture graph.
///
/// Returns a JS object with: `{ input, word_count, char_count, summary }`.
#[wasm_bindgen]
pub async fn analyze_text(input: &str) -> Result<JsValue, JsValue> {
    let graph = build_graph()
        .compile()
        .map_err(|e| JsValue::from_str(&e.to_string()))?;

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
        .map_err(|e| JsValue::from_str(&e.to_string()))?;

    serde_wasm_bindgen::to_value(&output.value).map_err(|e| JsValue::from_str(&e.to_string()))
}

/// Chat with an LLM through the Juncture chat module.
///
/// This demonstrates that the full ChatModel trait works on WASM.
///
/// # Arguments
///
/// * `api_key` - OpenAI API key
/// * `base_url` - API base URL (e.g., "https://api.openai.com/v1")
/// * `model` - Model name (e.g., "gpt-4o-mini")
/// * `message` - User message
#[wasm_bindgen]
pub async fn chat(
    api_key: &str,
    base_url: &str,
    model: &str,
    message: &str,
) -> Result<JsValue, JsValue> {
    let chat_model = juncture::llm::ChatOpenAI::new(api_key)
        .with_model(model)
        .with_base_url(base_url);

    let messages = vec![Message {
        id: "user-1".to_string(),
        role: Role::Human,
        content: Content::Text(message.to_string()),
        tool_calls: vec![],
        tool_call_id: None,
        name: None,
        usage: None,
    }];

    let options = CallOptions {
        max_tokens: Some(1024),
        ..Default::default()
    };

    let response = juncture_core::wasm_send::force_send(
        chat_model.invoke(&messages, Some(&options))
    )
        .await
        .map_err(|e| JsValue::from_str(&e.to_string()))?;

    // Extract content text from response
    let content_text = match &response.content {
        Content::Text(t) => t.clone(),
        Content::MultiPart(parts) => {
            parts.iter().filter_map(|p| {
                if let juncture::state::ContentPart::Text { text } = p {
                    Some(text.as_str())
                } else {
                    None
                }
            }).collect::<Vec<_>>().join("")
        }
    };

    #[derive(serde::Serialize)]
    struct ChatResult {
        role: String,
        content: String,
    }

    let result = ChatResult {
        role: format!("{:?}", response.role),
        content: content_text,
    };

    serde_wasm_bindgen::to_value(&result).map_err(|e| JsValue::from_str(&e.to_string()))
}

/// Get the Juncture version and WASM status.
#[wasm_bindgen]
pub fn info() -> JsValue {
    let info = serde_json::json!({
        "framework": "Juncture",
        "version": env!("CARGO_PKG_VERSION"),
        "target": "wasm32-unknown-unknown",
        "features": ["wasm", "openai"],
    });
    serde_wasm_bindgen::to_value(&info).unwrap_or(JsValue::NULL)
}
