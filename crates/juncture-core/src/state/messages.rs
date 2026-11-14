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
///
/// Text parts carry an optional [`CacheControl`](crate::prefix::CacheControl)
/// breakpoint. This is the only
/// abstraction both Anthropic (`cache_control`) and `OpenAI`
/// (`prompt_cache_breakpoint`) can express, because both mark a *content block*
/// rather than a request. Placing the marker on the content also keeps it out of
/// call parameters, where it does not belong (design `12` §2.5).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ContentPart {
    /// Text content, optionally ending a prompt-cache prefix segment
    Text {
        /// Text payload
        text: String,
        /// Explicit cache breakpoint; `None` serializes no marker at all
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cache_control: Option<crate::prefix::CacheControl>,
    },
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

impl ContentPart {
    /// Create a plain text part with no cache breakpoint.
    #[must_use]
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text {
            text: text.into(),
            cache_control: None,
        }
    }

    /// Create a text part that ends a prompt-cache prefix segment.
    #[must_use]
    pub fn text_with_cache_control(
        text: impl Into<String>,
        cache_control: crate::prefix::CacheControl,
    ) -> Self {
        Self::Text {
            text: text.into(),
            cache_control: Some(cache_control),
        }
    }

    /// Text payload of a text part; `None` for non-text parts.
    #[must_use]
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text { text, .. } => Some(text),
            Self::Image(_) | Self::Thinking { .. } => None,
        }
    }

    /// Cache breakpoint attached to this part, if any.
    #[must_use]
    pub const fn cache_control(&self) -> Option<crate::prefix::CacheControl> {
        match self {
            Self::Text { cache_control, .. } => *cache_control,
            Self::Image(_) | Self::Thinking { .. } => None,
        }
    }
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
///
/// Providers disagree on which counters they report:
///
/// - `OpenAI` reports `prompt_tokens` / `completion_tokens` / `total_tokens` and
///   nests `prompt_tokens_details.cached_tokens` (plus `cache_write_tokens` for
///   GPT-5.6 and later).
/// - Anthropic reports `input_tokens` / `output_tokens` and omits `total_tokens`
///   entirely, adding `cache_creation_input_tokens` / `cache_read_input_tokens`.
///
/// Every field beyond `input_tokens` / `output_tokens` therefore carries
/// `#[serde(default)]`: a provider that does not emit a counter must still
/// deserialize. Missing `total_tokens` previously failed the whole response
/// parse and broke every Anthropic `invoke` (design 12 §0.2).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TokenUsage {
    /// Number of input tokens
    pub input_tokens: u64,
    /// Number of output tokens
    pub output_tokens: u64,
    /// Total tokens used
    ///
    /// Anthropic does not return this field; it defaults to 0 rather than
    /// failing deserialization.
    #[serde(default)]
    pub total_tokens: u64,
    /// Anthropic: tokens written into the prompt cache by this request
    #[serde(default, skip_serializing_if = "is_zero")]
    pub cache_creation_input_tokens: u64,
    /// Anthropic: tokens served from the prompt cache by this request
    #[serde(default, skip_serializing_if = "is_zero")]
    pub cache_read_input_tokens: u64,
    /// `OpenAI`: tokens written into the cache (GPT-5.6 and later)
    #[serde(default, skip_serializing_if = "is_zero")]
    pub cache_write_tokens: u64,
    /// `OpenAI`: `prompt_tokens_details.cached_tokens`
    #[serde(default, skip_serializing_if = "is_zero")]
    pub cached_tokens: u64,
}

/// Serde helper: omit zero-valued cache counters from the wire form.
///
/// Takes `&u64` because `serde(skip_serializing_if = ...)` hands predicates a
/// reference; the copy lint is not applicable to that callback shape.
#[allow(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde skip_serializing_if predicates receive &u64"
)]
const fn is_zero(value: &u64) -> bool {
    *value == 0
}

