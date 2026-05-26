//! Example 15: Structured Output Extraction with Real LLM
//!
//! Demonstrates extracting structured JSON from LLM responses:
//! - Defining a target schema for structured data
//! - Using the LLM to extract structured information from natural language
//! - Parsing and validating the extracted JSON
//!
//! Run: `cargo run -p juncture-simple-example --bin 15_structured_output`

#[path = "common.rs"]
mod common;

use common::load_llm;
use juncture::Message;
use juncture::llm::{CallOptions, ChatModel, Content, ToolDefinition};
use serde::Deserialize;
use std::io::Write;

/// Target structure for entity extraction.
#[derive(Debug, Clone, Deserialize)]
struct ExtractedInfo {
    /// Name of the person mentioned.
    name: String,
    /// Their profession or role.
    profession: String,
    /// Key facts extracted.
    facts: Vec<String>,
    /// Overall sentiment.
    sentiment: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let llm = load_llm().map_err(std::io::Error::other)?;
    let mut stdout = std::io::stdout();

    let extraction_tool = ToolDefinition {
        name: "extract_info".to_string(),
        description: "Extract structured information from text about a person. \
                      Returns JSON with fields: name, profession, facts (array of strings), sentiment."
            .to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "name": {"type": "string", "description": "Person's name"},
                "profession": {"type": "string", "description": "Person's profession or role"},
                "facts": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Key facts about the person"
                },
                "sentiment": {
                    "type": "string",
                    "enum": ["positive", "neutral", "negative"],
                    "description": "Overall sentiment of the text"
                }
            },
            "required": ["name", "profession", "facts", "sentiment"]
        }),
    };

    let llm_with_tool = llm.bind_tools(vec![extraction_tool.clone()]);

    let text = "Dr. Sarah Chen, a brilliant marine biologist at Stanford, \
                recently discovered a new species of deep-sea jellyfish. \
                Her groundbreaking research has been published in Nature \
                and earned her widespread acclaim in the scientific community.";

    let messages = vec![
        Message::system(
            "You are an information extraction assistant. \
             When given text about a person, use the extract_info tool \
             to return structured JSON data about them.",
        ),
        Message::human(format!("Extract information from this text:\n\n{text}")),
    ];

    let options = CallOptions {
        tool_choice: Some(juncture::llm::ToolChoice::Required),
        ..CallOptions::default()
    };

    let response = llm_with_tool.invoke(&messages, Some(&options)).await?;

    // Check for tool calls in the response
    if let Some(tool_call) = response.tool_calls.first() {
        writeln!(stdout, "Tool called: {}", tool_call.name)?;
        let pretty = serde_json::to_string_pretty(&tool_call.arguments)?;
        writeln!(stdout, "Extracted JSON:\n{pretty}")?;

        // Parse into our target struct
        match serde_json::from_value::<ExtractedInfo>(tool_call.arguments.clone()) {
            Ok(info) => {
                writeln!(stdout, "\nParsed structure:")?;
                writeln!(stdout, "  Name: {}", info.name)?;
                writeln!(stdout, "  Profession: {}", info.profession)?;
                writeln!(stdout, "  Facts:")?;
                for fact in &info.facts {
                    writeln!(stdout, "    - {fact}")?;
                }
                writeln!(stdout, "  Sentiment: {}", info.sentiment)?;
            }
            Err(e) => {
                writeln!(stdout, "Failed to parse structured output: {e}")?;
            }
        }
    } else if let Content::Text(text) = &response.content {
        writeln!(stdout, "AI (raw text): {text}")?;
        writeln!(stdout, "\nNote: The LLM did not use the extraction tool.")?;
    }

    Ok(())
}

// Rust guideline compliant 2026-05-26
