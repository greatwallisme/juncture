//! WASI HTTP Server Example: Juncture Agent Edge Service on Spin
//!
//! Demonstrates a full ReAct agent loop on WASI with real LLM interaction,
//! tool calling, multi-turn reasoning, and Juncture graph integration.
//!
//! # Agent capabilities
//!
//! - Calculator: evaluates arithmetic expressions with step-by-step reasoning
//! - Text analysis: word/char/sentence counting, readability scoring
//! - Knowledge lookup: structured knowledge base queries
//! - Multi-turn tool calling loop: LLM -> tools -> LLM -> ... until answer
//!
//! # Environment variables (configure in spin.toml or pass via spin CLI)
//!
//! - `OPENAI_API_KEY` (required)
//! - `OPENAI_BASE_URL` (optional, default: https://api.openai.com/v1)
//! - `OPENAI_MODEL` (optional, default: gpt-4o)
//!
//! # Build & Run
//!
//!   spin build
//!   OPENAI_API_KEY=<your-key> spin up
//!
//! # Test
//!
//!   curl -X POST http://127.0.0.1:3000/ -H "Content-Type: application/json" \
//!     -d '{"message": "Calculate 15% of 2847, then explain the result"}'

use juncture_core::node::NodeFnUpdate;
use juncture_core::{RunnableConfig, StateGraph};
use juncture_derive::State;
use serde::Deserialize;
use spin_sdk::http::{IntoResponse, Method, Request, Response};
use spin_sdk::http_component;

/// Maximum agent loop iterations before forced stop.
const MAX_ITERATIONS: usize = 8;

/// State for the text analysis graph (runs in parallel with LLM).
#[derive(State, Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
struct TextState {
    input: String,
    word_count: usize,
    char_count: usize,
    sentence_count: usize,
    avg_word_length: f64,
    readability: String,
    summary: String,
}

/// Incoming request body.
#[derive(Deserialize)]
struct RequestBody {
    message: String,
}

/// Tool call parsed from LLM response.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct ToolCall {
    id: String,
    name: String,
    arguments: serde_json::Value,
}

