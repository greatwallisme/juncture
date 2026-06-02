//! Juncture Telemetry: Real LLM agent with Langfuse cloud export
//!
//! Demonstrates the `juncture-telemetry` crate with:
//! - Real LLM interaction (no mock) with tool calling loop
//! - Nested span observations (iteration spans + LLM/tool calls)
//! - Local `SQLite` storage + embedded web dashboard
//! - Automatic export to Langfuse cloud (when configured in `.env`)
//!
//! ## Usage
//!
//! ```text
//! cp .env.example .env   # fill in OPENAI_API_KEY + LANGFUSE_* keys
//! cargo run -p juncture-simple-example --bin 16_juncture_telemetry
//! ```
//!
//! ## .env Configuration
//!
//! ```env
//! OPENAI_API_KEY=sk-...
//! OPENAI_BASE_URL=https://api.openai.com/v1
//! OPENAI_MODEL=gpt-4o
//!
//! LANGFUSE_SECRET_KEY=sk-lf-...
//! LANGFUSE_PUBLIC_KEY=pk-lf-...
//! LANGFUSE_BASE_URL=https://cloud.langfuse.com
//! ```

#[path = "common.rs"]
mod common;

use std::io::Write;

use async_trait::async_trait;
use common::load_llm;
use juncture::Message;
use juncture::llm::{ChatModel, Content, PricingTable, Role, ToolDefinition};
use juncture::tools::{Tool, ToolError};
use juncture_telemetry::{TokenUsage, init};

// ── Tools ────────────────────────────────────────────────────

#[derive(Debug)]
struct WeatherTool;

#[async_trait]
impl Tool for WeatherTool {
    fn name(&self) -> &'static str {
        "get_weather"
    }

    fn description(&self) -> &'static str {
        "Returns the current weather for a given city. Input: {\"city\": \"city name\"}"
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
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
            "beijing" => "20C, smoggy, humidity 40%",
            _ => "20C, mild conditions",
        };
        Ok(format!("Weather in {city}: {weather}"))
    }
}

#[derive(Debug)]
struct MathTool;

#[async_trait]
impl Tool for MathTool {
    fn name(&self) -> &'static str {
        "calculator"
    }

    fn description(&self) -> &'static str {
        "Evaluates a simple arithmetic expression. Input: {\"expression\": \"2 + 3 * 4\"}"
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
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

// ── Main ─────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut stdout = std::io::stdout();

    writeln!(
        stdout,
        "=== Juncture Telemetry Demo (Real Agent + Langfuse) ==="
    )?;
    writeln!(stdout)?;

    // 1. Load LLM from .env
    writeln!(stdout, "[1] Loading LLM configuration...")?;
    let llm = load_llm().map_err(std::io::Error::other)?;
    writeln!(stdout, "    Model: {}", llm.model_name())?;

    // 2. Setup telemetry -- one-liner with auto-detection
    writeln!(stdout, "[2] Setting up telemetry...")?;
    let telemetry = init()
        .with_store("telemetry-demo.db")
        .with_langfuse_from_env()
        .with_dashboard(8123)
        .install()
        .await?;
    if let Some(url) = telemetry.dashboard_url() {
        writeln!(stdout, "    Dashboard: {url}")?;
    }
    writeln!(stdout)?;

    // 3. Run real agent
    writeln!(stdout, "[3] Running real agent with tool calling...")?;
    run_agent(&llm, telemetry.collector()).await?;
    writeln!(stdout, "    Agent execution complete")?;
    writeln!(stdout)?;

    // 4. Verify local data
    writeln!(stdout, "[4] Verifying local data...")?;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!(
            "{}/api/public/traces",
            telemetry.dashboard_url().unwrap_or_default()
        ))
        .send()
        .await?;
    let traces: serde_json::Value = resp.json().await?;
    let count = traces["totalCount"].as_u64().unwrap_or(0);
    writeln!(stdout, "    Local traces: {count}")?;

    let resp = client
        .get(format!(
            "{}/api/public/stats/summary",
            telemetry.dashboard_url().unwrap_or_default()
        ))
        .send()
        .await?;
    let summary: serde_json::Value = resp.json().await?;
    writeln!(
        stdout,
        "    Observations: {}",
        summary["totalObservations"].as_u64().unwrap_or(0)
    )?;
    writeln!(
        stdout,
        "    Total tokens: {}",
        summary["totalTokens"].as_u64().unwrap_or(0)
    )?;

    writeln!(stdout)?;
    writeln!(stdout, "Press Ctrl+C to stop the server.")?;

    tokio::signal::ctrl_c().await?;
    writeln!(stdout, "\nShutting down...")?;
    telemetry.shutdown().await?;

    Ok(())
}

// ── Helpers ──────────────────────────────────────────────────

