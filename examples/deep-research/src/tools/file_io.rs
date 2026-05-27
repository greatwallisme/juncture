//! File I/O tool for safe file reading within current working directory.

#![allow(
    dead_code,
    reason = "Public API components may not all be used in current binary"
)]

use async_trait::async_trait;
use juncture::tools::{Tool, ToolError};
use serde_json::json;

/// File reading tool with security restrictions.
#[derive(Debug, Default)]
pub struct ReadFile;

impl ReadFile {
    /// Create a new file reading tool.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for ReadFile {
    fn name(&self) -> &'static str {
        "read_file"
    }

    fn description(&self) -> &'static str {
        "Read the contents of a text file. Only files within the current working directory \
         can be accessed for security. \
         Input: {\"path\": \"relative/path/to/file.txt\"}"
    }

    fn schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Relative path to the file to read"
                }
            },
            "required": ["path"]
        })
    }

    async fn invoke(&self, input: serde_json::Value) -> Result<String, ToolError> {
        let path = input["path"]
            .as_str()
            .ok_or_else(|| ToolError::invalid_input("Missing 'path' parameter".to_string()))?;

        // Security check: reject paths with parent directory traversal
        if path.contains("..") {
            return Err(ToolError::execution_failed(
                "Path traversal not allowed: cannot access files outside current directory"
                    .to_string(),
            ));
        }

        // Security check: reject absolute paths
        if path.starts_with('/') || path.starts_with('\\') {
            return Err(ToolError::execution_failed(
                "Absolute paths not allowed: use relative paths from current directory".to_string(),
            ));
        }

        // Read the file
        let content = tokio::fs::read_to_string(path).await.map_err(|e| {
            ToolError::execution_failed(format!("Failed to read file '{path}': {e}"))
        })?;

        Ok(content)
    }
}

// Rust guideline compliant 2026-05-27