/// Build a richer Juncture text analysis graph.
fn build_graph() -> StateGraph<TextState> {
    let mut graph = StateGraph::<TextState>::new();

    graph
        .add_node_simple(
            "count",
            NodeFnUpdate(|state: &TextState| {
                let input = state.input.clone();
                async move {
                    let word_count = input.split_whitespace().count();
                    let char_count = input.chars().count();
                    let sentence_count = input
                        .split(|c: char| c == '.' || c == '!' || c == '?')
                        .filter(|s| !s.trim().is_empty())
                        .count()
                        .max(1);
                    Ok(TextStateUpdate {
                        input: None,
                        word_count: Some(word_count),
                        char_count: Some(char_count),
                        sentence_count: Some(sentence_count),
                        avg_word_length: None,
                        readability: None,
                        summary: None,
                    })
                }
            }),
        )
        .expect("failed to add count node");

    graph
        .add_node_simple(
            "analyze",
            NodeFnUpdate(|state: &TextState| {
                let word_count = state.word_count;
                let char_count = state.char_count;
                async move {
                    let avg_word_length = if word_count > 0 {
                        (char_count as f64) / (word_count as f64)
                    } else {
                        0.0
                    };
                    let readability = match avg_word_length {
                        x if x < 4.0 => "simple".to_string(),
                        x if x < 5.5 => "moderate".to_string(),
                        x if x < 7.0 => "complex".to_string(),
                        _ => "very complex".to_string(),
                    };
                    Ok(TextStateUpdate {
                        input: None,
                        word_count: None,
                        char_count: None,
                        sentence_count: None,
                        avg_word_length: Some(avg_word_length),
                        readability: Some(readability),
                        summary: None,
                    })
                }
            }),
        )
        .expect("failed to add analyze node");

    graph
        .add_node_simple(
            "summary",
            NodeFnUpdate(|state: &TextState| {
                let input = state.input.clone();
                let word_count = state.word_count;
                let char_count = state.char_count;
                let sentence_count = state.sentence_count;
                let avg_word_length = state.avg_word_length;
                let readability = state.readability.clone();
                async move {
                    let truncated = if input.len() > 60 {
                        format!("{}...", &input[..60])
                    } else {
                        input
                    };
                    let summary = format!(
                        "{word_count} words, {char_count} chars, {sentence_count} sentences, \
                         avg word length {avg_word_length:.1}, readability: {readability}. \
                         Preview: \"{truncated}\""
                    );
                    Ok(TextStateUpdate {
                        input: None,
                        word_count: None,
                        char_count: None,
                        sentence_count: None,
                        avg_word_length: None,
                        readability: None,
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

/// Get the tool definitions for the LLM.
fn tool_definitions() -> serde_json::Value {
    serde_json::json!([
        {
            "type": "function",
            "function": {
                "name": "calculator",
                "description": "Evaluate a mathematical expression. Supports +, -, *, /, parentheses, and power (^). Use for any calculation, no matter how simple or complex.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "expression": {
                            "type": "string",
                            "description": "Mathematical expression to evaluate, e.g. '(15 * 2847) / 100'"
                        },
                        "reasoning": {
                            "type": "string",
                            "description": "Brief explanation of what this calculation computes and why"
                        }
                    },
                    "required": ["expression", "reasoning"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "text_analyze",
                "description": "Analyze text structure: word count, character count, sentence count, readability score. Use when asked about text statistics or readability.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "text": {
                            "type": "string",
                            "description": "Text to analyze"
                        }
                    },
                    "required": ["text"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "knowledge_lookup",
                "description": "Look up factual information from a structured knowledge base. Use for factual questions about science, history, geography, technology, etc.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "topic": {
                            "type": "string",
                            "description": "Topic to look up (e.g., 'speed of light', 'population of Tokyo', 'Rust programming language')"
                        }
                    },
                    "required": ["topic"]
                }
            }
        }
    ])
}

/// Execute a tool and return the result.
async fn execute_tool(name: &str, args: &serde_json::Value) -> String {
    match name {
        "calculator" => execute_calculator(args),
        "text_analyze" => execute_text_analyze(args).await,
        "knowledge_lookup" => execute_knowledge_lookup(args),
        _ => format!("Unknown tool: {name}"),
    }
}

/// Evaluate a mathematical expression.
fn execute_calculator(args: &serde_json::Value) -> String {
    let expr = match args["expression"].as_str() {
        Some(e) => e,
        None => return "Error: missing 'expression' parameter".to_string(),
    };
    let reasoning = args["reasoning"]
        .as_str()
        .unwrap_or("no reasoning provided");

    match eval_expression(expr) {
        Ok(result) => {
            // Format result nicely: remove trailing zeros for clean display
            let formatted = if (result - result.round()).abs() < 1e-10 {
                format!("{}", result as i64)
            } else {
                format!("{result:.6}")
                .trim_end_matches('0')
                .trim_end_matches('.')
                .to_string()
            };
            format!("Calculation: {expr} = {formatted} (reasoning: {reasoning})")
        }
        Err(e) => format!("Calculation error for '{expr}': {e}"),
    }
}

/// Simple expression evaluator supporting +, -, *, /, ^, parentheses.
fn eval_expression(expr: &str) -> Result<f64, String> {
    let tokens = tokenize(expr)?;
    let mut pos = 0;
    let result = parse_addition(&tokens, &mut pos)?;
    if pos < tokens.len() {
        return Err(format!("Unexpected token at position {pos}"));
    }
    Ok(result)
}

#[derive(Debug)]
enum Token {
    Num(f64),
    Op(char),
    LParen,
    RParen,
}

fn tokenize(expr: &str) -> Result<Vec<Token>, String> {
    let mut tokens = Vec::new();
    let chars: Vec<char> = expr.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            ' ' => {}
            '(' => tokens.push(Token::LParen),
            ')' => tokens.push(Token::RParen),
            '+' | '-' | '*' | '/' | '^' => tokens.push(Token::Op(chars[i])),
            '0'..='9' | '.' => {
                let start = i;
                while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                    i += 1;
                }
                let num_str: String = chars[start..i].iter().collect();
                let num = num_str
                    .parse::<f64>()
                    .map_err(|_| format!("Invalid number: {num_str}"))?;
                tokens.push(Token::Num(num));
                continue;
            }
            c => return Err(format!("Unexpected character: '{c}'")),
        }
        i += 1;
    }
    Ok(tokens)
}

