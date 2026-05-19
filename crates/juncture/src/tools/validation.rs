//! Input validation node for pre-execution checks

use std::sync::Arc;

use juncture_core::state::messages::Message;

use crate::tools::error::ToolError;

/// Input validation node for pre-execution checks
///
/// Provides validation hooks before tool execution, including:
/// - Token limit checking
/// - Custom validation logic
/// - Input sanitization
///
/// # Example
///
/// ```ignore
/// use juncture::tools::ValidationNode;
/// use juncture_core::state::messages::Message;
///
/// let validator = ValidationNode::new()
///     .with_max_tokens(100_000)
///     .with_validator(|messages| {
///         // Custom validation logic
///         if messages.len() > 1000 {
///             return Err(ToolError::validation_failed("Too many messages".to_string()));
///         }
///         Ok(())
///     });
///
/// validator.validate(&messages)?;
/// ```
#[expect(
    clippy::type_complexity,
    reason = "validator type is necessarily complex for flexibility"
)]
pub struct ValidationNode {
    /// Maximum input tokens allowed
    pub max_input_tokens: Option<u64>,

    /// Custom validation function
    pub validator: Option<Arc<dyn Fn(&[Message]) -> Result<(), ToolError> + Send + Sync>>,
}

impl std::fmt::Debug for ValidationNode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ValidationNode")
            .field("max_input_tokens", &self.max_input_tokens)
            .field("validator", &self.validator.is_some())
            .finish()
    }
}

impl Default for ValidationNode {
    fn default() -> Self {
        Self::new()
    }
}

impl ValidationNode {
    /// Create a new validation node with default settings
    #[must_use]
    pub const fn new() -> Self {
        Self {
            max_input_tokens: None,
            validator: None,
        }
    }

    /// Set maximum input tokens
    #[must_use]
    pub const fn with_max_tokens(mut self, max: u64) -> Self {
        self.max_input_tokens = Some(max);
        self
    }

    /// Set a custom validation function
    #[must_use]
    pub fn with_validator(
        mut self,
        f: impl Fn(&[Message]) -> Result<(), ToolError> + Send + Sync + 'static,
    ) -> Self {
        self.validator = Some(Arc::new(f));
        self
    }

    /// Validate the input messages
    ///
    /// # Errors
    ///
    /// Returns [`ToolError`] if validation fails:
    /// - Token limit exceeded
    /// - Custom validation fails
    pub fn validate(&self, messages: &[Message]) -> Result<(), ToolError> {
        // Check token limit
        if let Some(max_tokens) = self.max_input_tokens {
            let total_tokens: u64 = messages
                .iter()
                .map(|m| m.usage.as_ref().map_or(0, |u| u.input_tokens))
                .sum();

            if total_tokens > max_tokens {
                return Err(ToolError::validation_failed(format!(
                    "Token limit exceeded: {total_tokens} > {max_tokens}"
                )));
            }
        }

        // Run custom validator
        if let Some(validator) = &self.validator {
            validator(messages)?;
        }

        Ok(())
    }

