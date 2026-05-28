//! Web content fetcher tool for research agents.
//!
//! [`WebFetchTool`] fetches full webpage content and converts it to
//! plain text. This is useful for research agents that need to read
//! full article content beyond search result snippets.
//!
//! Requires the `reqwest` feature to be enabled.
//!
//! # Example
//!
//! ```ignore
//! use juncture::tools::builtin::WebFetchTool;
//!
//! let tool = WebFetchTool::new();
//! // Agent calls: {"url": "https://example.com/article"}
//! // Tool returns: Full article text content
//! ```

use async_trait::async_trait;
use serde_json::json;

use crate::tools::error::ToolError;
use crate::tools::trait_::Tool;

/// Web content fetcher tool for research agents.
///
/// Fetches a webpage and extracts its text content. Strips HTML tags
/// and returns clean text suitable for LLM consumption.
///
/// # Features
///
/// - Fetches full webpage content (not just snippets)
/// - Strips HTML tags for clean text output
/// - Configurable timeout
/// - User-Agent spoofing for compatibility
/// - Size limit protection (max 500KB)
#[derive(Debug, Clone)]
pub struct WebFetchTool {
    /// Request timeout in seconds.
    timeout_secs: u64,
    /// Maximum response size in bytes.
    max_size: usize,
}

impl WebFetchTool {
    /// Create a new `WebFetchTool` with default settings.
    ///
    /// Default timeout: 15 seconds. Default max size: 500KB.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            timeout_secs: 15,
            max_size: 500 * 1024,
        }
    }

    /// Set the request timeout.
    #[must_use]
    pub const fn with_timeout(mut self, secs: u64) -> Self {
        self.timeout_secs = secs;
        self
    }

    /// Set the maximum response size in bytes.
    #[must_use]
    pub const fn with_max_size(mut self, bytes: usize) -> Self {
        self.max_size = bytes;
        self
    }
}

impl Default for WebFetchTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for WebFetchTool {
    #[allow(
        clippy::unnecessary_literal_bound,
        reason = "trait signature requires &self lifetime"
    )]
    fn name(&self) -> &str {
        "web_fetch"
    }

    #[allow(
        clippy::unnecessary_literal_bound,
        reason = "trait signature requires &self lifetime"
    )]
    fn description(&self) -> &str {
        "Fetch the full text content of a webpage. \
         Use this tool to read the complete content of articles, documentation, \
         or other web pages when search snippets are not sufficient.\n\n\
         When to use:\n\
         - When search results reference an article you need to read fully\n\
         - When you need specific details from a webpage\n\
         - When the search snippet is too brief to answer the question\n\n\
         When NOT to use:\n\
         - For simple facts that search snippets already cover\n\
         - For sites that require authentication\n\
         - For dynamic/JavaScript-heavy sites that may not render well"
    }

    fn schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "The URL of the webpage to fetch"
                }
            },
            "required": ["url"]
        })
    }

    async fn invoke(&self, input: serde_json::Value) -> Result<String, ToolError> {
        let url = input["url"]
            .as_str()
            .ok_or_else(|| ToolError::invalid_input("Missing 'url' field".to_string()))?;

        if url.trim().is_empty() {
            return Err(ToolError::invalid_input("URL cannot be empty".to_string()));
        }

        // Validate URL scheme
        if !url.starts_with("http://") && !url.starts_with("https://") {
            return Err(ToolError::invalid_input(format!(
                "URL must start with http:// or https://, got: {url}"
            )));
        }

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(self.timeout_secs))
            .build()
            .map_err(|e| {
                ToolError::execution_failed(format!("Failed to create HTTP client: {e}"))
            })?;

        let response = client
            .get(url)
            .header("User-Agent", "Mozilla/5.0 (compatible; JunctureBot/1.0)")
            .send()
            .await
            .map_err(|e| ToolError::execution_failed(format!("Failed to fetch URL: {e}")))?;

        if !response.status().is_success() {
            return Err(ToolError::execution_failed(format!(
                "HTTP error: {} for URL: {url}",
                response.status()
            )));
        }

        let body = response.text().await.map_err(|e| {
            ToolError::execution_failed(format!("Failed to read response body: {e}"))
        })?;

        if body.len() > self.max_size {
            return Err(ToolError::execution_failed(format!(
                "Response too large: {} bytes (max {} bytes)",
                body.len(),
                self.max_size
            )));
        }

        let text = strip_html_tags(&body);

        if text.trim().is_empty() {
            return Err(ToolError::execution_failed(
                "No text content found on the page".to_string(),
            ));
        }

        Ok(text)
    }
}