fn parse_addition(tokens: &[Token], pos: &mut usize) -> Result<f64, String> {
    let mut left = parse_multiplication(tokens, pos)?;
    while *pos < tokens.len() {
        if let Token::Op(op @ ('+' | '-')) = &tokens[*pos] {
            *pos += 1;
            let right = parse_multiplication(tokens, pos)?;
            if *op == '+' {
                left += right;
            } else {
                left -= right;
            }
        } else {
            break;
        }
    }
    Ok(left)
}

fn parse_multiplication(tokens: &[Token], pos: &mut usize) -> Result<f64, String> {
    let mut left = parse_power(tokens, pos)?;
    while *pos < tokens.len() {
        if let Token::Op(op @ ('*' | '/')) = &tokens[*pos] {
            *pos += 1;
            let right = parse_power(tokens, pos)?;
            if *op == '*' {
                left *= right;
            } else {
                if right == 0.0 {
                    return Err("Division by zero".to_string());
                }
                left /= right;
            }
        } else {
            break;
        }
    }
    Ok(left)
}

fn parse_power(tokens: &[Token], pos: &mut usize) -> Result<f64, String> {
    let base = parse_unary(tokens, pos)?;
    if *pos < tokens.len() && matches!(tokens[*pos], Token::Op('^')) {
        *pos += 1;
        let exp = parse_power(tokens, pos)?;
        Ok(base.powf(exp))
    } else {
        Ok(base)
    }
}

fn parse_unary(tokens: &[Token], pos: &mut usize) -> Result<f64, String> {
    if *pos < tokens.len() {
        if let Token::Op('-') = tokens[*pos] {
            *pos += 1;
            let val = parse_primary(tokens, pos)?;
            return Ok(-val);
        }
    }
    parse_primary(tokens, pos)
}

fn parse_primary(tokens: &[Token], pos: &mut usize) -> Result<f64, String> {
    if *pos >= tokens.len() {
        return Err("Unexpected end of expression".to_string());
    }
    match &tokens[*pos] {
        Token::Num(n) => {
            *pos += 1;
            Ok(*n)
        }
        Token::LParen => {
            *pos += 1;
            let val = parse_addition(tokens, pos)?;
            if *pos >= tokens.len() || !matches!(tokens[*pos], Token::RParen) {
                return Err("Missing closing parenthesis".to_string());
            }
            *pos += 1;
            Ok(val)
        }
        _ => Err(format!("Unexpected token at position {pos}")),
    }
}

/// Analyze text structure using the Juncture graph.
async fn execute_text_analyze(args: &serde_json::Value) -> String {
    let text = match args["text"].as_str() {
        Some(t) => t,
        None => return "Error: missing 'text' parameter".to_string(),
    };

    let graph = match build_graph().compile() {
        Ok(g) => g,
        Err(e) => return format!("Graph error: {e}"),
    };

    let initial_state = TextState {
        input: text.to_string(),
        ..Default::default()
    };

    let config = RunnableConfig {
        recursion_limit: 25,
        ..Default::default()
    };

    match graph.invoke_async(initial_state, &config).await {
        Ok(output) => {
            let s = &output.value;
            format!(
                "Text analysis: {} words, {} chars, {} sentences, \
                 avg word length {:.1}, readability: {}. \
                 Full text: \"{}\"",
                s.word_count, s.char_count, s.sentence_count, s.avg_word_length,
                s.readability, text
            )
        }
        Err(e) => format!("Analysis error: {e}"),
    }
}

