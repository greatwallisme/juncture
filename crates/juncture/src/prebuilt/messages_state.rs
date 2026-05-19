//! `MessagesState`: a simple state type for message-based agent workflows.
//!
//! This module provides [`MessagesState`], a state type with a single `messages`
//! field that uses the `messages_reducer` merge semantics. This is the standard
//! state type used by prebuilt agents like `create_react_agent`.
//!
//! # Example
//!
//! ```ignore
//! use juncture::prebuilt::MessagesState;
//! use juncture::Message;
//!
//! let state = MessagesState {
//!     messages: vec![Message::human("Hello")],
//! };
//! ```

use juncture_core::state::messages::Message;
use juncture_core::state::messages::messages_reducer;
use juncture_derive::State;
use serde::{Deserialize, Serialize};

/// State type for message-based agent workflows.
///
/// Contains a single `messages` field with reducer-based merge semantics.
/// When updates are applied, new messages are appended or merged according
/// to [`messages_reducer`]: matching IDs update existing messages, remove
/// sentinels delete messages, and new IDs are appended.
///
/// This is the default state type used by [`create_react_agent`](super::create_react_agent).
///
/// # Generated Types
///
/// The `#[derive(State)]` macro generates:
/// - [`MessagesStateUpdate`]: update struct with `messages: Option<Vec<Message>>`
/// - [`MessagesStateFieldVersions`]: field version tracking struct
///
/// # Example
///
/// ```ignore
/// use juncture::prebuilt::MessagesState;
/// use juncture::Message;
///
/// let state = MessagesState {
///     messages: vec![Message::human("What is the weather?")],
/// };
///
/// // Apply an update
/// let update = MessagesStateUpdate {
///     messages: Some(vec![Message::ai("Let me check.")]),
/// };
/// let changed = state.apply(update);
/// ```
#[derive(State, Clone, Debug, Serialize, Deserialize)]
pub struct MessagesState {
    /// Conversation messages with reducer-based merge semantics.
    ///
    /// Uses [`messages_reducer`] to handle updates:
    /// - Matching message IDs: update existing message
    /// - Remove sentinels: delete targeted messages
    /// - New IDs: append to the end
    #[reducer(custom = messages_reducer)]
    pub messages: Vec<Message>,
}

// Rust guideline compliant 2026-05-19