/// Strip HTML tags from content and normalize whitespace.
///
/// This is a simple tag stripper — not a full HTML parser. It removes
/// all HTML tags and collapses multiple whitespace into single spaces.
fn strip_html_tags(html: &str) -> String {
    let mut result = String::with_capacity(html.len());
    let mut in_tag = false;

    for c in html.chars() {
        match c {
            '<' => {
                in_tag = true;
            }
            '>' => {
                in_tag = false;
                // Add space after closing tag to separate adjacent content
                result.push(' ');
            }
            _ if in_tag => {}
            _ => {
                result.push(c);
            }
        }
    }

    // Collapse whitespace
    let mut normalized = String::with_capacity(result.len());
    let mut prev_was_space = false;
    for c in result.chars() {
        if c.is_whitespace() {
            if !prev_was_space {
                normalized.push(' ');
                prev_was_space = true;
            }
        } else {
            normalized.push(c);
            prev_was_space = false;
        }
    }

    normalized.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_web_fetch_tool_name() {
        let tool = WebFetchTool::new();
        assert_eq!(tool.name(), "web_fetch");
    }

    #[test]
    fn test_web_fetch_tool_description() {
        let tool = WebFetchTool::new();
        assert!(tool.description().contains("Fetch"));
    }

    #[test]
    fn test_web_fetch_tool_schema() {
        let tool = WebFetchTool::new();
        let schema = tool.schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["url"].is_object());
    }

    #[test]
    fn test_web_fetch_tool_default() {
        let tool = WebFetchTool::default();
        assert_eq!(tool.name(), "web_fetch");
    }

    #[test]
    fn test_web_fetch_tool_builder() {
        let tool = WebFetchTool::new()
            .with_timeout(30)
            .with_max_size(1024 * 1024);
        assert_eq!(tool.timeout_secs, 30);
        assert_eq!(tool.max_size, 1024 * 1024);
    }

    #[tokio::test]
    async fn test_web_fetch_missing_url() {
        let tool = WebFetchTool::new();
        let input = json!({});
        let result = tool.invoke(input).await;
        result.unwrap_err();
    }

    #[tokio::test]
    async fn test_web_fetch_empty_url() {
        let tool = WebFetchTool::new();
        let input = json!({"url": "  "});
        let result = tool.invoke(input).await;
        result.unwrap_err();
    }

    #[tokio::test]
    async fn test_web_fetch_invalid_scheme() {
        let tool = WebFetchTool::new();
        let input = json!({"url": "ftp://example.com"});
        let result = tool.invoke(input).await;
        result.unwrap_err();
    }

    #[test]
    fn test_strip_html_tags() {
        let html = "<html><body><h1>Hello</h1><p>World</p></body></html>";
        let text = strip_html_tags(html);
        assert_eq!(text, "Hello World");
    }

    #[test]
    fn test_strip_html_tags_whitespace() {
        let html = "<p>Hello   \n  World</p>";
        let text = strip_html_tags(html);
        assert_eq!(text, "Hello World");
    }

    #[test]
    fn test_strip_html_tags_nested() {
        let html = "<div><span><b>Bold</b> text</span></div>";
        let text = strip_html_tags(html);
        assert_eq!(text, "Bold text");
    }
}

// Rust guideline compliant 2026-05-27