/// Look up knowledge from a structured knowledge base.
fn execute_knowledge_lookup(args: &serde_json::Value) -> String {
    let topic = match args["topic"].as_str() {
        Some(t) => t.to_lowercase(),
        None => return "Error: missing 'topic' parameter".to_string(),
    };

    // Structured knowledge base for demonstration
    let entries: &[(&str, &str)] = &[
        ("speed of light", "The speed of light in vacuum is exactly 299,792,458 meters per second (approximately 3.00 x 10^8 m/s). This is a fundamental constant denoted by 'c' in physics."),
        ("rust programming language", "Rust is a systems programming language focused on safety, speed, and concurrency. It achieves memory safety without garbage collection through its ownership system. First released in 2010 by Mozilla, it has been voted the 'most loved language' in Stack Overflow surveys for multiple years."),
        ("population of tokyo", "Tokyo's population is approximately 13.96 million (2023 estimate) for the 23 special wards. The greater Tokyo metropolitan area has about 37.4 million people, making it the world's most populous metropolitan area."),
        ("water boiling point", "Water boils at 100 degrees Celsius (212 degrees Fahrenheit) at standard atmospheric pressure (1 atm / 101.325 kPa). The boiling point decreases at higher altitudes due to lower atmospheric pressure."),
        ("earth circumference", "Earth's circumference at the equator is approximately 40,075 km (24,901 miles). The polar circumference is approximately 40,008 km (24,860 miles) due to Earth's oblate shape."),
        ("pi", "Pi (π) is the ratio of a circle's circumference to its diameter, approximately 3.14159265358979... It is an irrational number with infinite non-repeating decimal digits."),
        ("ai artificial intelligence", "Artificial Intelligence (AI) is the simulation of human intelligence by machines. Modern AI includes machine learning, deep learning, natural language processing, and computer vision. Large Language Models (LLMs) like GPT and Claude represent a significant advancement in natural language understanding and generation."),
        ("wasm webassembly", "WebAssembly (Wasm) is a binary instruction format for a stack-based virtual machine. It enables high-performance applications on web and non-web platforms. WASI (WebAssembly System Interface) extends Wasm to run outside browsers on servers and edge devices."),
        ("juncture framework", "Juncture is a Rust implementation of LangGraph, providing a typed state machine framework for building LLM agent workflows. It features StateGraph, Pregel execution engine, tool calling, checkpointing, and streaming support. It compiles to both native and WASM targets."),
    ];

    for (key, value) in entries {
        if topic.contains(key) || key.contains(&*topic) {
            return value.to_string();
        }
    }

    format!(
        "No specific entry found for '{topic}'. This is a demonstration knowledge base \
         with entries on: speed of light, Rust programming, Tokyo population, water boiling point, \
         Earth circumference, pi, AI, WebAssembly, and Juncture framework."
    )
}

/// Call the LLM with messages and tool definitions.
async fn llm_call(
    messages: &[serde_json::Value],
    tools: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    let api_key = std::env::var("OPENAI_API_KEY")
        .map_err(|_| "OPENAI_API_KEY not configured".to_string())?;
    if api_key.is_empty() {
        return Err("OPENAI_API_KEY is empty".to_string());
    }

    let base_url = std::env::var("OPENAI_BASE_URL")
        .unwrap_or_else(|_| "https://api.openai.com/v1".to_string());
    let model = std::env::var("OPENAI_MODEL").unwrap_or_else(|_| "gpt-4o".to_string());

    let url = format!("{base_url}/chat/completions");

    let body = serde_json::json!({
        "model": model,
        "messages": messages,
        "tools": tools,
        "tool_choice": "auto",
        "max_tokens": 2048
    });

    let request = Request::builder()
        .method(Method::Post)
        .uri(&url)
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {api_key}"))
        .body(serde_json::to_vec(&body).map_err(|e| format!("JSON error: {e}"))?)
        .build();

    let response: Response = spin_sdk::http::send(request)
        .await
        .map_err(|e| format!("HTTP error: {e}"))?;

    let response_body =
        std::str::from_utf8(response.body()).map_err(|e| format!("UTF-8 error: {e}"))?;

    serde_json::from_str(response_body).map_err(|e| format!("JSON parse error: {e}"))
}

