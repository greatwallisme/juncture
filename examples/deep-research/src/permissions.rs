//! Permission configuration for research agent tools.

use juncture::tools::{PermissionConfig, PermissionGuard};

/// Build a permission guard for the research agent.
///
/// # Arguments
///
/// * `require_approval` - Whether to require approval for dangerous operations
///
/// # Examples
///
/// ```
/// use deep_research::permissions::build_permission_guard;
/// use juncture::tools::Permission;
///
/// let guard = build_permission_guard(false);
/// let check = guard.check("web_search");
/// assert!(matches!(check.permission, Permission::Allow));
/// ```
#[must_use]
#[allow(dead_code, reason = "Public API function reserved for future use")]
pub fn build_permission_guard(require_approval: bool) -> PermissionGuard {
    let mut config = PermissionConfig::new();

    // Safe tools - always allowed
    config = config.allow("web_search");
    config = config.allow("calculator");
    config = config.allow("memory_search");

    // File access - controlled by approval flag
    if require_approval {
        config = config.ask_with_reason(
            "read_file",
            "File access requires approval. This tool can read files from the current working directory.",
        );
    } else {
        config = config.allow("read_file");
    }

    PermissionGuard::new(config)
}

// Rust guideline compliant 2026-05-27
