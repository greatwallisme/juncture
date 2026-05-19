//! Types for server metadata and LLM caching policies
//!
//! This module provides types for deployment metadata and LLM response caching configuration.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Server deployment metadata for observability
///
/// Contains optional information about the deployment environment that can be
/// attached to traces and metrics for better observability in multi-instance
/// deployments.
///
/// # Examples
///
/// ```
/// use juncture_tracing::types::ServerInfo;
///
/// let info = ServerInfo {
///     assistant_id: Some("asst_123".to_string()),
///     deployment: Some("production".to_string()),
///     ..Default::default()
/// };
/// ```
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerInfo {
    /// Assistant ID for multi-assistant deployments
    pub assistant_id: Option<String>,

    /// Graph ID identifying the deployed graph
    pub graph_id: Option<String>,

    /// Authenticated user (if applicable)
    pub user: Option<String>,

    /// Deployment environment identifier
    pub deployment: Option<String>,

    /// Service version
    pub version: Option<String>,

    /// Instance ID for multi-instance deployments
    pub instance_id: Option<String>,
}

impl ServerInfo {
    /// Create a new empty `ServerInfo`
    ///
    /// # Examples
    ///
    /// ```
    /// use juncture_tracing::types::ServerInfo;
    ///
    /// let info = ServerInfo::new();
    /// assert!(info.assistant_id.is_none());
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the assistant ID
    ///
    /// # Examples
    ///
    /// ```
    /// use juncture_tracing::types::ServerInfo;
    ///
    /// let info = ServerInfo::new().with_assistant_id("asst_123");
    /// assert_eq!(info.assistant_id, Some("asst_123".to_string()));
    /// ```
    #[must_use]
    pub fn with_assistant_id(mut self, id: impl Into<String>) -> Self {
        self.assistant_id = Some(id.into());
        self
    }

    /// Set the graph ID
    ///
    /// # Examples
    ///
    /// ```
    /// use juncture_tracing::types::ServerInfo;
    ///
    /// let info = ServerInfo::new().with_graph_id("graph_456");
    /// assert_eq!(info.graph_id, Some("graph_456".to_string()));
    /// ```
    #[must_use]
    pub fn with_graph_id(mut self, id: impl Into<String>) -> Self {
        self.graph_id = Some(id.into());
        self
    }

    /// Set the user
    ///
    /// # Examples
    ///
    /// ```
    /// use juncture_tracing::types::ServerInfo;
    ///
    /// let info = ServerInfo::new().with_user("user@example.com");
    /// assert_eq!(info.user, Some("user@example.com".to_string()));
    /// ```
    #[must_use]
    pub fn with_user(mut self, user: impl Into<String>) -> Self {
        self.user = Some(user.into());
        self
    }

    /// Set the deployment environment
    ///
    /// # Examples
    ///
    /// ```
    /// use juncture_tracing::types::ServerInfo;
    ///
    /// let info = ServerInfo::new().with_deployment("production");
    /// assert_eq!(info.deployment, Some("production".to_string()));
    /// ```
    #[must_use]
    pub fn with_deployment(mut self, deployment: impl Into<String>) -> Self {
        self.deployment = Some(deployment.into());
        self
    }

    /// Set the version
    ///
    /// # Examples
    ///
    /// ```
    /// use juncture_tracing::types::ServerInfo;
    ///
    /// let info = ServerInfo::new().with_version("1.0.0");
    /// assert_eq!(info.version, Some("1.0.0".to_string()));
    /// ```
    #[must_use]
    pub fn with_version(mut self, version: impl Into<String>) -> Self {
        self.version = Some(version.into());
        self
    }

    /// Set the instance ID
    ///
    /// # Examples
    ///
    /// ```
    /// use juncture_tracing::types::ServerInfo;
    ///
    /// let info = ServerInfo::new().with_instance_id("pod-abc123");
    /// assert_eq!(info.instance_id, Some("pod-abc123".to_string()));
    /// ```
    #[must_use]
    pub fn with_instance_id(mut self, id: impl Into<String>) -> Self {
        self.instance_id = Some(id.into());
        self
    }
}

/// Cache policy for LLM response caching
///
/// Controls how LLM responses are cached, including the ability to customize
/// the cache key generation function.
///
/// # Examples
///
/// ```
/// use juncture_tracing::types::LlmCachePolicy;
///
/// let policy = LlmCachePolicy::default();
/// assert!(policy.key_func.is_none());
/// ```
#[derive(Default)]
pub struct LlmCachePolicy {
    /// Optional custom cache key function
    ///
    /// If `None`, the default key function using `(model, messages_hash)` is used.
    pub key_func: Option<LlmCacheKeyFn>,
}

/// Type alias for LLM cache key function
type LlmCacheKeyFn = std::sync::Arc<dyn Fn(&LlmCacheKeyInput) -> String + Send + Sync>;

impl fmt::Debug for LlmCachePolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LlmCachePolicy")
            .field(
                "key_func",
                if self.key_func.is_some() {
                    &"Some(custom function)"
                } else {
                    &"None"
                },
            )
            .finish()
    }
}