/// Run the full agent loop: LLM -> tools -> LLM -> ... until final answer.
async fn run_agent(query: &str) -> Result<serde_json::Value, String> {
    let tools = tool_definitions();

    let mut messages: Vec<serde_json::Value> = vec![
        serde_json::json!({
            "role": "system",
            "content": "You are a capable research assistant running on a WASI edge server. \
                You have access to tools for calculations, text analysis, and knowledge lookup. \
                Always think step by step. Use tools when they can help answer the question. \
                For complex questions, break them into sub-tasks and use multiple tools. \
                Provide clear, detailed explanations in your final answer."
        }),
        serde_json::json!({"role": "user", "content": query}),
    ];

    let mut tool_calls_log: Vec<serde_json::Value> = Vec::new();
    let mut iterations = 0;

    loop {
        if iterations >= MAX_ITERATIONS {
            return Err("Agent exceeded maximum iterations".to_string());
        }
        iterations += 1;

        let llm_response = llm_call(&messages, &tools).await?;

        let choice = llm_response["choices"][0].clone();
        let assistant_msg = choice["message"].clone();

        // Add assistant message to conversation
        messages.push(assistant_msg.clone());

        // Check for tool calls
        let tool_calls = assistant_msg["tool_calls"].as_array();

        match tool_calls {
            Some(calls) if !calls.is_empty() => {
                // Execute each tool call
                for tc in calls {
                    let tc_id = tc["id"].as_str().unwrap_or("unknown").to_string();
                    let fn_name = tc["function"]["name"]
                        .as_str()
                        .unwrap_or("unknown")
                        .to_string();
                    let fn_args_str = tc["function"]["arguments"]
                        .as_str()
                        .unwrap_or("{}");
                    let fn_args: serde_json::Value =
                        serde_json::from_str(fn_args_str).unwrap_or(serde_json::json!({}));

                    // Execute tool
                    let result = execute_tool(&fn_name, &fn_args).await;

                    tool_calls_log.push(serde_json::json!({
                        "iteration": iterations,
                        "tool": fn_name,
                        "arguments": fn_args,
                        "result": result,
                    }));

                    // Add tool result to conversation
                    messages.push(serde_json::json!({
                        "role": "tool",
                        "tool_call_id": tc_id,
                        "content": result,
                    }));
                }
                // Continue loop for next LLM call
            }
            _ => {
                // No tool calls - this is the final answer
                let content = assistant_msg["content"]
                    .as_str()
                    .unwrap_or("No response");

                return Ok(serde_json::json!({
                    "query": query,
                    "answer": content,
                    "iterations": iterations,
                    "tool_calls": tool_calls_log,
                    "model": std::env::var("OPENAI_MODEL").unwrap_or_else(|_| "gpt-4o".to_string()),
                    "runtime": "spin-wasi",
                    "target": "wasm32-wasip1",
                }));
            }
        }
    }
}

/// Spin HTTP component entry point.
#[http_component]
async fn handle_request(req: Request) -> anyhow::Result<impl IntoResponse> {
    let body_bytes = req.body();
    let body_str = std::str::from_utf8(body_bytes).unwrap_or("");

    let request: RequestBody = match serde_json::from_str(body_str) {
        Ok(r) => r,
        Err(_) => {
            let error = serde_json::json!({
                "error": "Invalid JSON. Send {\"message\": \"your question\"}",
                "examples": [
                    "Calculate 15% of 2847, then explain the result",
                    "What is the speed of light? Convert it to km/h",
                    "Analyze this text: 'The quick brown fox jumps over the lazy dog'",
                    "What is the population of Tokyo? Is it larger than all of Canada?"
                ]
            });
            return Ok(Response::builder()
                .status(400)
                .header("content-type", "application/json")
                .body(error.to_string())
                .build());
        }
    };

    match run_agent(&request.message).await {
        Ok(result) => Ok(Response::builder()
            .status(200)
            .header("content-type", "application/json")
            .body(serde_json::to_string_pretty(&result).unwrap_or_default())
            .build()),
        Err(e) => {
            let error = serde_json::json!({ "error": e });
            Ok(Response::builder()
                .status(500)
                .header("content-type", "application/json")
                .body(error.to_string())
                .build())
        }
    }
}

// Rust guideline compliant 2026-05-28
