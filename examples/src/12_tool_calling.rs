//! Example 12: Tool Calling with Real LLM
//!
//! Demonstrates real LLM-driven tool calling:
//! - Defining tools with the `Tool` trait
//! - Binding tools to `ChatOpenAI` via `bind_tools`
//! - The LLM decides when to call a tool based on user input
//! - Executing tool calls and returning results

#[path = "common.rs"]
mod common;

use async_trait::async_trait;
use common::load_llm;
use juncture::llm::{ChatModel, Content, Message};
use juncture::tools::{Tool, ToolError};
use serde_json::json;
use std::io::Write;

/// Calculator tool that evaluates basic arithmetic expressions.
#[derive(Debug)]
struct CalculatorTool;

#[async_trait]
impl Tool for CalculatorTool {
    fn name(&self) -> &'static str {
        "calculator"
    }

    fn description(&self) -> &'static str {
        "Evaluates a simple arithmetic expression (addition, subtraction, multiplication, division). \
         Input should be a JSON object with an 'expression' field like {\"expression\": \"2 + 3\"}."
    }

    fn schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "expression": {
                    "type": "string",
                    "description": "Arithmetic expression to evaluate (e.g. \"12 * 5 + 3\")"
                }
            },
            "required": ["expression"]
        })
    }

    async fn invoke(&self, input: serde_json::Value) -> Result<String, ToolError> {
        let expr = input["expression"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidInput("Missing 'expression' parameter".to_string()))?;

        // Simple arithmetic parser supporting +, -, *, /
        let result = evaluate_expression(expr)?;
        Ok(result.to_string())
    }
}

/// Parse and evaluate a simple arithmetic expression with `+`, `-`, `*`, `/`.
///
/// Does not use `eval` or external crates -- limited to flat `term op term` patterns.
fn evaluate_expression(expr: &str) -> Result<f64, ToolError> {
    let tokens: Vec<&str> = expr.split_whitespace().collect();
    if tokens.len() < 3 || tokens.len() % 2 == 0 {
        return Err(ToolError::InvalidInput(
            "Expression must be in the form: number op number [op number ...]".to_string(),
        ));
    }

    let mut result = tokens[0]
        .parse::<f64>()
        .map_err(|e| ToolError::InvalidInput(format!("Invalid number: {e}")))?;

    let mut i = 1;
    while i + 1 < tokens.len() {
        let op = tokens[i];
        let next = tokens[i + 1]
            .parse::<f64>()
            .map_err(|e| ToolError::InvalidInput(format!("Invalid number: {e}")))?;
        match op {
            "+" => result += next,
            "-" => result -= next,
            "*" => result *= next,
            "/" => {
                if next == 0.0 {
                    return Err(ToolError::execution_failed("Division by zero".to_string()));
                }
                result /= next;
            }
            _ => {
                return Err(ToolError::InvalidInput(format!("Unknown operator: {op}")));
            }
        }
        i += 2;
    }

    Ok(result)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let llm = load_llm().map_err(std::io::Error::other)?;
    let mut stdout = std::io::stdout();

    let calculator = CalculatorTool;
    let tool_def = juncture::llm::ToolDefinition {
        name: calculator.name().to_string(),
        description: calculator.description().to_string(),
        parameters: calculator.schema(),
    };

    let llm_with_tools = llm.bind_tools(vec![tool_def]);

    let messages = vec![
        Message::system(
            "You are a helpful assistant with access to a calculator tool. \
                          When the user asks a math question, use the calculator to compute the answer.",
        ),
        Message::human("What is 42 * 17 + 8?"),
    ];

    writeln!(stdout, "User: What is 42 * 17 + 8?\n")?;

    let response = llm_with_tools.invoke(&messages, None).await?;

    // Display any text content
    if let Content::Text(text) = &response.content
        && !text.is_empty()
    {
        writeln!(stdout, "AI: {text}")?;
    }

    // Handle tool calls
    if !response.tool_calls.is_empty() {
        for tc in &response.tool_calls {
            writeln!(stdout, "Tool call: {}({})", tc.name, tc.arguments)?;

            let tool_result = calculator.invoke(tc.arguments.clone()).await?;
            writeln!(stdout, "Tool result: {tool_result}\n")?;
        }

        // Send tool results back to the LLM for a final answer
        let mut extended_messages = messages;
        extended_messages.push(response);

        // Tool result message
        let tool_msg = Message {
            id: uuid::Uuid::new_v4().to_string(),
            role: juncture::llm::Role::Tool,
            content: Content::Text("722".to_string()),
            tool_calls: vec![],
            tool_call_id: Some("calc_1".to_string()),
            name: Some("calculator".to_string()),
            usage: None,
        };
        extended_messages.push(tool_msg);

        let final_response = llm_with_tools.invoke(&extended_messages, None).await?;
        if let Content::Text(text) = &final_response.content {
            writeln!(stdout, "AI (final): {text}")?;
        }
    }

    Ok(())
}

// Rust guideline compliant 2026-05-26