impl TokenUsage {
    /// Fraction of prompt tokens served from cache for this request.
    ///
    /// The numerator is the provider's cache-read counter. The denominator
    /// differs by provider shape and is normalized here:
    ///
    /// - Anthropic: `input_tokens` counts only the *uncached* remainder, so the
    ///   denominator is `input_tokens + cache_read_input_tokens`.
    /// - `OpenAI`: `cached_tokens` is already included in `input_tokens`, so the
    ///   denominator is `input_tokens`.
    ///
    /// Returns 0.0 when no prompt tokens were reported, which is the only
    /// honest answer for "unknown".
    #[must_use]
    #[expect(
        clippy::cast_precision_loss,
        reason = "token counts fit exactly in f64 across the whole practical range"
    )]
    pub fn cache_hit_rate(&self) -> f64 {
        let (hits, denominator) = if self.cached_tokens > 0 {
            (self.cached_tokens, self.input_tokens)
        } else {
            (
                self.cache_read_input_tokens,
                self.input_tokens + self.cache_read_input_tokens,
            )
        };

        if denominator == 0 {
            return 0.0;
        }
        hits as f64 / denominator as f64
    }
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
/// use juncture_core::state::messages::{MessagesState, Message};
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
                    ContentPart::Text { text, .. } => Some(text.as_str()),
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

    #[test]
    fn test_token_usage_deserializes_anthropic_shape() {
        // Anthropic Messages API usage omits `total_tokens` entirely and adds
        // cache counters. Parsing must succeed and expose the cache numbers,
        // otherwise every cache observation downstream is impossible.
        let json = r#"{
            "input_tokens": 2095,
            "output_tokens": 503,
            "cache_creation_input_tokens": 1886,
            "cache_read_input_tokens": 0
        }"#;

        let usage: TokenUsage =
            serde_json::from_str(json).expect("Anthropic usage without total_tokens must parse");

        assert_eq!(usage.input_tokens, 2095);
        assert_eq!(usage.output_tokens, 503);
        assert_eq!(usage.total_tokens, 0);
        assert_eq!(usage.cache_creation_input_tokens, 1886);
        assert_eq!(usage.cache_read_input_tokens, 0);
        assert_eq!(usage.cached_tokens, 0);
        assert_eq!(usage.cache_write_tokens, 0);
    }

    #[test]
    fn test_token_usage_deserializes_openai_shape() {
        let json = r#"{"input_tokens": 1000, "output_tokens": 20,
                       "total_tokens": 1020, "cached_tokens": 800}"#;
        let usage: TokenUsage = serde_json::from_str(json).expect("OpenAI usage must parse");
        assert_eq!(usage.total_tokens, 1020);
        assert_eq!(usage.cached_tokens, 800);
        assert_eq!(usage.cache_read_input_tokens, 0);
    }

    #[test]
    fn test_token_usage_cache_hit_rate_anthropic() {
        // Anthropic: input_tokens is the uncached remainder, so the cache-read
        // counter is added to the denominator.
        let usage = TokenUsage {
            input_tokens: 100,
            output_tokens: 0,
            total_tokens: 0,
            cache_creation_input_tokens: 900,
            cache_read_input_tokens: 900,
            cache_write_tokens: 0,
            cached_tokens: 0,
        };
        assert!((usage.cache_hit_rate() - 0.9).abs() < 1e-9);
    }

    #[test]
    fn test_token_usage_cache_hit_rate_openai() {
        // OpenAI: cached_tokens is already inside input_tokens.
        let usage = TokenUsage {
            input_tokens: 1000,
            output_tokens: 0,
            total_tokens: 0,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
            cache_write_tokens: 0,
            cached_tokens: 800,
        };
        assert!((usage.cache_hit_rate() - 0.8).abs() < 1e-9);
    }

    #[test]
    fn test_token_usage_cache_hit_rate_empty() {
        assert!((TokenUsage::default().cache_hit_rate() - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_token_usage_zero_cache_fields_omitted_from_wire() {
        let usage = TokenUsage {
            input_tokens: 10,
            output_tokens: 5,
            total_tokens: 15,
            ..TokenUsage::default()
        };
        let json = serde_json::to_value(&usage).expect("serialize");
        assert_eq!(json["input_tokens"], 10);
        assert!(json.get("cached_tokens").is_none());
        assert!(json.get("cache_read_input_tokens").is_none());
    }
}

// Rust guideline compliant 2026-05-20
