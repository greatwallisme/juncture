//! Integration tests for the tools module

use async_trait::async_trait;
use juncture::tools::{Tool, ToolError, ToolNode, ValidationNode, tools_condition};
use juncture_core::state::messages::{Message, ToolCall};
use serde_json::json;

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
    let node = ToolNode::new(tools);

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
async fn test_tools_condition_integration() {
    // Test with human message (no tools)
    let messages = vec![Message::human("Hello")];
    assert_eq!(tools_condition(&messages), juncture_core::edge::END);

    // Test with AI message with tools
    let messages_with_tools = vec![Message::ai_with_tool_calls(
        "Execute",
        vec![ToolCall {
            id: "call_1".to_string(),
            name: "test_tool".to_string(),
            arguments: json!({}),
        }],
    )];
    assert_eq!(tools_condition(&messages_with_tools), "tools");
}

#[test]
fn test_validation_node_integration() {
    let validator = ValidationNode::new()
        .with_max_tokens(1000)
        .with_validator(|messages| {
            if messages.len() > 100 {
                return Err(ToolError::validation_failed(
                    "Too many messages".to_string(),
                ));
            }
            Ok(())
        });

    validator.validate(&[]).unwrap();
    validator.validate(&[Message::human("test")]).unwrap();
}

// Rust guideline compliant 2026-05-19
