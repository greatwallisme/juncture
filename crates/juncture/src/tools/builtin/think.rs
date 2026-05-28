//! Strategic reflection tool for research agents.
//!
//! [`ThinkTool`] provides a deliberate pause in the agent loop for
//! strategic reflection. After each search or action, the agent can
//! use this tool to analyze results, assess gaps, and plan next steps.
//!
//! This pattern is used by deepagents' research workflow to improve
//! research quality and prevent wasteful tool calls.
//!
//! # Example
//!
//! ```ignore
//! use juncture::tools::builtin::ThinkTool;
//!
//! let tool = ThinkTool::new();
//! // Agent calls: {"reflection": "Found 3 relevant sources on quantum computing..."}
//! // Tool returns: "Reflection recorded: Found 3 relevant sources..."
//! ```

use async_trait::async_trait;
use serde_json::json;

use crate::tools::error::ToolError;
use crate::tools::trait_::Tool;

/// Strategic reflection tool for research agents.
///
/// When invoked, records the agent's reflection and returns a confirmation
/// message. This creates a deliberate pause in the research workflow for
/// quality decision-making.
///
/// The tool is intentionally simple — it serves as a structured prompt for
/// the agent to think critically about its progress rather than performing
/// any computation.
///
/// # When to use
///
/// - After receiving search results: analyze what was found
/// - Before deciding next steps: assess if more research is needed
/// - When evaluating research gaps: identify missing information
/// - Before concluding: verify completeness
#[derive(Debug, Clone)]
pub struct ThinkTool;

impl ThinkTool {
    /// Create a new `ThinkTool`.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl Default for ThinkTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for ThinkTool {
    #[allow(
        clippy::unnecessary_literal_bound,
        reason = "trait signature requires &self lifetime"
    )]
    fn name(&self) -> &str {
        "think"
    }

    #[allow(
        clippy::unnecessary_literal_bound,
        reason = "trait signature requires &self lifetime"
    )]
    fn description(&self) -> &str {
        "Tool for strategic reflection on research progress and decision-making. \
         Use this tool after each search to analyze results and plan next steps systematically. \
         This creates a deliberate pause in the research workflow for quality decision-making.\n\n\
         When to use:\n\
         - After receiving search results: What key information did I find?\n\
         - Before deciding next steps: Do I have enough to answer comprehensively?\n\
         - When assessing research gaps: What specific information am I still missing?\n\
         - Before concluding research: Can I provide a complete answer now?\n\n\
         Reflection should address:\n\
         1. Analysis of current findings - What concrete information have I gathered?\n\
         2. Gap assessment - What crucial information is still missing?\n\
         3. Quality evaluation - Do I have sufficient evidence/examples for a good answer?\n\
         4. Strategic decision - Should I continue searching or provide my answer?"
    }

    fn schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "reflection": {
                    "type": "string",
                    "description": "Your detailed reflection on research progress, findings, gaps, and next steps"
                }
            },
            "required": ["reflection"]
        })
    }

    async fn invoke(&self, input: serde_json::Value) -> Result<String, ToolError> {
        let reflection = input["reflection"]
            .as_str()
            .ok_or_else(|| ToolError::invalid_input("Missing 'reflection' field".to_string()))?;

        if reflection.trim().is_empty() {
            return Err(ToolError::invalid_input(
                "Reflection cannot be empty".to_string(),
            ));
        }

        Ok(format!("Reflection recorded: {reflection}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_think_tool_name() {
        let tool = ThinkTool::new();
        assert_eq!(tool.name(), "think");
    }

    #[test]
    fn test_think_tool_description() {
        let tool = ThinkTool::new();
        assert!(tool.description().contains("reflection"));
    }

    #[test]
    fn test_think_tool_schema() {
        let tool = ThinkTool::new();
        let schema = tool.schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["reflection"].is_object());
    }

    #[test]
    fn test_think_tool_default() {
        let tool = ThinkTool;
        assert_eq!(tool.name(), "think");
    }

    #[tokio::test]
    async fn test_think_tool_invoke() {
        let tool = ThinkTool::new();
        let input = json!({"reflection": "Found key data on AI safety"});
        let result = tool.invoke(input).await.unwrap();
        assert!(result.contains("Found key data on AI safety"));
        assert!(result.starts_with("Reflection recorded:"));
    }

    #[tokio::test]
    async fn test_think_tool_missing_reflection() {
        let tool = ThinkTool::new();
        let input = json!({});
        let result = tool.invoke(input).await;
        result.unwrap_err();
    }

    #[tokio::test]
    async fn test_think_tool_empty_reflection() {
        let tool = ThinkTool::new();
        let input = json!({"reflection": "  "});
        let result = tool.invoke(input).await;
        result.unwrap_err();
    }
}

// Rust guideline compliant 2026-05-27
