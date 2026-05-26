//! Example 13: Agent with Tool Calling Loop
//!
//! Demonstrates building an agent loop with real LLM and tools:
//! - Manually constructing an agent + tools graph (instead of prebuilt)
//! - Using `ChatModel::invoke` with `bind_tools` for LLM-driven tool selection
//! - Executing tools and feeding results back to the LLM
//!
//! Run: `cargo run -p juncture-simple-example --bin 13_react_agent`

#[path = "common.rs"]
mod common;

use async_trait::async_trait;
use common::load_llm;
use juncture::llm::{ChatModel, Content, Message, Role, ToolDefinition};
use juncture::tools::{Tool, ToolError};
use serde_json::json;
use std::io::Write;

/// Tool that looks up the current weather for a city.
#[derive(Debug)]
struct WeatherTool;

#[async_trait]
impl Tool for WeatherTool {
    fn name(&self) -> &'static str {
        "get_weather"
    }

    fn description(&self) -> &'static str {
        "Returns the current weather for a given city. \
         Input: {\"city\": \"city name\"}"
    }

    fn schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "city": {
                    "type": "string",
                    "description": "City name to get weather for"
                }
            },
            "required": ["city"]
        })
    }

    async fn invoke(&self, input: serde_json::Value) -> Result<String, ToolError> {
        let city = input["city"]
            .as_str()
            .ok_or_else(|| ToolError::invalid_input("Missing 'city' parameter".to_string()))?;

        let weather = match city.to_lowercase().as_str() {
            "tokyo" => "22C, partly cloudy, humidity 65%",
            "london" => "15C, rainy, humidity 80%",
            "new york" | "new york city" => "18C, clear skies, humidity 55%",
            "paris" => "17C, overcast, humidity 70%",
            _ => "20C, mild conditions",
        };
        Ok(format!("Weather in {city}: {weather}"))
    }
}

/// Tool that performs arithmetic calculations.
#[derive(Debug)]
struct MathTool;

#[async_trait]
impl Tool for MathTool {
    fn name(&self) -> &'static str {
        "calculator"
    }

    fn description(&self) -> &'static str {
        "Evaluates a simple arithmetic expression. \
         Input: {\"expression\": \"2 + 3 * 4\"}"
    }

    fn schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "expression": {
                    "type": "string",
                    "description": "Arithmetic expression (supports +, -, *, /)"
                }
            },
            "required": ["expression"]
        })
    }

    async fn invoke(&self, input: serde_json::Value) -> Result<String, ToolError> {
        let expr = input["expression"].as_str().ok_or_else(|| {
            ToolError::invalid_input("Missing 'expression' parameter".to_string())
        })?;

        let tokens: Vec<&str> = expr.split_whitespace().collect();
        if tokens.len() < 3 {
            return Err(ToolError::invalid_input(
                "Expression must have at least: number op number".to_string(),
            ));
        }

        let mut result = tokens[0]
            .parse::<f64>()
            .map_err(|e| ToolError::invalid_input(format!("Invalid number: {e}")))?;

        let mut i = 1;
        while i + 1 < tokens.len() {
            let op = tokens[i];
            let next = tokens[i + 1]
                .parse::<f64>()
                .map_err(|e| ToolError::invalid_input(format!("Invalid number: {e}")))?;
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
                _ => return Err(ToolError::invalid_input(format!("Unknown operator: {op}"))),
            }
            i += 2;
        }
        Ok(result.to_string())
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let llm = load_llm().map_err(std::io::Error::other)?;
    let mut stdout = std::io::stdout();

    let weather = WeatherTool;
    let calculator = MathTool;

    let tool_defs = vec![
        ToolDefinition {
            name: weather.name().to_string(),
            description: weather.description().to_string(),
            parameters: weather.schema(),
        },
        ToolDefinition {
            name: calculator.name().to_string(),
            description: calculator.description().to_string(),
            parameters: calculator.schema(),
        },
    ];

    let llm_with_tools = llm.bind_tools(tool_defs);

    let mut messages: Vec<Message> = vec![
        Message::system(
            "You are a helpful assistant. Use tools when needed to answer questions accurately.",
        ),
        Message::human("What's the weather in Tokyo? Also, what is 42 * 17?"),
    ];

    writeln!(
        stdout,
        "User: What's the weather in Tokyo? Also, what is 42 * 17?\n"
    )?;

    // Agent loop: call LLM, execute tools, repeat until no more tool calls
    let max_iterations = 10;
    for iteration in 0..max_iterations {
        let response = llm_with_tools.invoke(&messages, None).await?;
        messages.push(response.clone());

        if let Content::Text(text) = &response.content
            && !text.is_empty()
        {
            writeln!(stdout, "AI: {text}")?;
        }

        if response.tool_calls.is_empty() {
            break;
        }

        // Execute each tool call
        for tc in &response.tool_calls {
            writeln!(stdout, "  [Tool call: {}({})]", tc.name, tc.arguments)?;

            let tool_result = match tc.name.as_str() {
                "get_weather" => weather.invoke(tc.arguments.clone()).await?,
                "calculator" => calculator.invoke(tc.arguments.clone()).await?,
                _ => format!("Unknown tool: {}", tc.name),
            };
            writeln!(stdout, "  [Tool result: {tool_result}]")?;

            let tool_msg = Message {
                id: uuid::Uuid::new_v4().to_string(),
                role: Role::Tool,
                content: Content::Text(tool_result),
                tool_calls: vec![],
                tool_call_id: Some(tc.id.clone()),
                name: Some(tc.name.clone()),
                usage: None,
            };
            messages.push(tool_msg);
        }

        writeln!(stdout, "--- iteration {} complete ---\n", iteration + 1)?;
    }

    Ok(())
}

// Rust guideline compliant 2026-05-26
