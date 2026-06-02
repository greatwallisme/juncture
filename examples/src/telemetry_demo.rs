//! Telemetry Demo: End-to-end `OTel` pipeline with real LLM + tools
//!
//! Runs a multi-node agent graph with real LLM calls, tool execution,
//! conditional routing, and an error path. All telemetry (traces, metrics,
//! callbacks) flows through `OTel` Collector to Jaeger and Prometheus.
//!
//! ## Prerequisites
//!
//! ```text
//! # 1. Start the telemetry stack
//! docker compose -f docker/telemetry/docker-compose.yml up -d
//!
//! # 2. Configure LLM credentials
//! cp examples/.env.example examples/.env
//! # Edit .env with your OPENAI_API_KEY
//! ```
//!
//! ## Usage
//!
//! ```text
//! cargo run -p juncture-simple-example --bin telemetry_demo
//! ```
//!
//! ## Telemetry Coverage
//!
//! | Dimension | Metric/Span | Source |
//! |-----------|-------------|--------|
//! | LLM calls | `juncture.llm.call` span, `juncture.llm.calls` counter | `ChatOpenAI` provider |
//! | LLM tokens | `juncture.tokens.input/output` | `ChatOpenAI` provider |
//! | Tool calls | `juncture.tool.call` span | Manual span in tools node (weather, calculator, search) |
//! | Graph lifecycle | `juncture.graph.invocations`, `duration_ms` | `MetricsCollector` |
//! | Node execution | `juncture.node.duration_ms` | `MetricsCollector` |
//! | Multi-superstep | agent->tools->agent loop | Conditional routing |
//! | Error path | `juncture.graph.errors`, `on_node_error` | Error node + `MetricsCollector` |
//! | Callbacks | `on_node_start/end`, `on_graph_end` | `GraphCallbackHandler` |
//!
//! ## Verification
//!
//! ```text
//! Jaeger:     http://localhost:16686  (service: juncture-telemetry-demo)
//! Prometheus: http://localhost:9090   (query: juncture_graph_invocations_total)
//! ```

#[path = "common.rs"]
mod common;

use std::fmt::Write as _;
use std::io::Write;
use std::sync::Arc;

use async_trait::async_trait;
use common::load_llm;
use juncture::llm::{ChatModel, Content, Message, Role, ToolDefinition};
use juncture::tools::{Tool, ToolError, tools_condition_from_messages};
use juncture_core::edge::{END, PathMap, RouteResult, Router};
use juncture_core::node::NodeFnUpdate;
use juncture_core::observability::{GraphLifecycleCallback, MetricsCollector};
use juncture_core::state::messages::{MessagesState, MessagesStateUpdate};
use juncture_core::{JunctureError, RunnableConfig, StateGraph};
use juncture_tracing::callback::{CallbackHandlerAdapter, GraphCallbackHandler};
use juncture_tracing::{RegistryMetricsCollector, init};
use serde::Deserialize;
use serde_json::json;

// ---------------------------------------------------------------------------
// Tools
// ---------------------------------------------------------------------------

/// Weather lookup tool -- returns simulated weather data for a city.
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

/// Calculator tool -- evaluates simple arithmetic expressions.
#[derive(Debug)]
struct CalculatorTool;

#[async_trait]
impl Tool for CalculatorTool {
    fn name(&self) -> &'static str {
        "calculator"
    }

    fn description(&self) -> &'static str {
        "Evaluates a simple arithmetic expression. Input: {\"expression\": \"2 + 3 * 4\"}"
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

// ---------------------------------------------------------------------------
// Web search tool (Tavily API)
// ---------------------------------------------------------------------------

/// Web search tool powered by Tavily Search API.
#[derive(Debug)]
struct TavilySearchTool {
    api_key: Option<String>,
}

#[derive(Deserialize)]
struct TavilyResponse {
    results: Vec<TavilyResult>,
}

#[derive(Deserialize)]
struct TavilyResult {
    title: String,
    url: String,
    content: String,
}

