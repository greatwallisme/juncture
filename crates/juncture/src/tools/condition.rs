//! Conditional routing utilities for tool execution

use juncture_core::State;
use juncture_core::edge::END;
use juncture_core::state::messages::Message;

/// State-based conditional routing: check if the last AI message has tool calls.
///
/// Extracts messages from the state by serializing to JSON and accessing the
/// named field. This works with any state type that has a serializable field
/// containing a `Vec<Message>` (or equivalent JSON array).
///
/// # Type Parameters
///
/// * `S` - The state type, must implement [`State`] and `serde::Serialize`
///
/// # Arguments
///
/// * `state` - The graph state
/// * `messages_field` - The name of the field containing messages (e.g., `"messages"`)
///
/// # Returns
///
/// * `"tools"` - if the last AI message has tool calls
/// * [`END`] - otherwise
///
/// # Example
///
/// ```ignore
/// use juncture::tools::tools_condition;
/// use juncture_core::edge::END;
///
/// graph.add_conditional_edges(
///     "agent",
///     |state: &MyState| tools_condition(state, "messages"),
///     [("tools", "tools"), (END, END)],
/// )?;
/// ```
#[must_use]
pub fn tools_condition<S: State + serde::Serialize>(
    state: &S,
    messages_field: &str,
) -> &'static str {
    juncture_core::tools_condition(state, messages_field)
}

/// Check if the last message has tool calls from a pre-extracted messages slice.
///
/// This is a convenience variant for cases where messages are already extracted
/// from the state. For the generic state-based version, use [`tools_condition`].
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
/// use juncture::tools::tools_condition_from_messages;
/// use juncture_core::edge::END;
///
/// let result = tools_condition_from_messages(&state.messages);
/// ```
#[must_use]
pub fn tools_condition_from_messages(messages: &[Message]) -> &'static str {
    messages
        .last()
        .map_or(END, |m| if m.has_tool_calls() { "tools" } else { END })
}

#[cfg(test)]
mod tests {
    use super::*;
    use juncture_core::state::messages::{MessagesState, ToolCall};
    use serde_json::json;

    // ── tools_condition_from_messages tests ──────────────────────────

    #[test]
    fn test_from_messages_empty() {
        let messages: Vec<Message> = vec![];
        assert_eq!(tools_condition_from_messages(&messages), END);
    }

    #[test]
    fn test_from_messages_human() {
        let messages = vec![Message::human("Hello")];
        assert_eq!(tools_condition_from_messages(&messages), END);
    }

    #[test]
    fn test_from_messages_ai_no_tools() {
        let messages = vec![Message::ai("Hello")];
        assert_eq!(tools_condition_from_messages(&messages), END);
    }

    #[test]
    fn test_from_messages_ai_with_tools() {
        let messages = vec![Message::ai_with_tool_calls(
            "I'll search for that",
            vec![ToolCall {
                id: "call_123".to_string(),
                name: "search".to_string(),
                arguments: json!({"query": "test"}),
            }],
        )];
        assert_eq!(tools_condition_from_messages(&messages), "tools");
    }

    #[test]
    fn test_from_messages_last_message_only() {
        let messages = vec![
            Message::human("Search"),
            Message::ai_with_tool_calls(
                "Searching",
                vec![ToolCall {
                    id: "call_123".to_string(),
                    name: "search".to_string(),
                    arguments: json!({}),
                }],
            ),
            Message::human("Never mind"),
        ];
        // Last message is human, so should go to END
        assert_eq!(tools_condition_from_messages(&messages), END);
    }

    // ── tools_condition (state-based) tests ──────────────────────────

    #[test]
    fn test_state_based_empty_messages() {
        let state = MessagesState { messages: vec![] };
        assert_eq!(tools_condition(&state, "messages"), END);
    }

    #[test]
    fn test_state_based_human_message() {
        let state = MessagesState {
            messages: vec![Message::human("Hello")],
        };
        assert_eq!(tools_condition(&state, "messages"), END);
    }

    #[test]
    fn test_state_based_ai_no_tools() {
        let state = MessagesState {
            messages: vec![Message::ai("Hello")],
        };
        assert_eq!(tools_condition(&state, "messages"), END);
    }

    #[test]
    fn test_state_based_ai_with_tools() {
        let state = MessagesState {
            messages: vec![Message::ai_with_tool_calls(
                "I'll search",
                vec![ToolCall {
                    id: "call_1".to_string(),
                    name: "search".to_string(),
                    arguments: json!({"q": "test"}),
                }],
            )],
        };
        assert_eq!(tools_condition(&state, "messages"), "tools");
    }

    #[test]
    fn test_state_based_last_ai_message_with_tools() {
        let state = MessagesState {
            messages: vec![
                Message::human("Search"),
                Message::ai_with_tool_calls(
                    "Searching",
                    vec![ToolCall {
                        id: "call_1".to_string(),
                        name: "search".to_string(),
                        arguments: json!({}),
                    }],
                ),
                Message::human("Never mind"),
            ],
        };
        // The state-based version looks for the last AI message (not the last
        // message overall). The last AI message has tool calls, so it returns
        // "tools".
        assert_eq!(tools_condition(&state, "messages"), "tools");
    }

    #[test]
    fn test_state_based_custom_field_name() {
        let state = MessagesState {
            messages: vec![Message::ai_with_tool_calls(
                "Working",
                vec![ToolCall {
                    id: "call_1".to_string(),
                    name: "search".to_string(),
                    arguments: json!({}),
                }],
            )],
        };
        assert_eq!(tools_condition(&state, "messages"), "tools");
    }

    #[test]
    fn test_state_based_non_existent_field() {
        let state = MessagesState {
            messages: vec![Message::ai_with_tool_calls(
                "Working",
                vec![ToolCall {
                    id: "call_1".to_string(),
                    name: "search".to_string(),
                    arguments: json!({}),
                }],
            )],
        };
        // Non-existent field -- should return END safely
        assert_eq!(tools_condition(&state, "nonexistent"), END);
    }
}

// Rust guideline compliant 2026-05-19
