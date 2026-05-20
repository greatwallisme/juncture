//! Tool execution interceptor for pre/post execution hooks

use async_trait::async_trait;

use juncture_core::state::messages::ToolCall;

use crate::tools::error::ToolError;

/// Tool call interceptor for pre/post execution hooks
///
/// Interceptors allow customizing tool execution behavior without modifying
/// individual tools. Common use cases include:
/// - Logging and observability
/// - Input/output validation
/// - Rate limiting
/// - Access control
/// - Audit trails
///
/// # Example
///
/// ```ignore
/// use async_trait::async_trait;
/// use juncture::tools::{ToolInterceptor, ToolError};
/// use juncture_core::state::messages::ToolCall;
/// use tracing::{info, warn, error};
///
/// struct LoggingInterceptor;
///
/// #[async_trait]
/// impl ToolInterceptor for LoggingInterceptor {
///     async fn pre_execute(
///         &self,
///         tool_call: &ToolCall,
///         state: &serde_json::Value,
///     ) -> Result<(), ToolError> {
///         info!("Executing tool: {}", tool_call.name);
///         Ok(())
///     }
///
///     async fn post_execute(
///         &self,
///         tool_call: &ToolCall,
///         result: &Result<String, ToolError>,
///     ) -> Result<String, ToolError> {
///         match result {
///             Ok(output) => info!("Tool {} succeeded: {}", tool_call.name, output),
///             Err(e) => error!("Tool {} failed: {}", tool_call.name, e),
///         }
///         result.clone()
///     }
/// }
/// ```
#[async_trait]
pub trait ToolInterceptor: Send + Sync + 'static {
    /// Called before tool execution. Return Err to cancel.
    ///
    /// This hook allows intercepting tool calls before execution.
    /// Returning an error will prevent the tool from executing.
    ///
    /// # Errors
    ///
    /// Returns [`ToolError`] if:
    /// - Pre-execution validation fails
    /// - The interceptor chooses to block execution
    async fn pre_execute(
        &self,
        tool_call: &ToolCall,
        state: &serde_json::Value,
    ) -> Result<(), ToolError>;

    /// Called after tool execution. Can modify the result.
    ///
    /// This hook allows post-processing tool results or transforming errors.
    /// The interceptor can:
    /// - Return the original result unchanged
    /// - Modify successful outputs
    /// - Convert errors to successful results (retry logic)
    /// - Add logging or telemetry
    ///
    /// # Errors
    ///
    /// Returns [`ToolError`] if post-processing fails.
    async fn post_execute(
        &self,
        tool_call: &ToolCall,
        result: &Result<String, ToolError>,
    ) -> Result<String, ToolError>;
}

/// No-op interceptor (default)
///
/// Provides a default implementation that does nothing,
/// used when no custom interceptor is specified.
#[derive(Debug)]
pub struct NopToolInterceptor;

#[async_trait]
impl ToolInterceptor for NopToolInterceptor {
    async fn pre_execute(
        &self,
        _tool_call: &ToolCall,
        _state: &serde_json::Value,
    ) -> Result<(), ToolError> {
        Ok(())
    }

    async fn post_execute(
        &self,
        _tool_call: &ToolCall,
        result: &Result<String, ToolError>,
    ) -> Result<String, ToolError> {
        match result {
            Ok(s) => Ok(s.clone()),
            Err(e) => Err(ToolError::execution_failed(e.to_string())),
        }
    }
}

/// Composite interceptor that chains multiple interceptors
///
/// Executes interceptors in order, stopping at the first error in `pre_execute`
/// and running all `post_execute` hooks even if earlier ones fail.
pub struct CompositeInterceptor {
    interceptors: Vec<Box<dyn ToolInterceptor>>,
}

impl std::fmt::Debug for CompositeInterceptor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompositeInterceptor")
            .field("interceptors", &self.interceptors.len())
            .finish()
    }
}

impl CompositeInterceptor {
    /// Create a new composite interceptor
    #[must_use]
    pub fn new(interceptors: Vec<Box<dyn ToolInterceptor>>) -> Self {
        Self { interceptors }
    }

    /// Add an interceptor to the chain
    pub fn add(&mut self, interceptor: Box<dyn ToolInterceptor>) {
        self.interceptors.push(interceptor);
    }
}

#[async_trait]
impl ToolInterceptor for CompositeInterceptor {
    async fn pre_execute(
        &self,
        tool_call: &ToolCall,
        state: &serde_json::Value,
    ) -> Result<(), ToolError> {
        for interceptor in &self.interceptors {
            interceptor.pre_execute(tool_call, state).await?;
        }
        Ok(())
    }

