//! Tool trait and related types for LLM function calling

use async_trait::async_trait;
use std::sync::Arc;

use crate::tools::error::ToolError;

/// Tool definition for LLM function calling
///
/// Contains the metadata needed to describe a tool to an LLM,
/// including its name, description, and parameter schema.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ToolDefinition {
    /// Unique tool identifier
    pub name: String,

    /// Human-readable tool description
    pub description: String,

    /// JSON Schema for tool input parameters
    pub parameters: serde_json::Value,
}

impl ToolDefinition {
    /// Create a new tool definition
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: serde_json::Value,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            parameters,
        }
    }

    /// Convert to `OpenAI` function call format
    #[must_use]
    pub fn to_openai_format(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "function",
            "function": {
                "name": self.name,
                "description": self.description,
                "parameters": self.parameters,
            },
        })
    }

    /// Convert to Anthropic tool format
    #[must_use]
    pub fn to_anthropic_format(&self) -> serde_json::Value {
        serde_json::json!({
            "name": self.name,
            "description": self.description,
            "input_schema": self.parameters,
        })
    }
}

/// Core Tool trait for LLM function calling
///
/// Tools represent executable functions that LLMs can invoke during agent execution.
/// Each tool has a name, description, JSON schema for parameters, and execution logic.
///
/// # Example
///
/// ```ignore
/// use async_trait::async_trait;
/// use juncture::tools::{Tool, ToolError};
/// use serde_json::json;
///
/// struct Calculator;
///
/// #[async_trait]
/// impl Tool for Calculator {
///     fn name(&self) -> &str {
///         "calculator"
///     }
///
///     fn description(&self) -> &str {
///         "Performs basic arithmetic operations"
///     }
///
///     fn schema(&self) -> serde_json::Value {
///         json!({
///             "type": "object",
///             "properties": {
///                 "operation": {
///                     "type": "string",
///                     "enum": ["add", "subtract", "multiply", "divide"],
///                 },
///                 "a": {"type": "number"},
///                 "b": {"type": "number"},
///             },
///             "required": ["operation", "a", "b"],
///         })
///     }
///
///     async fn invoke(&self, input: serde_json::Value) -> Result<String, ToolError> {
///         // Parse and execute the calculation
///         Ok("42".to_string())
///     }
/// }
/// ```
#[async_trait]
pub trait Tool: Send + Sync + 'static {
    /// Tool name (must be unique within a `ToolNode`)
    fn name(&self) -> &str;

    /// Tool description (shown to LLM)
    ///
    /// This should clearly explain what the tool does and when to use it.
    fn description(&self) -> &str;

    /// JSON Schema for tool input
    ///
    /// Should be a valid JSON Schema describing the expected input structure.
    fn schema(&self) -> serde_json::Value;

    /// Get the full tool definition
    ///
    /// Default implementation combines name, description, and schema.
    #[must_use]
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name().to_string(),
            description: self.description().to_string(),
            parameters: self.schema(),
        }
    }

    /// Execute the tool
    ///
    /// # Errors
    ///
    /// Returns [`ToolError`] if:
    /// - Input validation fails
    /// - Tool execution encounters an error
    /// - Execution times out
    async fn invoke(&self, input: serde_json::Value) -> Result<String, ToolError>;
}

/// Blanket implementation for Arc-wrapped tools
#[async_trait]
impl<T: Tool + ?Sized> Tool for Arc<T> {
    fn name(&self) -> &str {
        T::name(self)
    }

    fn description(&self) -> &str {
        T::description(self)
    }

    fn schema(&self) -> serde_json::Value {
        T::schema(self)
    }

    async fn invoke(&self, input: serde_json::Value) -> Result<String, ToolError> {
        T::invoke(self, input).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Test tool for unit tests
    struct TestTool;

    #[async_trait]
    impl Tool for TestTool {
        fn name(&self) -> &'static str {
            "test_tool"
        }

        fn description(&self) -> &'static str {
            "A test tool"
        }

        fn schema(&self) -> serde_json::Value {
            json!({
                "type": "object",
                "properties": {
                    "input": {"type": "string"}
                }
            })
        }

        async fn invoke(&self, input: serde_json::Value) -> Result<String, ToolError> {
            Ok(format!("Processed: {input}"))
        }
    }

    #[test]
    fn test_tool_definition_new() {
        let def = ToolDefinition::new("my_tool", "My description", json!({"type": "object"}));

        assert_eq!(def.name, "my_tool");
        assert_eq!(def.description, "My description");
        assert_eq!(def.parameters, json!({"type": "object"}));
    }

    #[test]
    fn test_tool_definition_to_openai_format() {
        let def = ToolDefinition::new(
            "search",
            "Search the web",
            json!({"type": "object", "properties": {"query": {"type": "string"}}}),
        );

        let openai = def.to_openai_format();
        assert_eq!(openai["type"], "function");
        assert_eq!(openai["function"]["name"], "search");
        assert_eq!(openai["function"]["description"], "Search the web");
        assert_eq!(openai["function"]["parameters"]["type"], "object");
    }

    #[test]
    fn test_tool_definition_to_anthropic_format() {
        let def = ToolDefinition::new("search", "Search the web", json!({"type": "object"}));

        let anthropic = def.to_anthropic_format();
        assert_eq!(anthropic["name"], "search");
        assert_eq!(anthropic["description"], "Search the web");
        assert_eq!(anthropic["input_schema"]["type"], "object");
    }

    #[tokio::test]
    async fn test_tool_trait() {
        let tool = TestTool;

        assert_eq!(tool.name(), "test_tool");
        assert_eq!(tool.description(), "A test tool");

        let def = tool.definition();
        assert_eq!(def.name, "test_tool");
        assert_eq!(def.description, "A test tool");

        tool.invoke(json!({"test": "value"})).await.unwrap();
    }

    #[tokio::test]
    async fn test_tool_arc_wrapper() {
        let tool = Arc::new(TestTool);

        assert_eq!(tool.name(), "test_tool");
        assert_eq!(tool.description(), "A test tool");

        tool.invoke(json!({})).await.unwrap();
    }
}

// Rust guideline compliant 2026-05-19
