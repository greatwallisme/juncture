//! Web search tool using Tavily API.

use async_trait::async_trait;
use juncture::tools::{Tool, ToolError};
use serde::Deserialize;
use serde_json::json;
use std::fmt::Write;

/// Web search tool powered by Tavily Search API.
#[derive(Debug)]
pub struct WebSearch {
    api_key: Option<String>,
}

impl WebSearch {
    /// Create a new web search tool.
    ///
    /// # Arguments
    ///
    /// * `api_key` - Optional Tavily API key. If `None`, tool will return error when invoked.
    #[must_use]
    pub const fn new(api_key: Option<String>) -> Self {
        Self { api_key }
    }
}

#[derive(Deserialize)]
struct TavilyResponse {
    results: Vec<SearchResult>,
}

#[derive(Deserialize)]
struct SearchResult {
    title: String,
    url: String,
    content: String,
}

#[async_trait]
impl Tool for WebSearch {
    fn name(&self) -> &'static str {
        "web_search"
    }

    fn description(&self) -> &'static str {
        "Search the web for current information on any topic. \
         Use this tool when you need up-to-date facts, recent news, or current events. \
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

        // Build Tavily API request
        let client = reqwest::Client::new();
        let response = client
            .post("https://api.tavily.com/search")
            .json(&serde_json::json!({
                "api_key": api_key,
                "query": query,
                "max_results": 5,
                "search_depth": "advanced"
            }))
            .send()
            .await
            .map_err(|e| ToolError::execution_failed(format!("HTTP request failed: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Unable to read error response".to_string());
            return Err(ToolError::execution_failed(format!(
                "Tavily API returned error {status}: {error_text}"
            )));
        }

        let tavily_response: TavilyResponse = response
            .json()
            .await
            .map_err(|e| ToolError::execution_failed(format!("Failed to parse response: {e}")))?;

        if tavily_response.results.is_empty() {
            return Ok("No results found for the given query.".to_string());
        }

        // Format results for LLM
        let mut formatted = String::from("Search results:\n\n");
        for (i, result) in tavily_response.results.iter().enumerate() {
            writeln!(
                formatted,
                "{}. {}\n   URL: {}\n   {}\n\n",
                i + 1,
                result.title,
                result.url,
                result.content
            )
            .map_err(|e| ToolError::execution_failed(format!("Failed to format output: {e}")))?;
        }

        Ok(formatted)
    }
}

// Rust guideline compliant 2026-05-27