#[async_trait]
impl Tool for TavilySearchTool {
    fn name(&self) -> &'static str {
        "web_search"
    }

    fn description(&self) -> &'static str {
        "Search the web for current information on any topic. \
         Use when you need up-to-date facts, recent news, or current events. \
         Input: {\"query\": \"search query string\"}"
    }

    fn schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query to find relevant information"
                }
            },
            "required": ["query"]
        })
    }

    async fn invoke(&self, input: serde_json::Value) -> Result<String, ToolError> {
        let api_key = self.api_key.as_ref().ok_or_else(|| {
            ToolError::execution_failed(
                "TAVILY_API_KEY not configured. Set the environment variable to enable web search."
                    .to_string(),
            )
        })?;

        let query = input["query"]
            .as_str()
            .ok_or_else(|| ToolError::invalid_input("Missing 'query' parameter".to_string()))?;

        let client = reqwest::Client::new();
        let response = client
            .post("https://api.tavily.com/search")
            .json(&json!({
                "api_key": api_key,
                "query": query,
                "max_results": 3,
                "search_depth": "basic"
            }))
            .send()
            .await
            .map_err(|e| ToolError::execution_failed(format!("HTTP request failed: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            return Err(ToolError::execution_failed(format!(
                "Tavily API error {status}: {error_text}"
            )));
        }

        let tavily: TavilyResponse = response
            .json()
            .await
            .map_err(|e| ToolError::execution_failed(format!("Failed to parse response: {e}")))?;

        if tavily.results.is_empty() {
            return Ok("No results found.".to_string());
        }

        let mut output = String::from("Search results:\n\n");
        for (i, r) in tavily.results.iter().enumerate() {
            let _ = writeln!(
                output,
                "{}. {}\n   URL: {}\n   {}\n",
                i + 1,
                r.title,
                r.url,
                r.content
            );
        }
        Ok(output)
    }
}

// ---------------------------------------------------------------------------
// Router for conditional edges
// ---------------------------------------------------------------------------

/// Routes agent output to "tools" (if tool calls present) or to finish point.
struct AgentRouter;

impl Router<MessagesState> for AgentRouter {
    fn route(
        &self,
        state: &MessagesState,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<RouteResult, JunctureError>> + Send + '_>,
    > {
        let target = tools_condition_from_messages(&state.messages).to_string();
        Box::pin(async move { Ok(RouteResult::One(target)) })
    }
}

// ---------------------------------------------------------------------------
// Callback handler
// ---------------------------------------------------------------------------

struct TelemetryCallback;

impl GraphCallbackHandler for TelemetryCallback {
    fn on_node_start(&self, node: &str, task_id: &str) {
        let _ = writeln!(
            std::io::stdout(),
            "  [cb] node_start  node={node}  task={task_id}"
        );
    }

    fn on_node_end(&self, node: &str, task_id: &str, duration_ms: u64) {
        let _ = writeln!(
            std::io::stdout(),
            "  [cb] node_end    node={node}  task={task_id}  {duration_ms}ms"
        );
    }

    fn on_node_error(&self, node: &str, error: &JunctureError) {
        let _ = writeln!(
            std::io::stdout(),
            "  [cb] node_error  node={node}  err={error}"
        );
    }