    /// Check if validation is enabled
    #[must_use]
    pub const fn is_enabled(&self) -> bool {
        self.max_input_tokens.is_some() || self.validator.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use juncture_core::state::messages::{Content, Role, TokenUsage};

    #[test]
    fn test_validation_node_new() {
        let validator = ValidationNode::new();
        assert!(!validator.is_enabled());
        assert_eq!(validator.max_input_tokens, None);
        assert!(validator.validator.is_none());
    }

    #[test]
    fn test_validation_node_default() {
        let validator = ValidationNode::default();
        assert!(!validator.is_enabled());
    }

    #[test]
    fn test_validation_node_with_max_tokens() {
        let validator = ValidationNode::new().with_max_tokens(1000);
        assert!(validator.is_enabled());
        assert_eq!(validator.max_input_tokens, Some(1000));
    }

    #[test]
    fn test_validation_node_with_validator() {
        let validator = ValidationNode::new().with_validator(|_messages| Ok(()));
        assert!(validator.is_enabled());
        assert!(validator.validator.is_some());
    }

    #[test]
    fn test_validation_validate_empty() {
        let validator = ValidationNode::new();
        let messages: Vec<Message> = vec![];
        validator.validate(&messages).unwrap();
    }

    #[test]
    fn test_validation_validate_within_limit() {
        let validator = ValidationNode::new().with_max_tokens(1000);
        let messages = vec![
            Message {
                id: "msg1".to_string(),
                role: Role::Human,
                content: Content::Text("Hello".to_string()),
                tool_calls: vec![],
                tool_call_id: None,
                name: None,
                usage: Some(TokenUsage {
                    input_tokens: 500,
                    output_tokens: 0,
                    total_tokens: 500,
                }),
            },
            Message {
                id: "msg2".to_string(),
                role: Role::Ai,
                content: Content::Text("Hi".to_string()),
                tool_calls: vec![],
                tool_call_id: None,
                name: None,
                usage: Some(TokenUsage {
                    input_tokens: 300,
                    output_tokens: 0,
                    total_tokens: 300,
                }),
            },
        ];

        validator.validate(&messages).unwrap();
    }

    #[test]
    fn test_validation_validate_exceeds_limit() {
        let validator = ValidationNode::new().with_max_tokens(1000);
        let messages = vec![
            Message {
                id: "msg1".to_string(),
                role: Role::Human,
                content: Content::Text("Hello".to_string()),
                tool_calls: vec![],
                tool_call_id: None,
                name: None,
                usage: Some(TokenUsage {
                    input_tokens: 600,
                    output_tokens: 0,
                    total_tokens: 600,
                }),
            },
            Message {
                id: "msg2".to_string(),
                role: Role::Ai,
                content: Content::Text("Hi".to_string()),
                tool_calls: vec![],
                tool_call_id: None,
                name: None,
                usage: Some(TokenUsage {
                    input_tokens: 500,
                    output_tokens: 0,
                    total_tokens: 500,
                }),
            },
        ];

        let result = validator.validate(&messages);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ToolError::ValidationFailed(_)
        ));
    }

    #[test]
    fn test_validation_custom_validator_pass() {
        let validator = ValidationNode::new().with_validator(|messages| {
            if messages.len() <= 10 {
                Ok(())
            } else {
                Err(ToolError::validation_failed(
                    "Too many messages".to_string(),
                ))
            }
        });

        let messages: Vec<Message> = vec![Message::human("test"); 5];
        validator.validate(&messages).unwrap();
    }

    #[test]
    fn test_validation_custom_validator_fail() {
        let validator = ValidationNode::new().with_validator(|messages| {
            if messages.len() <= 10 {
                Ok(())
            } else {
                Err(ToolError::validation_failed(
                    "Too many messages".to_string(),
                ))
            }
        });

        let messages: Vec<Message> = vec![Message::human("test"); 15];
        let result = validator.validate(&messages);
        assert!(result.is_err());
    }

    #[test]
    fn test_validation_combined_validators() {
        let validator = ValidationNode::new()
            .with_max_tokens(1000)
            .with_validator(|messages| {
                if messages.len() <= 10 {
                    Ok(())
                } else {
                    Err(ToolError::validation_failed(
                        "Too many messages".to_string(),
                    ))
                }
            });

        let messages = vec![Message {
            id: "msg1".to_string(),
            role: Role::Human,
            content: Content::Text("Hello".to_string()),
            tool_calls: vec![],
            tool_call_id: None,
            name: None,
            usage: Some(TokenUsage {
                input_tokens: 500,
                output_tokens: 0,
                total_tokens: 500,
            }),
        }];

        validator.validate(&messages).unwrap();
    }
}

// Rust guideline compliant 2026-05-19
