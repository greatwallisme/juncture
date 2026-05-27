//! Memory search tool for retrieving past research facts.

#![allow(
    dead_code,
    reason = "Public API components may not all be used in current binary"
)]

use async_trait::async_trait;
use juncture::tools::{Tool, ToolError};
use juncture_core::store::{MemoryStore, SearchQuery, Store};
use serde_json::json;
use std::fmt::Write;
use std::sync::Arc;

/// Memory search tool for retrieving past research facts.
#[derive(Debug)]
pub struct MemorySearch {
    /// Optional fact store for searching.
    store: Option<Arc<MemoryStore>>,
}

impl MemorySearch {
    /// Create a new memory search tool.
    ///
    /// # Arguments
    ///
    /// * `store` - Optional fact store to search
    #[must_use]
    pub const fn new(store: Option<Arc<MemoryStore>>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl Tool for MemorySearch {
    fn name(&self) -> &'static str {
        "memory_search"
    }

    fn description(&self) -> &'static str {
        "Search past research facts by topic or keywords. \
         Use this tool to find relevant information from previous research sessions. \
         Input: {\"query\": \"search query\", \"limit\": 5}"
    }

    fn schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query for finding relevant facts"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of results to return",
                    "default": 5
                }
            },
            "required": ["query"]
        })
    }

    async fn invoke(&self, input: serde_json::Value) -> Result<String, ToolError> {
        let store = self.store.as_ref().ok_or_else(|| {
            ToolError::execution_failed(
                "Memory store not configured. Memory search is disabled.".to_string(),
            )
        })?;

        let query = input["query"]
            .as_str()
            .ok_or_else(|| ToolError::invalid_input("Missing 'query' parameter".to_string()))?;

        let limit = input["limit"].as_u64().unwrap_or(5).min(20) as usize;

        // Search the store
        let search_query = SearchQuery {
            namespace_prefix: "research_facts".to_string(),
            filter: None,
            query: Some(query.to_string()),
            limit,
            offset: 0,
        };

        let result = store
            .search(search_query)
            .await
            .map_err(|e| ToolError::execution_failed(format!("Memory search failed: {e}")))?;

        if result.items.is_empty() {
            return Ok("No relevant facts found in memory for the given query.".to_string());
        }

        // Format results
        let mut formatted = String::from("Relevant facts from memory:\n\n");
        for (i, search_item) in result.items.iter().enumerate() {
            writeln!(
                formatted,
                "{}. Key: {}\n   {}\n\n",
                i + 1,
                search_item.item.key,
                search_item.item.value
            )
            .map_err(|e| ToolError::execution_failed(format!("Failed to format output: {e}")))?;
        }

        Ok(formatted)
    }
}

// Rust guideline compliant 2026-05-27