fn message_text(msg: &Message) -> String {
    match &msg.content {
        Content::Text(t) => t.clone(),
        Content::MultiPart(parts) => parts
            .iter()
            .filter_map(|p| match p {
                juncture::llm::ContentPart::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(""),
    }
}

fn calc_cost(model_name: &str, usage: &juncture::llm::TokenUsage) -> f64 {
    let pricing = PricingTable::default();
    pricing.get(model_name).map_or(0.0, |(inp, out)| {
        #[expect(
            clippy::cast_precision_loss,
            reason = "token counts are precise but float is acceptable for pricing"
        )]
        let ic = usage.input_tokens as f64 * inp / 1_000_000.0;
        #[expect(
            clippy::cast_precision_loss,
            reason = "token counts are precise but float is acceptable for pricing"
        )]
        let oc = usage.output_tokens as f64 * out / 1_000_000.0;
        ic + oc
    })
}

// ── Agent ────────────────────────────────────────────────────

/// Run a real agent with tool calling loop.
#[expect(
    clippy::too_many_lines,
    reason = "agent loop: trace setup, LLM calls, tool execution, telemetry per step"
)]
async fn run_agent(
    llm: &impl ChatModel,
    collector: &juncture_telemetry::TelemetryCollector,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut stdout = std::io::stdout();
    let model_name = llm.model_name().to_string();

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

    collector
        .track_session("demo-session-1", Some("demo-user".to_string()))
        .await?;

    let mut trace = collector
        .begin_trace("react_agent", Some("demo-session-1".to_string()))
        .await?;
    trace.user_id = Some("demo-user".to_string());
    trace.tags = vec![
        "demo".to_string(),
        "agent".to_string(),
        "tool-calling".to_string(),
    ];
    let trace_id = trace.id;

    let mut total_cost: f64 = 0.0;
    let mut total_tokens: u64 = 0;

    let mut messages: Vec<Message> = vec![
        Message::system(
            "You are a helpful assistant with access to weather and calculator tools. \
             Use tools when needed to answer questions accurately. \
             Always use tools for factual lookups and calculations.",
        ),
        Message::human(
            "What's the weather in Tokyo? Also, what is 42 * 17? \
             Give me a brief summary with both results.",
        ),
    ];

    writeln!(
        stdout,
        "    User: What's the weather in Tokyo? Also, what is 42 * 17?"
    )?;
    writeln!(stdout)?;

    let max_iterations = 10;
    for iteration in 0..max_iterations {
        writeln!(stdout, "    [iteration {}] LLM call...", iteration + 1)?;

        let iter_span =
            collector.begin_span(trace_id, None, format!("iteration_{}", iteration + 1));

        // LLM call
        let llm_obs = collector.begin_llm_call(
            trace_id,
            Some(iter_span.id),
            &model_name,
            Some(&serde_json::to_value(&messages)?),
        );

        let response = llm_with_tools.invoke(&messages, None).await?;
        let response_text = message_text(&response);
        let usage = response.usage.clone().unwrap_or_default();
        let cost = calc_cost(&model_name, &usage);

        collector
            .end_llm_call(
                llm_obs,
                if response_text.is_empty() {
                    None
                } else {
                    Some(&response_text)
                },
                Some(TokenUsage::from(usage.clone())),
                Some(cost),
            )
            .await?;

        total_cost += cost;
        total_tokens += usage.total_tokens;
        writeln!(
            stdout,
            "    [iteration {}] LLM: {} in / {} out, cost: ${:.6}",
            iteration + 1,
            usage.input_tokens,
            usage.output_tokens,
            cost
        )?;

        if let Content::Text(ref text) = response.content
            && !text.is_empty()
        {
            writeln!(stdout, "    AI: {text}")?;
        }

        messages.push(response.clone());

        if response.tool_calls.is_empty() {
            writeln!(
                stdout,
                "    [iteration {}] No tool calls - agent finished",
                iteration + 1
            )?;
            collector.end_span(iter_span, None).await?;
            break;
        }

        // Tool calls
        for tc in &response.tool_calls {
            writeln!(stdout, "    [tool] {}({})", tc.name, tc.arguments)?;

            let tool_obs = collector.begin_tool_call(
                trace_id,
                Some(iter_span.id),
                &tc.name,
                Some(&tc.arguments),
            );

            let tool_result = match tc.name.as_str() {
                "get_weather" => weather.invoke(tc.arguments.clone()).await?,
                "calculator" => calculator.invoke(tc.arguments.clone()).await?,
                _ => format!("Unknown tool: {}", tc.name),
            };

            collector
                .end_tool_call(tool_obs, Some(serde_json::json!({"result": &tool_result})))
                .await?;

            writeln!(stdout, "    [tool] result: {tool_result}")?;

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

        collector.end_span(iter_span, None).await?;
        writeln!(stdout)?;
    }

    let final_answer = messages.last().map(message_text).unwrap_or_default();

    collector
        .end_trace(
            trace,
            Some(serde_json::json!({
                "answer": final_answer,
                "model": model_name,
            })),
            Some(total_cost),
            Some(total_tokens),
        )
        .await?;

    collector.flush().await?;

    writeln!(stdout, "    Total: {total_tokens} tokens, ${total_cost:.6}")?;

    Ok(())
}

// Rust guideline compliant 2026-06-02
