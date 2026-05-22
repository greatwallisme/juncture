//! Tools module for LLM function calling
//!
//! This module provides the Tool trait and related infrastructure for
//! executing function calls from LLM agents, including:
//!
//! - [`Tool`] trait for defining executable tools
//! - [`ToolNode`] for executing tools from AI messages
//! - [`ToolInterceptor`] for pre/post execution hooks
//! - [`ToolCallTransformer`] for argument transformation
//! - [`ToolError`] for error handling
//! - [`tools_condition`] for conditional routing
//!
//! # Example
//!
//! ```ignore
//! use juncture::tools::{Tool, ToolNode};
//! use async_trait::async_trait;
//! use serde_json::json;
//!
//! struct MyTool;
//!
//! #[async_trait]
//! impl Tool for MyTool {
//!     fn name(&self) -> &str {
//!         "my_tool"
//!     }
//!
//!     fn description(&self) -> &str {
//!         "Does something useful"
//!     }
//!
//!     fn schema(&self) -> serde_json::Value {
//!         json!({
//!             "type": "object",
//!             "properties": {
//!                 "input": {"type": "string"}
//!             },
//!             "required": ["input"]
//!         })
//!     }
//!
//!     async fn invoke(&self, input: serde_json::Value) -> Result<String, ToolError> {
//!         Ok("Result".to_string())
//!     }
//! }
//!
//! // Create tool node
//! let tool_node = ToolNode::new(vec![Box::new(MyTool)]);
//!
//! // Execute tools from AI messages
//! let results = tool_node.execute(&messages).await?;
//! ```

mod condition;
mod error;
mod interceptor;
mod node;
mod runtime;
mod trait_;
mod transformer;
mod validation;

pub use condition::{tools_condition, tools_condition_from_messages};
pub use error::ToolError;
pub use interceptor::{CompositeInterceptor, NopToolInterceptor, ToolInterceptor};
pub use node::{ToolExecutionTrace, ToolNode, ToolNodeConfig};
pub use runtime::ToolRuntime;
pub use trait_::{StatefulTool, Tool, ToolDefinition};
pub use transformer::{CompositeTransformer, NopToolTransformer, ToolCallTransformer};
pub use validation::ValidationNode;

// Rust guideline compliant 2026-05-19
