//! Permission system for controlling tool execution
//!
//! This module provides a flexible permission system that controls which tools
//! can be executed and which require human approval. It integrates with the
//! HITL (human-in-the-loop) system to trigger interrupts for tools requiring
//! approval.
//!
//! # Example
//!
//! ```ignore
//! use juncture::tools::permission::{Permission, PermissionConfig, PermissionGuard};
//!
//! // Create a permission config with some tools requiring approval
//! let config = PermissionConfig::new()
//!     .allow("search")
//!     .ask_with_reason("file_delete", "Deleting files is irreversible")
//!     .block_with_reason("system_shutdown", "System shutdown is not allowed");
//!
//! // Create a guard to check permissions
//! let guard = PermissionGuard::new(config);
//!
//! // Check if a tool is allowed
//! let check = guard.check("file_delete");
//! assert!(!check.is_allowed());
//! assert!(check.requires_approval());
//! ```

use std::collections::HashMap;
use std::sync::Arc;

use crate::tools::ToolDefinition;

/// Permission level for tool execution
///
/// Defines whether a tool can be executed without approval, requires human
/// approval before execution, or is blocked entirely.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Permission {
    /// Tool is always allowed without prompting
    Allow,

    /// Tool requires user approval before execution (triggers interrupt)
    Ask,

    /// Tool is blocked and cannot be executed
    Block,
}

impl Default for Permission {
    fn default() -> Self {
        Self::Allow
    }
}

/// Permission configuration for a specific tool
///
/// Contains the tool name, its permission level, and an optional reason
/// explaining why the permission was set.
#[derive(Clone, Debug)]
pub struct ToolPermission {
    /// Name of the tool this permission applies to
    pub tool_name: String,

    /// Permission level for this tool
    pub permission: Permission,

    /// Optional reason for the permission level
    pub reason: Option<String>,
}

impl ToolPermission {
    /// Creates a new tool permission
    ///
    /// # Arguments
    ///
    /// * `tool_name` - Name of the tool
    /// * `permission` - Permission level for the tool
    pub fn new(tool_name: impl Into<String>, permission: Permission) -> Self {
        Self {
            tool_name: tool_name.into(),
            permission,
            reason: None,
        }
    }

    /// Sets a reason for this permission
    ///
    /// # Arguments
    ///
    /// * `reason` - Explanation for why this permission level was set
    #[must_use]
    pub fn with_reason(mut self, reason: impl Into<String>) -> Self {
        self.reason = Some(reason.into());
        self
    }
}

/// Configuration for tool permissions
///
/// Stores per-tool permission levels with optional reasons, and provides
/// a builder-style API for configuring permissions.
#[derive(Clone, Debug)]
pub struct PermissionConfig {
    /// Map of tool name to permission configuration
    permissions: HashMap<String, ToolPermission>,

    /// Default permission for tools not explicitly configured
    default: Permission,
}

impl PermissionConfig {
    /// Creates a new empty permission configuration
    ///
    /// The configuration starts with no explicit tool permissions and defaults
    /// to [`Permission::Allow`] for unspecified tools.
    #[must_use]
    pub fn new() -> Self {
        Self {
            permissions: HashMap::new(),
            default: Permission::default(),
        }
    }

    /// Sets the default permission level for unspecified tools
    ///
    /// # Arguments
    ///
    /// * `perm` - Default permission level
    ///
    /// # Example
    ///
    /// ```ignore
    /// let config = PermissionConfig::new()
    ///     .with_default(Permission::Ask);
    /// ```
    #[must_use]
    pub const fn with_default(mut self, perm: Permission) -> Self {
        self.default = perm;
        self
    }

    /// Adds a tool that is always allowed
    ///
    /// # Arguments
    ///
    /// * `tool_name` - Name of the tool to allow
    #[must_use]
    pub fn allow(mut self, tool_name: impl Into<String>) -> Self {
        let name = tool_name.into();
        self.permissions
            .insert(name.clone(), ToolPermission::new(name, Permission::Allow));
        self
    }

