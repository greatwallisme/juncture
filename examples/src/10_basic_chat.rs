//! Example 10: Basic Chat with Real LLM
//!
//! Demonstrates single-turn and multi-turn conversation with a real LLM:
//! - Loading `.env` configuration via `dotenvy`
//! - Building a `ChatOpenAI` client with custom base URL and model
//! - Single-turn invocation with `ChatModel::invoke`
//! - Multi-turn conversation by accumulating `Message` history

#[path = "common.rs"]
mod common;

use common::load_llm;
use juncture::Message;
use juncture::llm::ChatModel;
use std::io::Write;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let llm = load_llm().map_err(std::io::Error::other)?;

    let mut stdout = std::io::stdout();

    // --- Single-turn ---
    writeln!(stdout, "=== Single-turn ===")?;
    let messages = vec![Message::human(
        "What is the capital of France? Reply in one sentence.",
    )];
    let response = llm.invoke(&messages, None).await?;

    if let juncture::llm::Content::Text(text) = &response.content {
        writeln!(stdout, "AI: {text}")?;
    }

    // --- Multi-turn ---
    writeln!(stdout, "\n=== Multi-turn ===")?;
    let mut history: Vec<Message> = vec![
        Message::system("You are a concise travel guide. Answer in one sentence."),
        Message::human("What should I visit in Tokyo?"),
    ];

    let resp1 = llm.invoke(&history, None).await?;
    if let juncture::llm::Content::Text(text) = &resp1.content {
        writeln!(stdout, "AI: {text}")?;
        history.push(resp1);
    }

    history.push(Message::human("And what about food there?"));
    let resp2 = llm.invoke(&history, None).await?;
    if let juncture::llm::Content::Text(text) = &resp2.content {
        writeln!(stdout, "AI: {text}")?;
    }

    Ok(())
}

// Rust guideline compliant 2026-05-26