impl LlmCachePolicy {
    /// Create a new cache policy with the default key function
    ///
    /// # Examples
    ///
    /// ```
    /// use juncture_tracing::types::LlmCachePolicy;
    ///
    /// let policy = LlmCachePolicy::new();
    /// assert!(policy.key_func.is_none());
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set a custom cache key function
    ///
    /// # Parameters
    ///
    /// * `f` - Function that generates cache keys from input
    ///
    /// # Examples
    ///
    /// ```
    /// use juncture_tracing::types::{LlmCachePolicy, LlmCacheKeyInput};
    /// use serde_json::json;
    ///
    /// let policy = LlmCachePolicy::new().with_key_func(|input| {
    ///     format!("{}:{}", input.model, input.messages.len())
    /// });
    ///
    /// let key_input = LlmCacheKeyInput {
    ///     model: "gpt-4".to_string(),
    ///     messages: vec![],
    ///     tools: vec![],
    ///     config: None,
    /// };
    /// assert!(policy.key_func.as_ref().map(|f| f(&key_input)).is_some());
    /// ```
    #[must_use]
    #[allow(
        clippy::type_complexity,
        reason = "Function pointer type is necessary for the cache key API"
    )]
    pub fn with_key_func<F>(mut self, f: F) -> Self
    where
        F: Fn(&LlmCacheKeyInput) -> String + Send + Sync + 'static,
    {
        self.key_func = Some(std::sync::Arc::new(f));
        self
    }
}

/// Input for LLM cache key generation
///
/// Contains the parameters that influence cache key generation for LLM calls.
///
/// # Examples
///
/// ```
/// use juncture_tracing::types::LlmCacheKeyInput;
/// use serde_json::json;
///
/// let input = LlmCacheKeyInput {
///     model: "gpt-4".to_string(),
///     messages: vec![json!({"role": "user", "content": "Hello"})],
///     tools: vec![],
///     config: None,
/// };
/// ```
#[derive(Clone, Debug)]
pub struct LlmCacheKeyInput {
    /// Model name
    pub model: String,

    /// Messages in the conversation
    pub messages: Vec<serde_json::Value>,

    /// Tools available in the call
    pub tools: Vec<serde_json::Value>,

    /// Optional call configuration
    ///
    /// Reserved for future use with `CallOptions` type.
    /// Currently unused but kept for API compatibility.
    pub config: Option<()>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_server_info_default() {
        let info = ServerInfo::default();
        assert!(info.assistant_id.is_none());
        assert!(info.graph_id.is_none());
        assert!(info.user.is_none());
        assert!(info.deployment.is_none());
        assert!(info.version.is_none());
        assert!(info.instance_id.is_none());
    }

    #[test]
    fn test_server_info_builder() {
        let info = ServerInfo::new()
            .with_assistant_id("asst_123")
            .with_graph_id("graph_456")
            .with_user("user@example.com")
            .with_deployment("production")
            .with_version("1.0.0")
            .with_instance_id("pod-abc123");

        assert_eq!(info.assistant_id, Some("asst_123".to_string()));
        assert_eq!(info.graph_id, Some("graph_456".to_string()));
        assert_eq!(info.user, Some("user@example.com".to_string()));
        assert_eq!(info.deployment, Some("production".to_string()));
        assert_eq!(info.version, Some("1.0.0".to_string()));
        assert_eq!(info.instance_id, Some("pod-abc123".to_string()));
    }

    #[test]
    fn test_server_info_serialization() {
        let info = ServerInfo {
            assistant_id: Some("asst_123".to_string()),
            deployment: Some("production".to_string()),
            ..Default::default()
        };

        let json = serde_json::to_string(&info).unwrap();
        let deserialized: ServerInfo = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.assistant_id, info.assistant_id);
        assert_eq!(deserialized.deployment, info.deployment);
    }

    #[test]
    fn test_llm_cache_policy_default() {
        let policy = LlmCachePolicy::default();
        assert!(policy.key_func.is_none());
    }

    #[test]
    fn test_llm_cache_policy_with_custom_func() {
        let policy = LlmCachePolicy::new()
            .with_key_func(|input| format!("custom:{}:{}", input.model, input.messages.len()));

        assert!(policy.key_func.is_some());

        let input = LlmCacheKeyInput {
            model: "gpt-4".to_string(),
            messages: vec![json!({}), json!({})],
            tools: vec![],
            config: None,
        };

        let key = policy.key_func.as_ref().unwrap()(&input);
        assert_eq!(key, "custom:gpt-4:2");
    }

    #[test]
    fn test_llm_cache_policy_debug() {
        let policy_without = LlmCachePolicy::default();
        let debug_str = format!("{policy_without:?}");
        assert!(debug_str.contains("None"));

        let policy_with = LlmCachePolicy::new().with_key_func(|_| "key".to_string());
        let debug_str = format!("{policy_with:?}");
        assert!(debug_str.contains("Some"));
    }

    #[test]
    fn test_llm_cache_key_input() {
        let input = LlmCacheKeyInput {
            model: "claude-3".to_string(),
            messages: vec![json!({"role": "user"})],
            tools: vec![],
            config: None,
        };

        assert_eq!(input.model, "claude-3");
        assert_eq!(input.messages.len(), 1);
        assert!(input.tools.is_empty());
        assert!(input.config.is_none());
    }
}

// Rust guideline compliant 2026-05-19