    /// Adds a tool that requires user approval
    ///
    /// # Arguments
    ///
    /// * `tool_name` - Name of the tool that requires approval
    #[must_use]
    pub fn ask(mut self, tool_name: impl Into<String>) -> Self {
        let name = tool_name.into();
        self.permissions
            .insert(name.clone(), ToolPermission::new(name, Permission::Ask));
        self
    }

    /// Adds a tool that requires user approval with a reason
    ///
    /// # Arguments
    ///
    /// * `tool_name` - Name of the tool that requires approval
    /// * `reason` - Explanation for why approval is required
    #[must_use]
    pub fn ask_with_reason(
        mut self,
        tool_name: impl Into<String>,
        reason: impl Into<String>,
    ) -> Self {
        let name = tool_name.into();
        self.permissions.insert(
            name.clone(),
            ToolPermission::new(name, Permission::Ask).with_reason(reason.into()),
        );
        self
    }

    /// Adds a tool that is blocked
    ///
    /// # Arguments
    ///
    /// * `tool_name` - Name of the tool to block
    #[must_use]
    pub fn block(mut self, tool_name: impl Into<String>) -> Self {
        let name = tool_name.into();
        self.permissions
            .insert(name.clone(), ToolPermission::new(name, Permission::Block));
        self
    }

    /// Adds a tool that is blocked with a reason
    ///
    /// # Arguments
    ///
    /// * `tool_name` - Name of the tool to block
    /// * `reason` - Explanation for why the tool is blocked
    #[must_use]
    pub fn block_with_reason(
        mut self,
        tool_name: impl Into<String>,
        reason: impl Into<String>,
    ) -> Self {
        let name = tool_name.into();
        self.permissions.insert(
            name.clone(),
            ToolPermission::new(name, Permission::Block).with_reason(reason.into()),
        );
        self
    }

    /// Gets the permission level for a specific tool
    ///
    /// Returns the explicitly configured permission if one exists, otherwise
    /// returns the default permission level.
    ///
    /// # Arguments
    ///
    /// * `tool_name` - Name of the tool to check
    ///
    /// # Example
    ///
    /// ```ignore
    /// let config = PermissionConfig::new()
    ///     .allow("search");
    ///
    /// assert_eq!(config.get("search"), &Permission::Allow);
    /// assert_eq!(config.get("unknown"), &Permission::Allow); // default
    /// ```
    #[must_use]
    pub fn get(&self, tool_name: &str) -> &Permission {
        self.permissions
            .get(tool_name)
            .map_or(&self.default, |tp| &tp.permission)
    }

    /// Gets the reason for a tool's permission level
    ///
    /// Returns `None` if the tool uses the default permission or has no
    /// configured reason.
    ///
    /// # Arguments
    ///
    /// * `tool_name` - Name of the tool to check
    #[must_use]
    pub fn get_reason(&self, tool_name: &str) -> Option<&str> {
        self.permissions
            .get(tool_name)
            .and_then(|tp| tp.reason.as_deref())
    }

    /// Checks if a tool is allowed without approval
    ///
    /// # Arguments
    ///
    /// * `tool_name` - Name of the tool to check
    #[must_use]
    pub fn is_allowed(&self, tool_name: &str) -> bool {
        self.get(tool_name) == &Permission::Allow
    }

    /// Checks if a tool is blocked
    ///
    /// # Arguments
    ///
    /// * `tool_name` - Name of the tool to check
    #[must_use]
    pub fn is_blocked(&self, tool_name: &str) -> bool {
        self.get(tool_name) == &Permission::Block
    }

    /// Checks if a tool requires approval
    ///
    /// # Arguments
    ///
    /// * `tool_name` - Name of the tool to check
    #[must_use]
    pub fn requires_approval(&self, tool_name: &str) -> bool {
        self.get(tool_name) == &Permission::Ask
    }
}

