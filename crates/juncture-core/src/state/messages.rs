use serde::{Deserialize, Serialize};

/// Message type for LLM conversations.
///
/// Used in agent workflows with message-based state.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Message {
    /// Unique message identifier
    pub id: String,
    /// Message role (system, human, ai, tool)
    pub role: Role,
    /// Message content (text or multimodal)
    pub content: Content,
    /// Tool calls made by the AI (for AI messages)
    pub tool_calls: Vec<ToolCall>,
    /// Tool call ID this message responds to (for tool messages)
    pub tool_call_id: Option<String>,
    /// Optional name for the message sender
    pub name: Option<String>,
    /// Token usage information from LLM API responses
    pub usage: Option<TokenUsage>,
}

/// Message role
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum Role {
    /// System message
    System,
    /// Human/user message
    Human,
    /// AI/assistant message
    Ai,
    /// Tool result message
    Tool,
}

/// Message content
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Content {
    /// Simple text content
    Text(String),
    /// Multimodal content with multiple parts
    MultiPart(Vec<ContentPart>),
}

/// Content part for multimodal messages
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ContentPart {
    /// Text content
    Text { text: String },
    /// Image data
    Image(ImageData),
    /// Extended thinking content (Anthropic API)
    ///
    /// Contains the model's internal reasoning process without affecting tool calls.
    Thinking {
        text: String,
        signature: Option<String>,
    },
}

/// Image data for multimodal content
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImageData {
    /// Media type (e.g., "image/png", "image/jpeg")
    pub media_type: String,
    /// Image source data
    pub source: ImageSource,
}

/// Image source for multimodal content
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ImageSource {
    /// Base64-encoded image data
    Base64(String),
    /// Image URL
    Url(String),
}

/// Tool call within a message
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolCall {
    /// Unique tool call identifier
    pub id: String,
    /// Tool name
    pub name: String,
    /// Tool arguments as JSON value
    pub arguments: serde_json::Value,
}

/// Token usage information from LLM API responses
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TokenUsage {
    /// Number of input tokens
    pub input_tokens: u64,
    /// Number of output tokens
    pub output_tokens: u64,
    /// Total tokens used
    pub total_tokens: u64,
}

/// Special sentinel: remove all messages
///
/// Used to clear the entire messages list.
pub const REMOVE_ALL_MESSAGES: &str = "__remove_all__";

/// Built-in state for simple chat agents
///
/// Provides a zero-config entry point for simple chat agents with a
/// single `messages` field using the messages reducer semantics.
///
/// # Examples
///
/// ```
/// use juncture_core::state::MessagesState;
///
/// let mut state = MessagesState::default();
/// state.messages.push(Message::human("Hello"));
/// state.messages.push(Message::ai("Hi there!"));
/// ```
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct MessagesState {
    /// Message history using append+merge+delete semantics
    pub messages: Vec<Message>,
}

/// Update type for `MessagesState`
///
/// All fields are optional to support partial updates.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct MessagesStateUpdate {
    /// Optional messages update
    pub messages: Option<Vec<Message>>,
}

impl crate::State for MessagesState {
    type Update = MessagesStateUpdate;
    type FieldVersions = ();

    fn apply(&mut self, update: Self::Update) -> crate::FieldsChanged {
        let mut changed = crate::FieldsChanged(0);

        if let Some(messages) = update.messages {
            messages_reducer(&mut self.messages, messages);
            changed.0 |= 1 << 0;
        }

        changed
    }

    fn reset_ephemeral(&mut self) {
        // No ephemeral fields in MessagesState
    }
}

impl MessagesState {
    /// Apply an update with structured error propagation for reducer violations
    ///
    /// The messages reducer is an append+merge+delete reducer that never
    /// conflicts, so this always succeeds. Provided for trait consistency.
    ///
    /// # Errors
    ///
    /// This method never returns an error, as the messages reducer has no
    /// write-conflict semantics. The `Result` return type is for API
    /// consistency with `State::try_apply()`.
    pub fn try_apply_messages(
        &mut self,
        update: MessagesStateUpdate,
    ) -> Result<crate::FieldsChanged, crate::error::InvalidUpdateError> {
        Ok(crate::State::apply(self, update))
    }
}

/// Messages reducer with append+merge+delete semantics
///
/// Handles message updates, deletions, and appends.
/// - If message ID matches existing message: update it
/// - If message ID starts with "__remove__:": delete that message
/// - If message is `REMOVE_ALL_MESSAGES`: clear all messages
/// - Otherwise: append the message
pub fn messages_reducer(current: &mut Vec<Message>, incoming: Vec<Message>) {
    for msg in incoming {
        if msg.id == REMOVE_ALL_MESSAGES {
            current.clear();
        } else if msg.id.starts_with("__remove__:") {
            let target_id = &msg.id["__remove__:".len()..];
            current.retain(|m| m.id != target_id);
        } else if let Some(existing) = current.iter_mut().find(|m| m.id == msg.id) {
            *existing = msg;
        } else {
            current.push(msg);
        }
    }
}

