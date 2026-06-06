//! Example 04: Basic Chat with `MessagesState`
//!
//! Demonstrates a simple chatbot using `MessagesState` and `MockChatModel`:
//! - Using `MessagesState` for conversation history
//! - `MockChatModel` for simulating LLM responses
//! - Single-node graph that processes messages
//!
//! Key concepts:
//! - `MessagesState` and `Message` constructors
//! - Simple chatbot graph construction

use juncture_core::node::NodeFnUpdate;
use juncture_core::state::messages::{Message, MessagesState, MessagesStateUpdate};
use juncture_core::state::{Content, Role};
use juncture_core::{RunnableConfig, StateGraph};
use std::io::Write;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut graph = StateGraph::<MessagesState>::new();

    graph.add_node_simple(
        "chatbot",
        NodeFnUpdate(|state: &MessagesState| {
            let last_message = state.messages.last().cloned();
            async move {
                if let Some(msg) = last_message
                    && matches!(msg.role, Role::Human)
                {
                    let response = Message {
                        id: uuid::Uuid::new_v4().to_string(),
                        role: Role::Ai,
                        content: Content::Text("Hello! How can I help you today?".to_string()),
                        tool_calls: vec![],
                        tool_call_id: None,
                        name: None,
                        usage: None,
                    };

                    return Ok(MessagesStateUpdate {
                        messages: Some(vec![response]),
                    });
                }

                Ok(MessagesStateUpdate::default())
            }
        }),
    )?;

    graph.set_entry_point("chatbot");
    graph.set_finish_point("chatbot");

    let compiled = graph.compile()?;

    let human_message = Message {
        id: uuid::Uuid::new_v4().to_string(),
        role: Role::Human,
        content: Content::Text("Hi there!".to_string()),
        tool_calls: vec![],
        tool_call_id: None,
        name: None,
        usage: None,
    };

    let initial_state = MessagesState {
        messages: vec![human_message],
    };

    let output = compiled.invoke(initial_state, &RunnableConfig::new())?;

    let mut stdout = std::io::stdout();
    writeln!(stdout, "Conversation:")?;
    for msg in &output.value.messages {
        match msg.role {
            Role::Human => {
                if let Content::Text(text) = &msg.content {
                    writeln!(stdout, "  Human: {text}")?;
                }
            }
            Role::Ai => {
                if let Content::Text(text) = &msg.content {
                    writeln!(stdout, "  AI: {text}")?;
                }
            }
            Role::System => {
                if let Content::Text(text) = &msg.content {
                    writeln!(stdout, "  System: {text}")?;
                }
            }
            Role::Tool => {}
        }
    }

    Ok(())
}

// Rust guideline compliant 2026-05-24