impl Default for PermissionConfig {
    fn default() -> Self {
        Self::new()
    }
}

/// Result of checking a tool's permission
///
/// Contains the tool name, its permission level, and an optional reason.
#[derive(Clone, Debug)]
pub struct PermissionCheck {
    /// Name of the tool that was checked
    pub tool_name: String,

    /// Permission level for the tool
    pub permission: Permission,

    /// Optional reason for the permission level
    pub reason: Option<String>,
}

impl PermissionCheck {
    /// Creates a new permission check result
    ///
    /// # Arguments
    ///
    /// * `tool_name` - Name of the checked tool
    /// * `permission` - Permission level for the tool
    /// * `reason` - Optional reason for the permission level
    pub fn new(
        tool_name: impl Into<String>,
        permission: Permission,
        reason: Option<String>,
    ) -> Self {
        Self {
            tool_name: tool_name.into(),
            permission,
            reason,
        }
    }

    /// Checks if this result indicates the tool is allowed
    #[must_use]
    pub fn is_allowed(&self) -> bool {
        self.permission == Permission::Allow
    }

    /// Checks if this result indicates the tool is blocked
    #[must_use]
    pub fn is_blocked(&self) -> bool {
        self.permission == Permission::Block
    }

    /// Checks if this result indicates the tool requires approval
    #[must_use]
    pub fn requires_approval(&self) -> bool {
        self.permission == Permission::Ask
    }
}

/// Guard for enforcing tool permissions
///
/// Wraps a permission configuration and provides methods to check permissions
/// for individual tools or multiple tools at once.
#[derive(Clone, Debug)]
pub struct PermissionGuard {
    /// Permission configuration
    config: Arc<PermissionConfig>,
}

impl PermissionGuard {
    /// Creates a new permission guard
    ///
    /// # Arguments
    ///
    /// * `config` - Permission configuration to use
    #[must_use]
    pub fn new(config: PermissionConfig) -> Self {
        Self {
            config: Arc::new(config),
        }
    }

    /// Checks the permission for a single tool
    ///
    /// # Arguments
    ///
    /// * `tool_name` - Name of the tool to check
    ///
    /// # Example
    ///
    /// ```ignore
    /// let guard = PermissionGuard::new(
    ///     PermissionConfig::new().ask("file_delete")
    /// );
    ///
    /// let check = guard.check("file_delete");
    /// assert!(check.requires_approval());
    /// ```
    #[must_use]
    pub fn check(&self, tool_name: &str) -> PermissionCheck {
        let permission = self.config.get(tool_name).clone();
        let reason = self.config.get_reason(tool_name).map(String::from);

        PermissionCheck::new(tool_name, permission, reason)
    }

    /// Checks permissions for multiple tools
    ///
    /// # Arguments
    ///
    /// * `tools` - Slice of tool definitions to check
    ///
    /// # Returns
    ///
    /// Vector of permission check results in the same order as the input tools
    ///
    /// # Example
    ///
    /// ```ignore
    /// let tools = vec![
    ///     ToolDefinition::new("search", "Search", json!({})),
    ///     ToolDefinition::new("delete", "Delete", json!({})),
    /// ];
    ///
    /// let guard = PermissionGuard::new(
    ///     PermissionConfig::new().block("delete")
    /// );
    ///
    /// let results = guard.check_all(&tools);
    /// assert!(results[0].is_allowed());
    /// assert!(results[1].is_blocked());
    /// ```
    #[must_use]
    pub fn check_all(&self, tools: &[ToolDefinition]) -> Vec<PermissionCheck> {
        tools.iter().map(|tool| self.check(&tool.name)).collect()
    }
}

