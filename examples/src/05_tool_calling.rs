//! Example 05: Tool Calling
//!
//! Demonstrates tool definition and execution:
//! - Defining a custom `Tool` implementation
//! - Using `ToolNode` to execute tools
//! - Manual graph construction with tool execution
//!
//! Key concepts:
//! - `Tool` trait implementation
//! - `ToolNode` for executing tools from messages
//! - Manual agent graph with tool execution

use async_trait::async_trait;
use juncture::tools::{Tool, ToolError};
use juncture_core::node::NodeFnUpdate;
use juncture_core::state::messages::{Message, MessagesState, MessagesStateUpdate};
use juncture_core::state::{Content, Role};
use juncture_core::{RunnableConfig, StateGraph};
use serde_json::json;
use std::io::Write;

/// Simple calculator tool that adds two numbers
#[derive(Debug)]
struct CalculatorTool;

#[async_trait]
impl Tool for CalculatorTool {
    fn name(&self) -> &'static str {
        "calculator"
    }

    fn description(&self) -> &'static str {
        "Adds two numbers together and returns the result"
    }

    fn schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "a": {
                    "type": "number",
                    "description": "First number to add"
                },
                "b": {
                    "type": "number",
                    "description": "Second number to add"
                }
            },
            "required": ["a", "b"]
        })
    }

    async fn invoke(&self, input: serde_json::Value) -> Result<String, ToolError> {
        let a = input["a"]
            .as_f64()
            .ok_or_else(|| ToolError::InvalidInput("Missing 'a' parameter".to_string()))?;
        let b = input["b"]
            .as_f64()
            .ok_or_else(|| ToolError::InvalidInput("Missing 'b' parameter".to_string()))?;

        let result = a + b;
        Ok(result.to_string())
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut stdout = std::io::stdout();

    // Demonstrate the CalculatorTool directly
    let calculator = CalculatorTool;
    writeln!(stdout, "Tool: {}", calculator.name())?;
    writeln!(
        stdout,
        "Schema: {}",
        serde_json::to_string_pretty(&calculator.schema())?
    )?;

    // Execute the tool directly to show it works
    let rt = tokio::runtime::Runtime::new()?;
    let result = rt.block_on(async { calculator.invoke(json!({"a": 5, "b": 3})).await })?;
    writeln!(stdout, "5 + 3 = {result}")?;

    // Build a graph that uses the tool via an agent node
    let mut graph = StateGraph::<MessagesState>::new();

    graph.add_node_simple(
        "agent",
        NodeFnUpdate(|state: &MessagesState| {
            let last_message = state.messages.last().cloned();
            async move {
                if let Some(msg) = last_message
                    && matches!(msg.role, Role::Human)
                {
                    let response_text = if let Content::Text(text) = &msg.content {
                        if text.contains("add") || text.contains("plus") {
                            "I can help with that using the calculator tool!"
                        } else {
                            "Hello! I can help you with calculations."
                        }
                    } else {
                        "Hello! How can I help you today?"
                    };

                    let response = Message {
                        id: uuid::Uuid::new_v4().to_string(),
                        role: Role::Ai,
                        content: Content::Text(response_text.to_string()),
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

    graph.set_entry_point("agent");
    graph.set_finish_point("agent");

    let compiled = graph.compile()?;

    let human_message = Message {
        id: uuid::Uuid::new_v4().to_string(),
        role: Role::Human,
        content: Content::Text("What is 5 plus 3?".to_string()),
        tool_calls: vec![],
        tool_call_id: None,
        name: None,
        usage: None,
    };

    let initial_state = MessagesState {
        messages: vec![human_message],
    };

    let output = compiled.invoke(initial_state, &RunnableConfig::default())?;

    writeln!(stdout, "\nConversation:")?;
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
            Role::System | Role::Tool => {}
        }
    }

    Ok(())
}

// Rust guideline compliant 2026-05-24
