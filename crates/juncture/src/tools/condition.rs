//! Conditional routing utilities for tool execution

use juncture_core::edge::END;
use juncture_core::state::messages::Message;

/// Check if the last message has tool calls (for conditional edge routing)
///
/// This function is used in conditional edges to route to the tool node
/// when the AI message contains tool calls, or to END otherwise.
///
/// # Arguments
///
/// * `messages` - The conversation messages
///
/// # Returns
///
/// * `"tools"` - if the last message has tool calls
/// * [`END`] - otherwise
///
/// # Example
///
/// ```ignore
/// use juncture::tools::tools_condition;
/// use juncture_core::{StateGraph, edge};
/// use juncture_core::state::messages::Message;
///
/// let mut graph = StateGraph::new();
/// graph.add_conditional_edges(
///     "agent",
///     tools_condition,
///     [("tools", "tools"), (edge::END, edge::END)],
/// )?;
/// ```
#[must_use]
pub fn tools_condition(messages: &[Message]) -> &'static str {
    messages
        .last()
        .map_or(END, |m| if m.has_tool_calls() { "tools" } else { END })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tools_condition_with_empty_messages() {
        let messages: Vec<Message> = vec![];
        assert_eq!(tools_condition(&messages), END);
    }

    #[test]
    fn test_tools_condition_with_human_message() {
        let messages = vec![Message::human("Hello")];
        assert_eq!(tools_condition(&messages), END);
    }

    #[test]
    fn test_tools_condition_with_ai_message_no_tools() {
        let messages = vec![Message::ai("Hello")];
        assert_eq!(tools_condition(&messages), END);
    }

    #[test]
    fn test_tools_condition_with_ai_message_with_tools() {
        use juncture_core::state::messages::ToolCall;
        use serde_json::json;

        let messages = vec![Message::ai_with_tool_calls(
            "I'll search for that",
            vec![ToolCall {
                id: "call_123".to_string(),
                name: "search".to_string(),
                args: json!({"query": "test"}),
            }],
        )];
        assert_eq!(tools_condition(&messages), "tools");
    }

    #[test]
    fn test_tools_condition_last_message_only() {
        use juncture_core::state::messages::ToolCall;
        use serde_json::json;

        let messages = vec![
            Message::human("Search"),
            Message::ai_with_tool_calls(
                "Searching",
                vec![ToolCall {
                    id: "call_123".to_string(),
                    name: "search".to_string(),
                    args: json!({}),
                }],
            ),
            Message::human("Never mind"),
        ];
        // Last message is human, so should go to END
        assert_eq!(tools_condition(&messages), END);
    }
}

// Rust guideline compliant 2026-05-19
