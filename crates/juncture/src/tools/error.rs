//! Tool execution errors

use std::fmt::{self, Write};

/// Format validation error messages for display
fn format_validation_errors(errors: &[String]) -> String {
    if errors.is_empty() {
        ": unknown error".to_string()
    } else if errors.len() == 1 {
        format!(": {}", errors[0])
    } else {
        let mut msg = ":\n".to_string();
        for (i, error) in errors.iter().enumerate() {
            writeln!(msg, "  {}. {}", i + 1, error).expect("writing to String cannot fail");
        }
        msg
    }
}

/// Tool execution errors
///
/// Represents errors that can occur during tool execution in the agent workflow.
#[derive(Debug, Clone, thiserror::Error)]
pub enum ToolError {
    /// Invalid tool input
    ///
    /// The input provided to the tool does not match the expected schema
    /// or contains invalid values.
    #[error("invalid input: {0}")]
    InvalidInput(String),

    /// Tool execution failed
    ///
    /// The tool execution encountered an error during runtime.
    #[error("execution failed: {0}")]
    ExecutionFailed(String),

    /// Tool execution timeout
    ///
    /// The tool execution exceeded the configured timeout duration.
    #[error("timeout")]
    Timeout,

    /// Tool not found
    ///
    /// The requested tool name does not exist in the available tools registry.
    #[error("tool not found: {0}")]
    ToolNotFound(String),

    /// Tool call validation failed
    ///
    /// The tool call failed validation checks (e.g., missing required fields).
    /// May contain multiple validation error messages.
    #[error("validation failed{}", format_validation_errors(.0))]
    ValidationFailed(Vec<String>),

    /// Tool execution was intercepted
    ///
    /// An interceptor cancelled the tool execution.
    #[error("execution intercepted: {0}")]
    Intercepted(String),
}

impl ToolError {
    /// Create an invalid input error
    #[must_use]
    pub const fn invalid_input(msg: String) -> Self {
        Self::InvalidInput(msg)
    }

    /// Create an execution failed error
    #[must_use]
    pub const fn execution_failed(msg: String) -> Self {
        Self::ExecutionFailed(msg)
    }

    /// Create a timeout error
    #[must_use]
    pub const fn timeout() -> Self {
        Self::Timeout
    }

    /// Create a tool not found error
    #[must_use]
    pub fn tool_not_found(name: impl fmt::Display) -> Self {
        Self::ToolNotFound(name.to_string())
    }

    /// Create a validation failed error
    #[must_use]
    pub fn validation_failed(msg: impl Into<Vec<String>>) -> Self {
        Self::ValidationFailed(msg.into())
    }

    /// Create an intercepted error
    #[must_use]
    pub const fn intercepted(msg: String) -> Self {
        Self::Intercepted(msg)
    }

    /// Check if this error should prevent agent continuation
    #[must_use]
    pub const fn is_fatal(&self) -> bool {
        matches!(self, Self::Timeout | Self::Intercepted(_))
    }

    /// Check if this error can be retried by the LLM
    #[must_use]
    pub const fn is_retryable(&self) -> bool {
        matches!(self, Self::InvalidInput(_) | Self::ValidationFailed(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_error_display() {
        let err = ToolError::InvalidInput("bad input".to_string());
        assert_eq!(err.to_string(), "invalid input: bad input");

        let err = ToolError::ExecutionFailed("execution error".to_string());
        assert_eq!(err.to_string(), "execution failed: execution error");

        let err = ToolError::Timeout;
        assert_eq!(err.to_string(), "timeout");

        let err = ToolError::ToolNotFound("my_tool".to_string());
        assert_eq!(err.to_string(), "tool not found: my_tool");

        let err = ToolError::ValidationFailed(vec!["validation failed".to_string()]);
        assert_eq!(err.to_string(), "validation failed: validation failed");

        let err = ToolError::Intercepted("blocked".to_string());
        assert_eq!(err.to_string(), "execution intercepted: blocked");
    }

    #[test]
    fn test_tool_error_constructors() {
        let err = ToolError::invalid_input("test".to_string());
        assert!(matches!(err, ToolError::InvalidInput(_)));

        let err = ToolError::execution_failed("test".to_string());
        assert!(matches!(err, ToolError::ExecutionFailed(_)));

        let err = ToolError::timeout();
        assert!(matches!(err, ToolError::Timeout));

        let err = ToolError::tool_not_found("search");
        assert!(matches!(err, ToolError::ToolNotFound(_)));

        let err = ToolError::validation_failed(vec!["test".to_string()]);
        assert!(matches!(err, ToolError::ValidationFailed(_)));

        let err = ToolError::intercepted("test".to_string());
        assert!(matches!(err, ToolError::Intercepted(_)));
    }

    #[test]
    fn test_tool_error_is_fatal() {
        assert!(ToolError::Timeout.is_fatal());
        assert!(ToolError::Intercepted("test".to_string()).is_fatal());
        assert!(!ToolError::InvalidInput("test".to_string()).is_fatal());
        assert!(!ToolError::ExecutionFailed("test".to_string()).is_fatal());
    }

    #[test]
    fn test_tool_error_is_retryable() {
        assert!(ToolError::InvalidInput("test".to_string()).is_retryable());
        assert!(ToolError::ValidationFailed(vec!["test".to_string()]).is_retryable());
        assert!(!ToolError::Timeout.is_retryable());
        assert!(!ToolError::Intercepted("test".to_string()).is_retryable());
    }

    #[test]
    fn test_tool_error_clone() {
        let err = ToolError::InvalidInput("test".to_string());
        let cloned = err.clone();
        assert_eq!(err.to_string(), cloned.to_string());
    }

    #[test]
    fn test_validation_failed_multiple_errors() {
        let errors = vec![
            "Missing required field: 'name'".to_string(),
            "Field 'age' expected type 'integer', got 'string'".to_string(),
        ];
        let err = ToolError::ValidationFailed(errors);
        assert!(matches!(err, ToolError::ValidationFailed(_)));

        let display = err.to_string();
        assert!(display.contains("validation failed"));
        assert!(display.contains("Missing required field: 'name'"));
        assert!(display.contains("Field 'age' expected type 'integer', got 'string'"));
    }

    #[test]
    fn test_validation_failed_empty_vec() {
        let err = ToolError::ValidationFailed(vec![]);
        assert!(err.to_string().contains("validation failed: unknown error"));
    }
}

// Rust guideline compliant 2026-05-19
