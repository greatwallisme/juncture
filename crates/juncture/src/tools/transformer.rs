//! Tool call transformer for modifying tool call arguments before execution

use juncture_core::state::messages::ToolCall;

use crate::tools::error::ToolError;

/// Transform tool call arguments before execution
///
/// Transformers allow modifying tool calls before they are executed,
/// enabling patterns like:
/// - Argument sanitization
/// - Default value injection
/// - Schema migration
/// - Security filtering
///
/// # Example
///
/// ```ignore
/// use juncture::tools::{ToolCallTransformer, ToolError};
/// use juncture_core::state::messages::ToolCall;
/// use serde_json::json;
///
/// struct DefaultInjector;
///
/// impl ToolCallTransformer for DefaultInjector {
///     fn transform(&self, tool_call: &mut ToolCall) -> Result<(), ToolError> {
///         if tool_call.name == "search" {
///             if let Some(obj) = tool_call.args.as_object_mut() {
///                 if !obj.contains_key("limit") {
///                     obj.insert("limit".to_string(), json!(10));
///                 }
///             }
///         }
///         Ok(())
///     }
/// }
/// ```
pub trait ToolCallTransformer: Send + Sync + 'static {
    /// Transform a tool call before execution
    ///
    /// This method can modify the tool call's arguments or metadata.
    /// Return an error to prevent the tool call from executing.
    ///
    /// # Errors
    ///
    /// Returns [`ToolError`] if transformation fails.
    fn transform(&self, tool_call: &mut ToolCall) -> Result<(), ToolError>;
}

/// `No-op` transformer (default)
///
/// Provides a default implementation that does nothing,
/// used when no custom transformer is specified.
#[derive(Debug)]
pub struct NopToolTransformer;

impl ToolCallTransformer for NopToolTransformer {
    fn transform(&self, _tool_call: &mut ToolCall) -> Result<(), ToolError> {
        Ok(())
    }
}

/// Composite transformer that chains multiple transformers
///
/// Executes transformers in order, stopping at the first error.
pub struct CompositeTransformer {
    transformers: Vec<Box<dyn ToolCallTransformer>>,
}

impl std::fmt::Debug for CompositeTransformer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompositeTransformer")
            .field("transformers", &self.transformers.len())
            .finish()
    }
}

impl CompositeTransformer {
    /// Create a new composite transformer
    #[must_use]
    pub fn new(transformers: Vec<Box<dyn ToolCallTransformer>>) -> Self {
        Self { transformers }
    }

    /// Add a transformer to the chain
    pub fn add(&mut self, transformer: Box<dyn ToolCallTransformer>) {
        self.transformers.push(transformer);
    }
}

impl ToolCallTransformer for CompositeTransformer {
    fn transform(&self, tool_call: &mut ToolCall) -> Result<(), ToolError> {
        for transformer in &self.transformers {
            transformer.transform(tool_call)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Test transformer that adds a default limit
    struct LimitInjector;

    impl ToolCallTransformer for LimitInjector {
        fn transform(&self, tool_call: &mut ToolCall) -> Result<(), ToolError> {
            if tool_call.name == "search"
                && let Some(obj) = tool_call.args.as_object_mut()
                && !obj.contains_key("limit")
            {
                obj.insert("limit".to_string(), json!(10));
            }
            Ok(())
        }
    }

    /// Test transformer that blocks certain tools
    struct BlockingTransformer {
        blocked_tools: Vec<String>,
    }

    impl ToolCallTransformer for BlockingTransformer {
        fn transform(&self, tool_call: &mut ToolCall) -> Result<(), ToolError> {
            if self.blocked_tools.contains(&tool_call.name) {
                return Err(ToolError::Intercepted(format!(
                    "Tool '{}' is blocked",
                    tool_call.name
                )));
            }
            Ok(())
        }
    }

    #[test]
    fn test_nop_transformer() {
        let transformer = NopToolTransformer;
        let mut tool_call = ToolCall {
            id: "call_1".to_string(),
            name: "test".to_string(),
            args: json!({}),
        };

        transformer.transform(&mut tool_call).unwrap();
    }

    #[test]
    fn test_limit_injector() {
        let transformer = LimitInjector;
        let mut tool_call = ToolCall {
            id: "call_1".to_string(),
            name: "search".to_string(),
            args: json!({"query": "test"}),
        };

        transformer.transform(&mut tool_call).unwrap();
        assert_eq!(tool_call.args["limit"], 10);
    }

    #[test]
    fn test_limit_injector_non_search() {
        let transformer = LimitInjector;
        let mut tool_call = ToolCall {
            id: "call_1".to_string(),
            name: "other".to_string(),
            args: json!({"query": "test"}),
        };

        transformer.transform(&mut tool_call).unwrap();
        assert!(!tool_call.args.as_object().unwrap().contains_key("limit"));
    }

    #[test]
    fn test_blocking_transformer() {
        let transformer = BlockingTransformer {
            blocked_tools: vec!["dangerous".to_string()],
        };
        let mut tool_call = ToolCall {
            id: "call_1".to_string(),
            name: "dangerous".to_string(),
            args: json!({}),
        };

        let result = transformer.transform(&mut tool_call);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ToolError::Intercepted(_)));
    }

    #[test]
    fn test_composite_transformer() {
        let transformer1 = Box::new(NopToolTransformer) as Box<dyn ToolCallTransformer>;
        let transformer2 = Box::new(LimitInjector) as Box<dyn ToolCallTransformer>;

        let composite = CompositeTransformer::new(vec![transformer1, transformer2]);

        let mut tool_call = ToolCall {
            id: "call_1".to_string(),
            name: "search".to_string(),
            args: json!({"query": "test"}),
        };

        composite.transform(&mut tool_call).unwrap();
        assert_eq!(tool_call.args["limit"], 10);
    }

    #[test]
    fn test_composite_transformer_add() {
        let mut composite = CompositeTransformer::new(vec![]);

        composite.add(Box::new(NopToolTransformer));
        composite.add(Box::new(LimitInjector));

        assert_eq!(composite.transformers.len(), 2);
    }

    #[test]
    fn test_composite_transformer_blocking() {
        let transformer1 = Box::new(NopToolTransformer) as Box<dyn ToolCallTransformer>;
        let transformer2 = Box::new(BlockingTransformer {
            blocked_tools: vec!["blocked".to_string()],
        }) as Box<dyn ToolCallTransformer>;

        let composite = CompositeTransformer::new(vec![transformer1, transformer2]);

        let mut tool_call = ToolCall {
            id: "call_1".to_string(),
            name: "blocked".to_string(),
            args: json!({}),
        };

        let result = composite.transform(&mut tool_call);
        assert!(result.is_err());
    }
}

// Rust guideline compliant 2026-05-19