    fn on_graph_end(&self, result: &Result<(), JunctureError>) {
        let status = if result.is_ok() { "ok" } else { "error" };
        let _ = writeln!(std::io::stdout(), "  [cb] graph_end   status={status}");
    }
}

// ---------------------------------------------------------------------------
// Tool execution helper
// ---------------------------------------------------------------------------

async fn execute_tool_call(tc: &juncture::llm::ToolCall) -> Message {
    let span = tracing::info_span!(
        "juncture.tool.call",
        juncture.tool.name = tc.name.as_str(),
        juncture.tool.duration_ms = tracing::field::Empty,
    );
    let _enter = span.enter();
    let start = std::time::Instant::now();

    let result = match tc.name.as_str() {
        "get_weather" => WeatherTool.invoke(tc.arguments.clone()).await,
        "calculator" => CalculatorTool.invoke(tc.arguments.clone()).await,
        "web_search" => {
            let search = TavilySearchTool {
                api_key: std::env::var("TAVILY_API_KEY").ok(),
            };
            search.invoke(tc.arguments.clone()).await
        }
        _ => Err(ToolError::invalid_input(format!(
            "Unknown tool: {}",
            tc.name
        ))),
    };

    let duration_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
    span.record("juncture.tool.duration_ms", duration_ms);

    let output = match result {
        Ok(val) => val,
        Err(e) => format!("Error: {e}"),
    };

    Message {
        id: uuid::Uuid::new_v4().to_string(),
        role: Role::Tool,
        content: Content::Text(output),
        tool_calls: vec![],
        tool_call_id: Some(tc.id.clone()),
        name: Some(tc.name.clone()),
        usage: None,
    }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _ = dotenvy::dotenv();
    let mut stdout = std::io::stdout();

    // 1. Load config from .env
    let otlp_endpoint = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT")
        .unwrap_or_else(|_| "http://127.0.0.1:4318".to_string());
    let service_name = std::env::var("OTEL_SERVICE_NAME")
        .unwrap_or_else(|_| "juncture-telemetry-demo".to_string());

    // 2. Initialize OTel pipeline
    let metrics_registry = init()
        .with_service_name(&service_name)
        .with_otlp_endpoint(&otlp_endpoint)
        .with_metrics(true)
        .install()?
        .expect("metrics enabled");

    writeln!(stdout, "[demo] OTel pipeline -> {otlp_endpoint}")?;
    writeln!(stdout, "[demo] service name  -> {service_name}")?;

    // 2. Wire metrics + callbacks
    let metrics_collector: Arc<dyn MetricsCollector> =
        Arc::new(RegistryMetricsCollector::new(metrics_registry));
    let callback_handler: Arc<dyn GraphLifecycleCallback> =
        Arc::new(CallbackHandlerAdapter::new(Arc::new(TelemetryCallback)));

    let config = RunnableConfig::new()
        .with_metrics_collector(metrics_collector)
        .with_callback_handler(callback_handler);

    // 3. Load real LLM from .env
    let llm = load_llm().map_err(std::io::Error::other)?;
    writeln!(stdout, "[demo] LLM loaded: model={}", llm.model_name())?;

    // 4. Set up tools and bind to LLM
    let tavily_key = std::env::var("TAVILY_API_KEY").ok();
    let search_tool = TavilySearchTool {
        api_key: tavily_key.clone(),
    };
    if tavily_key.is_some() {
        writeln!(stdout, "[demo] Tavily search enabled")?;
    } else {
        writeln!(
            stdout,
            "[demo] TAVILY_API_KEY not set, search tool will error on use"
        )?;
    }

    let tool_defs = vec![
        ToolDefinition {
            name: WeatherTool.name().to_string(),
            description: WeatherTool.description().to_string(),
            parameters: WeatherTool.schema(),
        },
        ToolDefinition {
            name: CalculatorTool.name().to_string(),
            description: CalculatorTool.description().to_string(),
            parameters: CalculatorTool.schema(),
        },
        ToolDefinition {
            name: search_tool.name().to_string(),
            description: search_tool.description().to_string(),
            parameters: search_tool.schema(),
        },
    ];
    let llm_with_tools = Arc::new(llm.bind_tools(tool_defs));

    // 5. Build agent graph
    let compiled = build_agent_graph(&llm_with_tools)?;

    // 6. Run the main agent graph
    run_agent_graph(&compiled, &config, &mut stdout).await?;

    // 7. Run error path graph
    run_error_graph(&config, &mut stdout).await?;

    // 8. Flush metrics before exit (PeriodicReader exports every 5s)
    writeln!(stdout, "[demo] Waiting for metrics export...")?;
    tokio::time::sleep(std::time::Duration::from_secs(8)).await;

    // 9. Print verification instructions
    print_verification_summary(&mut stdout, &service_name)?;

    Ok(())
}

/// Build the agent graph: agent -> (tools | summarize), tools -> agent
fn build_agent_graph(
    llm: &Arc<juncture::llm::ChatOpenAI>,
) -> Result<
    juncture_core::graph::CompiledGraph<MessagesState, MessagesState, MessagesState>,
    Box<dyn std::error::Error>,
> {
    let mut graph = StateGraph::<MessagesState>::new();

    // Agent node: calls real LLM with tools bound.
    // The ChatOpenAI provider emits juncture.llm.call span with token/duration
    // metrics automatically via OTel.
    let llm_agent = Arc::clone(llm);
    graph.add_node_simple(
        "agent",
        NodeFnUpdate(move |state: &MessagesState| {
            let llm = Arc::clone(&llm_agent);
            let messages = state.messages.clone();
            async move {
                let response = llm
                    .invoke(&messages, None)
                    .await
                    .map_err(|e| JunctureError::execution(format!("LLM call failed: {e}")))?;
                Ok(MessagesStateUpdate {
                    messages: Some(vec![response]),
                })
            }
        }),
    )?;

    // Tools node: executes tool calls from the last AI message.
    graph.add_node_simple(
        "tools",
        NodeFnUpdate(|state: &MessagesState| {
            let last_msg = state.messages.last().cloned();
            async move {
                let Some(msg) = last_msg else {
                    return Ok(MessagesStateUpdate::default());
                };

                let mut tool_results = Vec::new();
                for tc in &msg.tool_calls {
                    tool_results.push(execute_tool_call(tc).await);
                }

                Ok(MessagesStateUpdate {
                    messages: Some(tool_results),
                })
            }
        }),
    )?;

    // Summarize node: adds a final summary after agent completes.
    graph.add_node_simple(
        "summarize",
        NodeFnUpdate(|state: &MessagesState| {
            let last_content = state
                .messages
                .last()
                .and_then(|m| match &m.content {
                    Content::Text(t) => Some(t.clone()),
                    Content::MultiPart(_) => None,
                })
                .unwrap_or_default();
            async move {
                let summary = Message::ai(format!(
                    "Summary: I processed your request. Last response: {last_content}"
                ));
                Ok(MessagesStateUpdate {
                    messages: Some(vec![summary]),
                })
            }
        }),
    )?;

    // Conditional routing: agent -> tools (if tool_calls) or -> summarize
    let mut path_map = PathMap::new();
    path_map.insert("tools", "tools");
    path_map.insert(END, "summarize");
    graph.add_conditional_edges("agent", Arc::new(AgentRouter), path_map);

    // Tools -> agent (loop back for LLM to process tool results)
    graph.add_edge("tools", "agent");

    graph.set_entry_point("agent");
    graph.set_finish_point("summarize");

    Ok(graph.compile()?)
}

/// Run the agent graph with a real LLM query
async fn run_agent_graph(
    compiled: &juncture_core::graph::CompiledGraph<MessagesState, MessagesState, MessagesState>,
    config: &RunnableConfig,
    stdout: &mut impl Write,
) -> Result<(), Box<dyn std::error::Error>> {
    writeln!(stdout, "\n[demo] === Agent Graph (real LLM + tools) ===")?;

    let initial_state = MessagesState {
        messages: vec![
            Message::system(
                "You are a helpful assistant. Use tools when needed. \
                 Use web_search for current information, get_weather for weather, \
                 and calculator for math.",
            ),
            Message::human(
                "Search for recent AI news, check the weather in Tokyo, \
                 and calculate 42 * 17.",
            ),
        ],
    };

    let output = compiled.invoke_async(initial_state, config).await?;

    writeln!(stdout, "\n[demo] Conversation:")?;
    for msg in &output.value.messages {
        let prefix = match msg.role {
            Role::Human => "Human",
            Role::Ai => "AI",
            Role::System => "System",
            Role::Tool => "Tool",
        };
        if let Content::Text(text) = &msg.content
            && !text.is_empty()
        {
            let display = if text.len() > 120 {
                format!("{}...", &text[..120])
            } else {
                text.clone()
            };
            writeln!(stdout, "  {prefix}: {display}")?;
        }
        for tc in &msg.tool_calls {
            writeln!(stdout, "  [tool_call: {}({})]", tc.name, tc.arguments)?;
        }
    }

    writeln!(stdout, "\n[demo] Steps executed: {}", output.metadata.steps)?;
    Ok(())
}

/// Run a graph with a deliberately failing node to exercise error metrics
async fn run_error_graph(
    config: &RunnableConfig,
    stdout: &mut impl Write,
) -> Result<(), Box<dyn std::error::Error>> {
    writeln!(stdout, "\n[demo] === Error Path Test ===")?;

    let mut error_graph = StateGraph::<MessagesState>::new();
    error_graph.add_node_simple(
        "error_node",
        NodeFnUpdate(|_state: &MessagesState| async move {
            Err(JunctureError::execution(
                "Deliberate test error for telemetry verification",
            ))
        }),
    )?;
    error_graph.set_entry_point("error_node");
    error_graph.set_finish_point("error_node");
    let error_compiled = error_graph.compile()?;

    let error_state = MessagesState {
        messages: vec![Message::human("trigger error")],
    };
    match error_compiled.invoke_async(error_state, config).await {
        Ok(_) => writeln!(stdout, "[demo] Error graph unexpectedly succeeded")?,
        Err(e) => writeln!(stdout, "[demo] Error graph failed as expected: {e}")?,
    }
    Ok(())
}

/// Print verification instructions
fn print_verification_summary(
    stdout: &mut impl Write,
    service_name: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    writeln!(stdout, "\n---")?;
    writeln!(stdout, "Telemetry verification:")?;
    writeln!(
        stdout,
        "  Jaeger:     http://localhost:16686  (service: {service_name})"
    )?;
    writeln!(
        stdout,
        "  Prometheus: http://localhost:9090   (query: juncture_graph_invocations_total)"
    )?;
    writeln!(stdout)?;
    writeln!(stdout, "Expected metrics in Prometheus:")?;
    writeln!(
        stdout,
        "  juncture_graph_invocations_total  (agent + error graphs)"
    )?;
    writeln!(stdout, "  juncture_graph_errors_total       (error graph)")?;
    writeln!(
        stdout,
        "  juncture_node_duration_ms         (per-node histogram)"
    )?;
    writeln!(
        stdout,
        "  juncture_graph_duration_ms        (per-graph histogram)"
    )?;
    writeln!(stdout)?;
    writeln!(stdout, "Expected traces in Jaeger:")?;
    writeln!(stdout, "  juncture.graph.invoke             (graph span)")?;
    writeln!(
        stdout,
        "  juncture.node.execute             (per-node spans)"
    )?;
    writeln!(
        stdout,
        "  juncture.llm.call                 (LLM provider span)"
    )?;
    writeln!(
        stdout,
        "  juncture.tool.call                (tool execution span)"
    )?;
    Ok(())
}

// Rust guideline compliant 2026-05-29
