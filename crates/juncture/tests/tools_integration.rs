//! Integration tests for the tools module

use async_trait::async_trait;
use juncture::tools::{
    Tool, ToolError, ToolNode, ValidationNode, tools_condition, tools_condition_from_messages,
};
use juncture_core::state::messages::{Message, MessagesState, ToolCall};
use serde_json::json;

// Type alias for tests
type TestToolNode = ToolNode<juncture_core::state::messages::MessagesState>;

/// Simple test tool
struct TestTool;

#[async_trait]
impl Tool for TestTool {
    fn name(&self) -> &'static str {
        "test_tool"
    }

    fn description(&self) -> &'static str {
        "A test tool"
    }

    fn schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "input": {"type": "string"}
            },
            "required": ["input"]
        })
    }

    async fn invoke(&self, input: serde_json::Value) -> Result<String, ToolError> {
        let input_str = input["input"]
            .as_str()
            .ok_or_else(|| ToolError::invalid_input("Missing 'input'".to_string()))?;
        Ok(format!("Processed: {input_str}"))
    }
}

#[tokio::test]
async fn test_tool_node_integration() {
    let tools = vec![Box::new(TestTool) as Box<dyn Tool>];
    let node = TestToolNode::new(tools);

    let messages = vec![Message::ai_with_tool_calls(
        "Execute test",
        vec![ToolCall {
            id: "call_1".to_string(),
            name: "test_tool".to_string(),
            arguments: json!({"input": "hello"}),
        }],
    )];

    let results = node.execute(&messages).await.unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].tool_call_id, Some("call_1".to_string()));
}

#[tokio::test]
async fn test_tools_condition_from_messages_integration() {
    // Test with human message (no tools)
    let messages = vec![Message::human("Hello")];
    assert_eq!(
        tools_condition_from_messages(&messages),
        juncture_core::edge::END
    );

    // Test with AI message with tools
    let messages_with_tools = vec![Message::ai_with_tool_calls(
        "Execute",
        vec![ToolCall {
            id: "call_1".to_string(),
            name: "test_tool".to_string(),
            arguments: json!({}),
        }],
    )];
    assert_eq!(tools_condition_from_messages(&messages_with_tools), "tools");
}

#[tokio::test]
async fn test_tools_condition_state_based_integration() {
    // State with human message (no tools)
    let state = MessagesState {
        messages: vec![Message::human("Hello")],
    };
    assert_eq!(
        tools_condition(&state, "messages"),
        juncture_core::edge::END
    );

    // State with AI message with tool calls
    let state_with_tools = MessagesState {
        messages: vec![Message::ai_with_tool_calls(
            "Execute",
            vec![ToolCall {
                id: "call_1".to_string(),
                name: "test_tool".to_string(),
                arguments: json!({}),
            }],
        )],
    };
    assert_eq!(tools_condition(&state_with_tools, "messages"), "tools");

    // State with empty messages
    let empty_state = MessagesState { messages: vec![] };
    assert_eq!(
        tools_condition(&empty_state, "messages"),
        juncture_core::edge::END
    );

    // State with non-existent field name
    assert_eq!(
        tools_condition(&state, "non_existent_field"),
        juncture_core::edge::END
    );
}

#[test]
fn test_validation_node_integration() {
    let validator = ValidationNode::new()
        .with_max_tokens(1000)
        .with_validator(|messages| {
            if messages.len() > 100 {
                return Err(ToolError::validation_failed(vec![
                    "Too many messages".to_string(),
                ]));
            }
            Ok(())
        });

    validator.validate(&[]).unwrap();
    validator.validate(&[Message::human("test")]).unwrap();
}

// Rust guideline compliant 2026-05-22
