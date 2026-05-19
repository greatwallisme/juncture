//! Span name and attribute constants for Juncture tracing
//!
//! This module provides standardized constants for span names and attributes
//! following OpenTelemetry semantic conventions and Juncture-specific naming.

/// Span name constants following the `juncture.{component}.{action}` convention
pub mod names {
    /// Graph invocation span name
    pub const GRAPH_INVOKE: &str = "juncture.graph.invoke";

    /// Graph completion span name
    pub const GRAPH_COMPLETE: &str = "juncture.graph.complete";

    /// Superstep execution span name
    pub const SUPERSTEP: &str = "juncture.superstep";

    /// Node execution span name
    pub const NODE_EXECUTE: &str = "juncture.node.execute";

    /// LLM call span name
    pub const LLM_CALL: &str = "juncture.llm.call";

    /// Tool call span name
    pub const TOOL_CALL: &str = "juncture.tool.call";

    /// Checkpoint write span name
    pub const CHECKPOINT_PUT: &str = "juncture.checkpoint.put";
}

/// Span attribute key constants
pub mod attrs {
    // Graph level attributes

    /// Thread ID attribute
    pub const THREAD_ID: &str = "juncture.thread.id";

    /// Graph name attribute
    pub const GRAPH_NAME: &str = "juncture.graph.name";

    /// Run ID attribute
    pub const RUN_ID: &str = "juncture.run.id";

    /// Recursion limit attribute
    pub const RECURSION_LIMIT: &str = "juncture.recursion.limit";

    // Superstep level attributes

    /// Step number attribute
    pub const STEP: &str = "juncture.step";

    /// Step nodes attribute
    pub const STEP_NODES: &str = "juncture.step.nodes";

    /// Step duration attribute
    pub const STEP_DURATION_MS: &str = "juncture.step.duration_ms";

    // Node level attributes

    /// Node name attribute
    pub const NODE_NAME: &str = "juncture.node.name";

    /// Node duration attribute
    pub const NODE_DURATION_MS: &str = "juncture.node.duration_ms";

    /// Node error attribute
    pub const NODE_ERROR: &str = "juncture.node.error";

    /// Node output type attribute
    pub const NODE_OUTPUT_TYPE: &str = "juncture.node.output_type";

    // LLM level attributes

    /// LLM model attribute
    pub const LLM_MODEL: &str = "juncture.llm.model";

    /// LLM provider attribute
    pub const LLM_PROVIDER: &str = "juncture.llm.provider";

    /// Input tokens attribute
    pub const TOKENS_INPUT: &str = "juncture.tokens.input";

    /// Output tokens attribute
    pub const TOKENS_OUTPUT: &str = "juncture.tokens.output";

    /// Cost in USD attribute
    pub const COST_USD: &str = "juncture.cost.usd";

    /// LLM has tool calls attribute
    pub const LLM_HAS_TOOL_CALLS: &str = "juncture.llm.has_tool_calls";

    /// LLM stop reason attribute
    pub const LLM_STOP_REASON: &str = "juncture.llm.stop_reason";

    // Tool level attributes

    /// Tool name attribute
    pub const TOOL_NAME: &str = "juncture.tool.name";

    /// Tool duration attribute
    pub const TOOL_DURATION_MS: &str = "juncture.tool.duration_ms";

    /// Tool error attribute
    pub const TOOL_ERROR: &str = "juncture.tool.error";

    // Checkpoint level attributes

    /// Checkpoint ID attribute
    pub const CHECKPOINT_ID: &str = "juncture.checkpoint.id";

    /// Checkpoint source attribute
    pub const CHECKPOINT_SOURCE: &str = "juncture.checkpoint.source";

    /// Checkpoint step attribute
    pub const CHECKPOINT_STEP: &str = "juncture.checkpoint.step";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_span_names_format() {
        // All span names should follow juncter.{component}.{action} format
        assert!(names::GRAPH_INVOKE.starts_with("juncture."));
        assert!(names::GRAPH_COMPLETE.starts_with("juncture."));
        assert!(names::SUPERSTEP.starts_with("juncture."));
        assert!(names::NODE_EXECUTE.starts_with("juncture."));
        assert!(names::LLM_CALL.starts_with("juncture."));
        assert!(names::TOOL_CALL.starts_with("juncture."));
        assert!(names::CHECKPOINT_PUT.starts_with("juncture."));
    }

    #[test]
    fn test_attributes_format() {
        // All attributes should follow juncter.* format
        assert!(attrs::THREAD_ID.starts_with("juncture."));
        assert!(attrs::GRAPH_NAME.starts_with("juncture."));
        assert!(attrs::STEP.starts_with("juncture."));
        assert!(attrs::NODE_NAME.starts_with("juncture."));
        assert!(attrs::LLM_MODEL.starts_with("juncture."));
        assert!(attrs::TOOL_NAME.starts_with("juncture."));
        assert!(attrs::CHECKPOINT_ID.starts_with("juncture."));
    }
}

// Rust guideline compliant 2026-05-19