    async fn post_execute(
        &self,
        tool_call: &ToolCall,
        result: &Result<String, ToolError>,
    ) -> Result<String, ToolError> {
        let mut current_result = result.clone();
        for interceptor in &self.interceptors {
            current_result = match interceptor.post_execute(tool_call, &current_result).await {
                Ok(r) => Ok(r),
                Err(e) => Err(e),
            };
        }
        current_result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Arc;

    /// Test interceptor that tracks calls
    struct TrackingInterceptor {
        pre_executed: Arc<std::sync::atomic::AtomicBool>,
        post_executed: Arc<std::sync::atomic::AtomicBool>,
    }

    #[async_trait]
    impl ToolInterceptor for TrackingInterceptor {
        async fn pre_execute(
            &self,
            _tool_call: &ToolCall,
            _state: &serde_json::Value,
        ) -> Result<(), ToolError> {
            self.pre_executed
                .store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }

        async fn post_execute(
            &self,
            _tool_call: &ToolCall,
            result: &Result<String, ToolError>,
        ) -> Result<String, ToolError> {
            self.post_executed
                .store(true, std::sync::atomic::Ordering::SeqCst);
            result.clone()
        }
    }

    /// Test interceptor that blocks execution
    struct BlockingInterceptor;

    #[async_trait]
    impl ToolInterceptor for BlockingInterceptor {
        async fn pre_execute(
            &self,
            _tool_call: &ToolCall,
            _state: &serde_json::Value,
        ) -> Result<(), ToolError> {
            Err(ToolError::Intercepted("Blocked by interceptor".to_string()))
        }

        async fn post_execute(
            &self,
            _tool_call: &ToolCall,
            result: &Result<String, ToolError>,
        ) -> Result<String, ToolError> {
            result.clone()
        }
    }

    #[tokio::test]
    async fn test_nop_interceptor() {
        let interceptor = NopToolInterceptor;
        let tool_call = ToolCall {
            id: "call_1".to_string(),
            name: "test".to_string(),
            arguments: json!({}),
        };

        interceptor
            .pre_execute(&tool_call, &json!(null))
            .await
            .unwrap();

        let post_result = interceptor
            .post_execute(&tool_call, &Ok("success".to_string()))
            .await;
        assert_eq!(post_result.unwrap(), "success");

        interceptor
            .post_execute(&tool_call, &Err(ToolError::Timeout))
            .await
            .unwrap_err();
    }

    #[tokio::test]
    async fn test_tracking_interceptor() {
        let pre_executed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let post_executed = Arc::new(std::sync::atomic::AtomicBool::new(false));

        let interceptor = TrackingInterceptor {
            pre_executed: Arc::clone(&pre_executed),
            post_executed: Arc::clone(&post_executed),
        };

        let tool_call = ToolCall {
            id: "call_1".to_string(),
            name: "test".to_string(),
            arguments: json!({}),
        };

        interceptor
            .pre_execute(&tool_call, &json!(null))
            .await
            .unwrap();
        assert!(pre_executed.load(std::sync::atomic::Ordering::SeqCst));

        interceptor
            .post_execute(&tool_call, &Ok("result".to_string()))
            .await
            .unwrap();
        assert!(post_executed.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[tokio::test]
    async fn test_blocking_interceptor() {
        let interceptor = BlockingInterceptor;
        let tool_call = ToolCall {
            id: "call_1".to_string(),
            name: "test".to_string(),
            arguments: json!({}),
        };

        let result = interceptor.pre_execute(&tool_call, &json!(null)).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ToolError::Intercepted(_)));
    }

    #[tokio::test]
    async fn test_composite_interceptor() {
        let interceptor1 = Box::new(NopToolInterceptor) as Box<dyn ToolInterceptor>;
        let interceptor2 = Box::new(NopToolInterceptor) as Box<dyn ToolInterceptor>;

        let composite = CompositeInterceptor::new(vec![interceptor1, interceptor2]);

        let tool_call = ToolCall {
            id: "call_1".to_string(),
            name: "test".to_string(),
            arguments: json!({}),
        };

        composite
            .pre_execute(&tool_call, &json!(null))
            .await
            .unwrap();

        let post_result = composite
            .post_execute(&tool_call, &Ok("success".to_string()))
            .await;
        post_result.unwrap();
    }

    #[tokio::test]
    async fn test_composite_interceptor_blocking() {
        let interceptor1 = Box::new(NopToolInterceptor) as Box<dyn ToolInterceptor>;
        let interceptor2 = Box::new(BlockingInterceptor) as Box<dyn ToolInterceptor>;

        let composite = CompositeInterceptor::new(vec![interceptor1, interceptor2]);

        let tool_call = ToolCall {
            id: "call_1".to_string(),
            name: "test".to_string(),
            arguments: json!({}),
        };

        let result = composite.pre_execute(&tool_call, &json!(null)).await;
        assert!(result.is_err());
    }
}

// Rust guideline compliant 2026-05-19