/// Errors that can occur when checking tool permissions
///
/// These errors are returned when attempting to execute a tool that is
/// blocked or requires approval.
#[derive(Debug, thiserror::Error)]
pub enum PermissionError {
    /// Tool is blocked and cannot be executed
    #[error("tool '{tool}' is blocked: {reason}")]
    Blocked { tool: String, reason: String },

    /// Tool requires user approval before execution
    #[error("tool '{tool}' requires approval: {reason}")]
    RequiresApproval { tool: String, reason: String },
}

impl PermissionError {
    /// Creates a new blocked error
    ///
    /// # Arguments
    ///
    /// * `tool` - Name of the blocked tool
    /// * `reason` - Reason why the tool is blocked
    #[must_use]
    pub fn blocked(tool: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::Blocked {
            tool: tool.into(),
            reason: reason.into(),
        }
    }

    /// Creates a new requires approval error
    ///
    /// # Arguments
    ///
    /// * `tool` - Name of the tool requiring approval
    /// * `reason` - Reason why approval is required
    #[must_use]
    pub fn requires_approval(tool: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::RequiresApproval {
            tool: tool.into(),
            reason: reason.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // 1. test_permission_default_is_allow
    #[test]
    fn test_permission_default_is_allow() {
        let perm = Permission::default();
        assert_eq!(perm, Permission::Allow);
    }

    // 2. test_permission_config_new
    #[test]
    fn test_permission_config_new() {
        let config = PermissionConfig::new();
        assert_eq!(config.get("any_tool"), &Permission::Allow);
        assert!(config.is_allowed("any_tool"));
        assert!(!config.is_blocked("any_tool"));
        assert!(!config.requires_approval("any_tool"));
    }

    // 3. test_permission_config_builder
    #[test]
    fn test_permission_config_builder() {
        let config = PermissionConfig::new()
            .allow("search")
            .ask("file_delete")
            .block("system_shutdown");

        assert!(config.is_allowed("search"));
        assert!(config.requires_approval("file_delete"));
        assert!(config.is_blocked("system_shutdown"));
    }

    // 4. test_permission_config_get_specific
    #[test]
    fn test_permission_config_get_specific() {
        let config = PermissionConfig::new().block("dangerous_tool");

        assert_eq!(config.get("dangerous_tool"), &Permission::Block);
        assert!(config.is_blocked("dangerous_tool"));
    }

    // 5. test_permission_config_get_default
    #[test]
    fn test_permission_config_get_default() {
        let config = PermissionConfig::new();
        assert_eq!(config.get("unconfigured_tool"), &Permission::Allow);

        let config_with_default = PermissionConfig::new().with_default(Permission::Ask);
        assert_eq!(
            config_with_default.get("unconfigured_tool"),
            &Permission::Ask
        );
    }

    // 6. test_permission_config_is_allowed
    #[test]
    fn test_permission_config_is_allowed() {
        let config = PermissionConfig::new()
            .allow("allowed_tool")
            .ask("ask_tool")
            .block("blocked_tool");

        assert!(config.is_allowed("allowed_tool"));
        assert!(config.is_allowed("unconfigured_tool"));
        assert!(!config.is_allowed("ask_tool"));
        assert!(!config.is_allowed("blocked_tool"));
    }

    // 7. test_permission_config_is_blocked
    #[test]
    fn test_permission_config_is_blocked() {
        let config = PermissionConfig::new()
            .allow("allowed_tool")
            .block("blocked_tool");

        assert!(config.is_blocked("blocked_tool"));
        assert!(!config.is_blocked("allowed_tool"));
        assert!(!config.is_blocked("unconfigured_tool"));
    }

    // 8. test_permission_config_requires_approval
    #[test]
    fn test_permission_config_requires_approval() {
        let config = PermissionConfig::new()
            .allow("allowed_tool")
            .ask("ask_tool")
            .block("blocked_tool");

        assert!(config.requires_approval("ask_tool"));
        assert!(!config.requires_approval("allowed_tool"));
        assert!(!config.requires_approval("blocked_tool"));
        assert!(!config.requires_approval("unconfigured_tool"));
    }

    // 9. test_permission_guard_check
    #[test]
    fn test_permission_guard_check() {
        let config = PermissionConfig::new()
            .allow("safe_tool")
            .ask_with_reason("risky_tool", "Potential data loss")
            .block_with_reason("dangerous_tool", "System instability risk");

        let guard = PermissionGuard::new(config);

        let safe_check = guard.check("safe_tool");
        assert!(safe_check.is_allowed());
        assert_eq!(safe_check.tool_name, "safe_tool");
        assert!(safe_check.reason.is_none());

        let risky_check = guard.check("risky_tool");
        assert!(risky_check.requires_approval());
        assert_eq!(risky_check.reason.as_deref(), Some("Potential data loss"));

        let dangerous_check = guard.check("dangerous_tool");
        assert!(dangerous_check.is_blocked());
        assert_eq!(
            dangerous_check.reason.as_deref(),
            Some("System instability risk")
        );
    }

    // 10. test_permission_guard_check_all
    #[test]
    fn test_permission_guard_check_all() {
        let config = PermissionConfig::new()
            .allow("search")
            .ask("delete")
            .block("shutdown");

        let guard = PermissionGuard::new(config);

        let tools = vec![
            ToolDefinition::new("search", "Search tool", json!({"type": "object"})),
            ToolDefinition::new("delete", "Delete tool", json!({"type": "object"})),
            ToolDefinition::new("shutdown", "Shutdown tool", json!({"type": "object"})),
            ToolDefinition::new("unknown", "Unknown tool", json!({"type": "object"})),
        ];

        let results = guard.check_all(&tools);

        assert_eq!(results.len(), 4);
        assert!(results[0].is_allowed());
        assert!(results[1].requires_approval());
        assert!(results[2].is_blocked());
        assert!(results[3].is_allowed()); // uses default
    }

    // 11. test_permission_error_display
    #[test]
    fn test_permission_error_display() {
        let blocked_err = PermissionError::blocked("system_shutdown", "Not allowed");
        assert!(blocked_err.to_string().contains("system_shutdown"));
        assert!(blocked_err.to_string().contains("blocked"));
        assert!(blocked_err.to_string().contains("Not allowed"));

        let approval_err =
            PermissionError::requires_approval("file_delete", "Irreversible operation");
        assert!(approval_err.to_string().contains("file_delete"));
        assert!(approval_err.to_string().contains("requires approval"));
        assert!(approval_err.to_string().contains("Irreversible operation"));
    }

    // 12. test_permission_config_with_reason
    #[test]
    fn test_permission_config_with_reason() {
        let config = PermissionConfig::new()
            .ask_with_reason("risky_tool", "Potential data loss")
            .block_with_reason("dangerous_tool", "System instability risk");

        assert_eq!(config.get_reason("risky_tool"), Some("Potential data loss"));
        assert_eq!(
            config.get_reason("dangerous_tool"),
            Some("System instability risk")
        );
        assert_eq!(config.get_reason("unconfigured_tool"), None);
    }

    // Additional tests for serde support
    #[test]
    fn test_permission_serde_roundtrip() {
        let perm = Permission::Ask;
        let serialized = serde_json::to_string(&perm).expect("serialize failed");
        let deserialized: Permission =
            serde_json::from_str(&serialized).expect("deserialize failed");
        assert_eq!(perm, deserialized);
    }

    #[test]
    fn test_permission_all_variants_serde() {
        let variants = vec![
            (Permission::Allow, "Allow"),
            (Permission::Ask, "Ask"),
            (Permission::Block, "Block"),
        ];

        for (perm, expected_name) in variants {
            let serialized = serde_json::to_string(&perm).expect("serialize failed");
            assert!(serialized.contains(expected_name));
        }
    }
}

// Rust guideline compliant 2026-05-27
