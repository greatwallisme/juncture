//! Example 14: Multi-turn Conversation with Real LLM
//!
//! Demonstrates a multi-turn conversational agent:
//! - Maintaining conversation history across turns
//! - System prompt configuration
//! - Interactive loop that accumulates context
//!
//! Run: `cargo run -p juncture-simple-example --bin 14_multi_turn`

#[path = "common.rs"]
mod common;

use common::load_llm;
use juncture::Message;
use juncture::llm::{ChatModel, Content};
use std::io::Write;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let llm = load_llm().map_err(std::io::Error::other)?;
    let mut stdout = std::io::stdout();

    let mut history: Vec<Message> = vec![Message::system(
        "You are a friendly and knowledgeable cooking assistant. \
             Provide concise, practical cooking advice. \
             If the user asks about something outside cooking, \
             gently steer the conversation back to food and cooking.",
    )];

    let questions = [
        "I have chicken, rice, and broccoli. What can I make?",
        "How long should I cook the chicken?",
        "Any seasoning suggestions?",
    ];

    for question in &questions {
        writeln!(stdout, "User: {question}")?;
        history.push(Message::human((*question).to_string()));

        let response = llm.invoke(&history, None).await?;

        if let Content::Text(text) = &response.content {
            writeln!(stdout, "AI: {text}\n")?;
        }

        history.push(response);
    }

    writeln!(
        stdout,
        "--- Conversation had {} messages ---",
        history.len()
    )?;

    Ok(())
}

// Rust guideline compliant 2026-05-26