impl Message {
    /// Create a human message
    pub fn human(content: impl Into<String>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            role: Role::Human,
            content: Content::Text(content.into()),
            tool_calls: vec![],
            tool_call_id: None,
            name: None,
            usage: None,
        }
    }

    /// Create an AI message
    pub fn ai(content: impl Into<String>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            role: Role::Ai,
            content: Content::Text(content.into()),
            tool_calls: vec![],
            tool_call_id: None,
            name: None,
            usage: None,
        }
    }

    /// Create an AI message with tool calls
    pub fn ai_with_tool_calls(content: impl Into<String>, tool_calls: Vec<ToolCall>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            role: Role::Ai,
            content: Content::Text(content.into()),
            tool_calls,
            tool_call_id: None,
            name: None,
            usage: None,
        }
    }

    /// Create a system message
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            role: Role::System,
            content: Content::Text(content.into()),
            tool_calls: vec![],
            tool_call_id: None,
            name: None,
            usage: None,
        }
    }

    /// Create a tool result message
    pub fn tool_result(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            role: Role::Tool,
            content: Content::Text(content.into()),
            tool_calls: vec![],
            tool_call_id: Some(tool_call_id.into()),
            name: None,
            usage: None,
        }
    }

    /// Check if message has tool calls
    #[must_use]
    pub const fn has_tool_calls(&self) -> bool {
        !self.tool_calls.is_empty()
    }

    /// Extract text content from the message
    ///
    /// For `Content::Text`, returns the text directly.
    /// For `Content::MultiPart`, returns the text of the first `ContentPart::Text` found,
    /// or an empty string if no text part exists.
    #[must_use]
    pub fn content_text(&self) -> &str {
        match &self.content {
            Content::Text(s) => s,
            Content::MultiPart(parts) => parts
                .iter()
                .find_map(|p| match p {
                    ContentPart::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .unwrap_or(""),
        }
    }

    /// Create a remove message sentinel
    #[must_use]
    pub fn remove(id: impl Into<String>) -> Self {
        let id = id.into();
        Self {
            id: format!("__remove__:{id}"),
            role: Role::System,
            content: Content::Text(String::new()),
            tool_calls: vec![],
            tool_call_id: None,
            name: None,
            usage: None,
        }
    }

    /// Create a remove-all message sentinel
    ///
    /// This message clears the entire messages list when processed
    /// by the messages reducer. The sentinel has a special ID
    /// (`REMOVE_ALL_MESSAGES`) that triggers the clear operation.
    #[must_use]
    pub fn remove_all() -> Self {
        Self {
            id: REMOVE_ALL_MESSAGES.to_string(),
            role: Role::System,
            content: Content::Text(String::new()),
            tool_calls: vec![],
            tool_call_id: None,
            name: None,
            usage: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::trait_::State;

    #[test]
    fn test_messages_state_default() {
        let state = MessagesState::default();
        assert!(state.messages.is_empty());
    }

    #[test]
    fn test_messages_state_apply() {
        let mut state = MessagesState::default();

        let update = MessagesStateUpdate {
            messages: Some(vec![Message::human("Hello")]),
        };

        let changed = state.apply(update);
        assert_eq!(state.messages.len(), 1);
        assert!(!changed.is_empty());
        assert!(changed.has_field(0));
    }

    #[test]
    fn test_messages_state_apply_merge() {
        let mut state = MessagesState {
            messages: vec![Message::human("Hello")],
        };

        let update = MessagesStateUpdate {
            messages: Some(vec![Message::ai("Hi there!")]),
        };

        state.apply(update);
        assert_eq!(state.messages.len(), 2);
    }

    #[test]
    fn test_messages_state_apply_none() {
        let mut state = MessagesState {
            messages: vec![Message::human("Hello")],
        };

        let update = MessagesStateUpdate { messages: None };

        let changed = state.apply(update);
        assert_eq!(state.messages.len(), 1);
        assert!(changed.is_empty());
    }

    #[test]
    fn test_messages_state_reset_ephemeral() {
        let mut state = MessagesState {
            messages: vec![Message::human("Hello")],
        };

        state.reset_ephemeral();
        // No-op for MessagesState since it has no ephemeral fields
        assert_eq!(state.messages.len(), 1);
    }

    #[test]
    fn test_messages_state_serialization() {
        let state = MessagesState {
            messages: vec![Message::human("Hello")],
        };

        let json = serde_json::to_string(&state).unwrap();
        let deserialized: MessagesState = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.messages.len(), 1);
        assert_eq!(deserialized.messages[0].role, Role::Human);
    }

    #[test]
    fn test_messages_state_update_serialization() {
        let update = MessagesStateUpdate {
            messages: Some(vec![Message::ai("Hi!")]),
        };

        let json = serde_json::to_string(&update).unwrap();
        let deserialized: MessagesStateUpdate = serde_json::from_str(&json).unwrap();

        assert!(deserialized.messages.is_some());
        assert_eq!(deserialized.messages.unwrap().len(), 1);
    }
}

// Rust guideline compliant 2026-05-20
