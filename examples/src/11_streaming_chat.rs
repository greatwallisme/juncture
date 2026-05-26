//! Example 11: Streaming Chat with Real LLM
//!
//! Demonstrates token-by-token streaming from a real LLM:
//! - Using `ChatModel::stream` to get a chunk stream
//! - Accumulating chunks into a complete response
//! - Real-time display of tokens as they arrive

#[path = "common.rs"]
mod common;

use common::load_llm;
use futures::StreamExt;
use juncture::Message;
use juncture::llm::ChatModel;
use std::io::Write;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let llm = load_llm().map_err(std::io::Error::other)?;

    let mut stdout = std::io::stdout();
    writeln!(stdout, "Streaming response:\n")?;

    let messages = vec![
        Message::system(
            "You are a creative storyteller. Write a very short story (3-4 sentences).",
        ),
        Message::human("Tell me a story about a robot learning to paint."),
    ];

    let mut stream = llm.stream(&messages, None);
    let mut full_response = String::new();

    while let Some(chunk_result) = stream.next().await {
        let chunk = chunk_result?;
        if !chunk.content.is_empty() {
            write!(stdout, "{}", chunk.content)?;
            stdout.flush()?;
            full_response.push_str(&chunk.content);
        }
    }

    writeln!(stdout, "\n\n--- Streaming complete ---")?;
    writeln!(stdout, "Total characters: {}", full_response.len())?;

    Ok(())
}

// Rust guideline compliant 2026-05-26
