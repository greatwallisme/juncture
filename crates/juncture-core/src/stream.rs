use crate::state::State;
use futures::StreamExt;
use std::collections::HashMap;

/// Stream mode for graph execution
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum StreamMode {
    /// Output complete state after each superstep
    #[default]
    Values,

    /// Output updates from each node
    Updates,

    /// Output LLM token streams
    Messages,

    /// Output custom events from nodes
    Custom,

    /// Output all debug events
    Debug,

    /// Output tool execution lifecycle events
    Tools,

    /// Output checkpoint save events
    Checkpoints,

    /// Output detailed task events
    Tasks,

    /// Combine multiple stream modes
    Multi(Vec<StreamMode>),
}

/// Stream event during graph execution
#[derive(Clone, Debug)]
pub enum StreamEvent<S: State> {
    /// Complete state snapshot
    Values { state: S, step: usize },

    /// Filtered state snapshot (only selected fields as JSON).
    ///
    /// Emitted instead of [`Values`](Self::Values) when
    /// [`StreamConfig::output_keys`] is set. The `data` field contains a JSON
    /// object with only the keys requested by the caller.
    FilteredValues {
        data: serde_json::Value,
        step: usize,
    },

    /// Node update
    Updates {
        node: String,
        update: S::Update,
        step: usize,
    },

    /// Filtered node update (only selected fields as JSON).
    ///
    /// Emitted instead of [`Updates`](Self::Updates) when
    /// [`StreamConfig::output_keys`] is set.
    FilteredUpdates {
        node: String,
        data: serde_json::Value,
        step: usize,
    },

    /// LLM token chunk
    Messages {
        chunk: MessageChunk,
        metadata: MessageStreamMetadata,
    },

    /// Custom event from node
    Custom {
        node: String,
        data: serde_json::Value,
        ns: Vec<String>,
    },

    /// Task started
    TaskStart {
        node: String,
        task_id: String,
        step: usize,
    },

    /// Task completed
    TaskEnd {
        node: String,
        task_id: String,
        step: usize,
        duration_ms: u64,
    },

    /// HITL interrupt
    Interrupt {
        node: String,
        payload: serde_json::Value,
        resumable: bool,
        ns: Vec<String>,
    },

    /// Budget exceeded
    BudgetExceeded {
        reason: crate::pregel::BudgetExceededReason,
        usage: BudgetUsage,
    },

    /// Graph execution completed
    End { output: S },

    /// Debug event
    Debug(DebugEvent),

    /// Tool lifecycle event
    Tools(ToolsEvent),

    /// Checkpoint saved
    CheckpointSaved {
        checkpoint_id: String,
        metadata: crate::checkpoint::CheckpointMetadata,
        step: usize,
    },

    /// Detailed task event
    TaskDetail {
        task_id: String,
        node: String,
        step: usize,
        attempt: usize,
        event: TaskEventType,
    },
}

impl<S: State> StreamEvent<S> {
    /// Return the namespace segment list attached to this event, if any.
    ///
    /// Events originating from subgraphs carry a non-empty `ns` field (the
    /// nesting path). Top-level graph events return an empty slice.
    /// This is used by stream filtering to decide whether to forward or
    /// suppress subgraph events.
    #[must_use]
    #[allow(
        clippy::match_same_arms,
        reason = "each arm is explicit for clarity even when some return the same value"
    )]
    pub fn namespace(&self) -> &[String] {
        match self {
            Self::Custom { ns, .. } => ns,
            Self::Messages { metadata, .. } => &metadata.ns,
            Self::Interrupt { ns, .. } => ns,
            Self::Values { .. }
            | Self::FilteredValues { .. }
            | Self::Updates { .. }
            | Self::FilteredUpdates { .. }
            | Self::TaskStart { .. }
            | Self::TaskEnd { .. }
            | Self::BudgetExceeded { .. }
            | Self::End { .. }
            | Self::Debug(_)
            | Self::Tools(_)
            | Self::CheckpointSaved { .. }
            | Self::TaskDetail { .. } => &[],
        }
    }
}

/// Message chunk for streaming
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct MessageChunk {
    pub content: String,
    pub tool_call_chunks: Vec<ToolCallChunk>,
    pub usage_delta: Option<crate::state::TokenUsage>,
}

/// Tool call chunk
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ToolCallChunk {
    pub id: Option<String>,
    pub name: Option<String>,
    pub args_delta: String,
    pub index: usize,
}

/// Message stream metadata
#[derive(Clone, Debug)]
pub struct MessageStreamMetadata {
    pub node: String,
    pub model: String,
    pub tags: Vec<String>,
    pub ns: Vec<String>,
}

/// Debug event details
#[derive(Clone, Debug)]
pub enum DebugEvent {
    SuperstepStart {
        step: usize,
        nodes: Vec<String>,
    },
    SuperstepEnd {
        step: usize,
        duration_ms: u64,
    },
    CheckpointSaved {
        checkpoint_id: String,
        metadata: crate::checkpoint::CheckpointMetadata,
        step: usize,
    },
    ChannelUpdate {
        channel: String,
        version: u64,
    },
    RouteDecision {
        from: String,
        to: Vec<String>,
        step: usize,
    },
    BudgetStatus {
        usage: BudgetUsage,
    },
}

/// Tool execution event
#[derive(Clone, Debug)]
pub enum ToolsEvent {
    ToolStarted {
        tool_name: String,
        tool_call_id: String,
        node: String,
        input: serde_json::Value,
    },
    ToolOutputDelta {
        tool_call_id: String,
        delta: String,
    },
    ToolFinished {
        tool_call_id: String,
        output: serde_json::Value,
        duration_ms: u64,
    },
    ToolError {
        tool_call_id: String,
        error: String,
    },
}

/// Budget usage information
#[derive(Clone, Debug)]
pub struct BudgetUsage {
    pub tokens_used: u64,
    pub cost_usd: f64,
    pub duration_ms: u64,
    pub steps_completed: usize,
}

/// Task event type
#[derive(Clone, Debug)]
pub enum TaskEventType {
    Started,
    Completed { duration_ms: u64 },
    Failed { error: String },
    Retrying { attempt: usize },
}

/// A part of a stream with namespace and metadata
#[derive(Clone)]
pub struct StreamPart<S: State> {
    pub ns: Vec<String>,
    pub event: &'static str,
    pub data: StreamEvent<S>,
    pub metadata: Option<HashMap<String, serde_json::Value>>,
}

impl<S: State> std::fmt::Debug for StreamPart<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamPart")
            .field("ns", &self.ns)
            .field("event", &self.event)
            .field("data", &"<StreamEvent>")
            .field("metadata", &self.metadata)
            .finish()
    }
}

/// A named channel for streaming events
#[derive(Clone)]
pub struct StreamChannel {
    pub name: String,
    tx: tokio::sync::mpsc::Sender<serde_json::Value>,
}

impl std::fmt::Debug for StreamChannel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamChannel")
            .field("name", &self.name)
            .field("tx", &"<mpsc::Sender>")
            .finish()
    }
}

impl StreamChannel {
    #[must_use]
    pub const fn new(name: String, tx: tokio::sync::mpsc::Sender<serde_json::Value>) -> Self {
        Self { name, tx }
    }

    /// Send data through this channel
    ///
    /// # Errors
    ///
    /// Returns an error if the channel is closed
    pub async fn send(
        &self,
        data: serde_json::Value,
    ) -> Result<(), tokio::sync::mpsc::error::SendError<serde_json::Value>> {
        self.tx.send(data).await
    }
}

/// Transformer for stream data
pub trait StreamTransformer: Send + Sync + 'static {
    fn transform(&self, data: serde_json::Value) -> serde_json::Value;
}

/// Event emitter for streaming
#[derive(Clone)]
pub struct EventEmitter<S: State> {
    pub tx: tokio::sync::mpsc::Sender<StreamEvent<S>>,
    pub mode: StreamMode,
    ns: Vec<String>,
    _phantom: std::marker::PhantomData<S>,
}

impl<S: State> std::fmt::Debug for EventEmitter<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventEmitter")
            .field("tx", &"<mpsc::Sender>")
            .field("mode", &self.mode)
            .field("ns", &self.ns)
            .finish()
    }
}

impl<S: State> EventEmitter<S> {
    #[must_use]
    pub const fn new(tx: tokio::sync::mpsc::Sender<StreamEvent<S>>, mode: StreamMode) -> Self {
        Self {
            tx,
            mode,
            ns: Vec::new(),
            _phantom: std::marker::PhantomData,
        }
    }

    /// Create a child emitter with an additional subgraph namespace segment.
    ///
    /// Used by subgraph execution to namespace streaming events,
    /// allowing consumers to distinguish events from nested subgraphs.
    #[must_use]
    pub fn with_subgraph_ns(&self, ns_segment: String) -> Self {
        let mut new_ns = self.ns.clone();
        new_ns.push(ns_segment);
        Self {
            tx: self.tx.clone(),
            mode: self.mode.clone(),
            ns: new_ns,
            _phantom: std::marker::PhantomData,
        }
    }

    /// Return the current namespace stack.
    #[must_use]
    pub fn ns(&self) -> &[String] {
        &self.ns
    }

    /// Return the stream mode for this emitter.
    #[must_use]
    pub const fn mode(&self) -> &StreamMode {
        &self.mode
    }

    /// Emit an event to the stream.
    ///
    /// Silently drops the event if the receiver has been closed,
    /// matching the design intent that stream consumers may disconnect
    /// at any time without disrupting execution.
    pub async fn emit(&self, event: StreamEvent<S>) {
        let _ = self.tx.send(event).await;
    }

    #[must_use]
    pub fn stream_writer(&self, node: String) -> StreamWriter<S> {
        StreamWriter::new(self.tx.clone(), node, self.mode.clone())
    }

    #[must_use]
    #[allow(clippy::match_same_arms, reason = "each arm is explicit for clarity")]
    pub fn should_emit(&self, event: &StreamEvent<S>) -> bool {
        match (&self.mode, event) {
            (
                StreamMode::Values,
                StreamEvent::Values { .. }
                | StreamEvent::FilteredValues { .. }
                | StreamEvent::End { .. },
            ) => true,
            (
                StreamMode::Updates,
                StreamEvent::Updates { .. }
                | StreamEvent::FilteredUpdates { .. }
                | StreamEvent::End { .. },
            ) => true,
            (StreamMode::Messages, StreamEvent::Messages { .. } | StreamEvent::End { .. }) => {
                // Filter out Messages events with "nostream" tag
                if let StreamEvent::Messages { metadata, .. } = event {
                    !Self::has_nostream_tag(metadata)
                } else {
                    true
                }
            }
            (StreamMode::Custom, StreamEvent::Custom { .. } | StreamEvent::End { .. }) => true,
            (StreamMode::Debug, _) => true, // Debug mode receives all events including End
            (StreamMode::Tools, StreamEvent::Tools(_) | StreamEvent::End { .. }) => true,
            (
                StreamMode::Checkpoints,
                StreamEvent::CheckpointSaved { .. } | StreamEvent::End { .. },
            ) => true,
            (StreamMode::Tasks, StreamEvent::TaskDetail { .. } | StreamEvent::End { .. }) => true,
            (StreamMode::Multi(modes), _) => {
                // Check if any sub-mode matches this event
                Self::mode_matches_multi(modes, event)
            }
            _ => false,
        }
    }

    /// Check if metadata contains "nostream" tag
    #[must_use]
    fn has_nostream_tag(metadata: &MessageStreamMetadata) -> bool {
        metadata.tags.iter().any(|tag| tag == "nostream")
    }

    /// Check if event matches any mode in a Multi mode
    #[must_use]
    fn mode_matches_multi(modes: &[StreamMode], event: &StreamEvent<S>) -> bool {
        modes.iter().any(|m| Self::mode_matches_single(m, event))
    }

    /// Check if event matches a single mode
    #[must_use]
    #[allow(
        clippy::match_same_arms,
        clippy::missing_const_for_fn,
        reason = "each arm is explicit for clarity; non-const for multi-mode filtering"
    )]
    fn mode_matches_single(mode: &StreamMode, event: &StreamEvent<S>) -> bool {
        match (mode, event) {
            (
                StreamMode::Values,
                StreamEvent::Values { .. }
                | StreamEvent::FilteredValues { .. }
                | StreamEvent::End { .. },
            ) => true,
            (
                StreamMode::Updates,
                StreamEvent::Updates { .. }
                | StreamEvent::FilteredUpdates { .. }
                | StreamEvent::End { .. },
            ) => true,
            (StreamMode::Messages, StreamEvent::Messages { .. } | StreamEvent::End { .. }) => true,
            (StreamMode::Custom, StreamEvent::Custom { .. } | StreamEvent::End { .. }) => true,
            (StreamMode::Debug, _) => true,
            (StreamMode::Tools, StreamEvent::Tools(_) | StreamEvent::End { .. }) => true,
            (
                StreamMode::Checkpoints,
                StreamEvent::CheckpointSaved { .. } | StreamEvent::End { .. },
            ) => true,
            (StreamMode::Tasks, StreamEvent::TaskDetail { .. } | StreamEvent::End { .. }) => true,
            (StreamMode::Multi(_), _) => false,
            _ => false,
        }
    }
}

/// Writer for streaming events from a node
///
/// Nodes receive this writer to emit custom streaming events during execution.
/// The writer carries a sender channel, the current node name, stream mode,
/// and namespace stack for subgraph isolation.
#[derive(Clone)]
pub struct StreamWriter<S: State> {
    tx: Option<tokio::sync::mpsc::Sender<StreamEvent<S>>>,
    node: String,
    mode: StreamMode,
    ns: Vec<String>,
}

impl<S: State> StreamWriter<S> {
    /// Create a new writer backed by a real channel
    #[must_use]
    pub const fn new(
        tx: tokio::sync::mpsc::Sender<StreamEvent<S>>,
        node: String,
        mode: StreamMode,
    ) -> Self {
        Self {
            tx: Some(tx),
            node,
            mode,
            ns: Vec::new(),
        }
    }

    /// Create a disconnected writer (no-op send)
    ///
    /// Used when streaming is not configured for the current execution.
    #[must_use]
    pub const fn disconnected(node: String, mode: StreamMode) -> Self {
        Self {
            tx: None,
            node,
            mode,
            ns: Vec::new(),
        }
    }

    /// Create a child writer with an additional namespace segment (for subgraphs)
    #[must_use]
    pub fn with_ns(&self, ns_segment: String) -> Self {
        let mut new_ns = self.ns.clone();
        new_ns.push(ns_segment);
        Self {
            tx: self.tx.clone(),
            node: self.node.clone(),
            mode: self.mode.clone(),
            ns: new_ns,
        }
    }

    /// Send a custom stream event through the channel.
    ///
    /// Silently drops the event if the writer is disconnected or the event
    /// does not match the configured [`StreamMode`].
    pub async fn send(&self, data: serde_json::Value) {
        let Some(ref tx) = self.tx else {
            return;
        };

        let event = StreamEvent::Custom {
            node: self.node.clone(),
            data,
            ns: self.ns.clone(),
        };

        let emitter = EventEmitter::new(tx.clone(), self.mode.clone());
        if emitter.should_emit(&event) {
            let _ = tx.send(event).await;
        }
    }
}

impl<S: State> std::fmt::Debug for StreamWriter<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamWriter")
            .field("tx", &self.tx.is_some())
            .field("node", &self.node)
            .field("mode", &self.mode)
            .field("ns", &self.ns)
            .finish()
    }
}

/// Call an LLM with streaming, forwarding chunks to the event emitter.
///
/// Accumulates the full LLM response while simultaneously forwarding
/// each chunk as a [`StreamEvent::Messages`] event through the emitter.
/// Tool call arguments are accumulated from delta chunks and parsed as
/// JSON values in the final message.
///
/// # Errors
///
/// Returns [`crate::llm::LlmError`] if the model fails to start streaming,
/// a chunk contains an error, or tool call arguments are not valid JSON.
pub async fn call_llm_streaming<S: State, M: crate::llm::ChatModel>(
    model: &M,
    messages: &[crate::state::Message],
    options: Option<&crate::llm::CallOptions>,
    emitter: &EventEmitter<S>,
    node_name: &str,
) -> Result<crate::state::Message, crate::llm::LlmError> {
    let mut stream = model.stream(messages, options).await?;
    let mut full_content = String::new();
    let mut tool_calls: Vec<crate::state::ToolCall> = Vec::new();
    let mut total_usage = crate::state::TokenUsage::default();

    // Extract tags from CallOptions for nostream filtering
    #[allow(clippy::option_if_let_else, reason = "explicit match is clearer")]
    let tags: Vec<String> = match options {
        Some(opts) => opts.tags.clone(),
        None => Vec::new(),
    };

    while let Some(chunk_result) = stream.next().await {
        let chunk = chunk_result?;

        full_content.push_str(&chunk.content);

        for tc_chunk in &chunk.tool_call_chunks {
            while tool_calls.len() <= tc_chunk.index {
                tool_calls.push(crate::state::ToolCall {
                    id: String::new(),
                    name: String::new(),
                    arguments: serde_json::Value::Null,
                });
            }
            let tc = &mut tool_calls[tc_chunk.index];
            if let Some(ref id) = tc_chunk.id {
                id.clone_into(&mut tc.id);
            }
            if let Some(ref name) = tc_chunk.name {
                name.clone_into(&mut tc.name);
            }
            if !tc_chunk.args_delta.is_empty() {
                match &mut tc.arguments {
                    serde_json::Value::String(s) => s.push_str(&tc_chunk.args_delta),
                    serde_json::Value::Null => {
                        tc.arguments = serde_json::Value::String(tc_chunk.args_delta.clone());
                    }
                    other => {
                        let mut s = match std::mem::replace(other, serde_json::Value::Null) {
                            serde_json::Value::String(existing) => existing,
                            _ => String::new(),
                        };
                        s.push_str(&tc_chunk.args_delta);
                        *other = serde_json::Value::String(s);
                    }
                }
            }
        }

        if let Some(ref usage) = chunk.usage {
            total_usage.input_tokens += usage.input_tokens;
            total_usage.output_tokens += usage.output_tokens;
            total_usage.total_tokens += usage.total_tokens;
        }

        let stream_chunk = MessageChunk {
            content: chunk.content,
            tool_call_chunks: chunk.tool_call_chunks,
            usage_delta: chunk.usage,
        };

        let event = StreamEvent::Messages {
            chunk: stream_chunk,
            metadata: MessageStreamMetadata {
                node: node_name.to_string(),
                model: model.model_name().to_string(),
                tags: tags.clone(),
                ns: emitter.ns().to_vec(),
            },
        };

        if emitter.should_emit(&event) {
            emitter.emit(event).await;
        }
    }

    // Parse accumulated argument strings into JSON values
    for tc in &mut tool_calls {
        if let serde_json::Value::String(s) = &tc.arguments {
            tc.arguments = serde_json::from_str(s).unwrap_or_else(|_| {
                serde_json::Value::String(std::mem::take(&mut tc.arguments).to_string())
            });
        }
    }

    total_usage.total_tokens = total_usage.input_tokens + total_usage.output_tokens;

    Ok(crate::state::Message {
        id: uuid::Uuid::new_v4().to_string(),
        role: crate::state::Role::Ai,
        content: crate::state::Content::Text(full_content),
        tool_calls,
        tool_call_id: None,
        name: None,
        usage: Some(total_usage),
    })
}

/// Configuration for batching LLM streaming chunks.
///
/// Controls how token chunks are accumulated before forwarding to stream
/// consumers. Batching reduces overhead for high-volume token streaming
/// by coalescing small chunks into fewer, larger deliveries.
#[derive(Clone, Debug)]
pub struct MessageBatchConfig {
    /// Maximum number of chunks to accumulate before flushing.
    ///
    /// When this many chunks are collected, they are flushed immediately
    /// regardless of the time threshold.
    pub max_chunks: usize,

    /// Maximum time in milliseconds to wait before flushing.
    ///
    /// If this duration elapses without reaching `max_chunks`, the
    /// accumulated chunks are flushed. A value of `None` disables
    /// time-based flushing.
    pub flush_interval_ms: Option<u64>,
}

impl Default for MessageBatchConfig {
    fn default() -> Self {
        Self {
            max_chunks: 10,
            flush_interval_ms: Some(100),
        }
    }
}

impl MessageBatchConfig {
    /// Create a new batch config with the specified parameters.
    #[must_use]
    pub const fn new(max_chunks: usize, flush_interval_ms: Option<u64>) -> Self {
        Self {
            max_chunks,
            flush_interval_ms,
        }
    }

    /// Create config with no batching (flush every chunk immediately).
    #[must_use]
    pub const fn no_batching() -> Self {
        Self {
            max_chunks: 1,
            flush_interval_ms: None,
        }
    }
}

/// Filter a JSON object to retain only the specified keys.
///
/// If `keys` is empty the original value is returned unchanged.
/// Non-object values pass through unmodified.
pub(crate) fn filter_json_by_keys(value: serde_json::Value, keys: &[String]) -> serde_json::Value {
    if keys.is_empty() {
        return value;
    }
    match value {
        serde_json::Value::Object(mut map) => {
            let keep: std::collections::HashSet<&String> = keys.iter().collect();
            map.retain(|k, _| keep.contains(k));
            serde_json::Value::Object(map)
        }
        other => other,
    }
}

/// Configuration for streaming.
#[derive(Clone, Debug, Default)]
pub struct StreamConfig {
    pub mode: StreamMode,
    pub include_subgraphs: bool,
    pub subgraph_filter: Option<Vec<String>>,
    /// Optional field names to filter in Values/Updates events
    pub output_keys: Option<Vec<String>>,
    /// Batching configuration for Messages mode streaming.
    pub message_batch_config: MessageBatchConfig,
    /// Resumption state for replaying a stream from a checkpoint.
    ///
    /// When set, events at or before [`StreamResumption::last_step`] are
    /// silently skipped so the consumer only receives new events.
    pub resumption: Option<StreamResumption>,
}

impl StreamConfig {
    #[must_use]
    pub const fn new(mode: StreamMode) -> Self {
        Self {
            mode,
            include_subgraphs: false,
            subgraph_filter: None,
            output_keys: None,
            message_batch_config: MessageBatchConfig {
                max_chunks: 10,
                flush_interval_ms: Some(100),
            },
            resumption: None,
        }
    }

    #[must_use]
    pub const fn with_subgraphs(mut self, include: bool) -> Self {
        self.include_subgraphs = include;
        self
    }

    #[must_use]
    pub fn with_subgraph_filter(mut self, filter: Vec<String>) -> Self {
        self.subgraph_filter = Some(filter);
        self
    }

    /// Filter output to only include specified fields in Values/Updates events.
    #[must_use]
    pub fn with_output_keys(mut self, keys: Vec<String>) -> Self {
        self.output_keys = Some(keys);
        self
    }

    /// Set the batching configuration for Messages mode streaming.
    #[must_use]
    pub const fn with_message_batch_config(mut self, config: MessageBatchConfig) -> Self {
        self.message_batch_config = config;
        self
    }

    /// Set the resumption state for checkpoint-based stream replay.
    ///
    /// Events at or before `resumption.last_step` are silently skipped,
    /// allowing consumers to resume from the last checkpoint without
    /// receiving already-processed events.
    #[must_use]
    pub fn with_resumption(mut self, resumption: StreamResumption) -> Self {
        self.resumption = Some(resumption);
        self
    }
}

/// Resumption state for streaming
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct StreamResumption {
    pub run_id: String,
    pub last_checkpoint_id: Option<String>,
    pub last_step: Option<usize>,
}

impl StreamResumption {
    #[must_use]
    pub const fn new(
        run_id: String,
        last_checkpoint_id: Option<String>,
        last_step: Option<usize>,
    ) -> Self {
        Self {
            run_id,
            last_checkpoint_id,
            last_step,
        }
    }

    #[must_use]
    pub const fn should_skip(&self, current_step: usize) -> bool {
        match self.last_step {
            Some(last_step) => current_step <= last_step,
            None => false,
        }
    }
}

/// Transformer that parses JSON strings
#[derive(Clone, Debug, Default)]
pub struct JsonParseTransformer;

impl JsonParseTransformer {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl StreamTransformer for JsonParseTransformer {
    #[allow(
        clippy::option_if_let_else,
        reason = "project rules prohibit map_or with unwrap; match is explicit and readable"
    )]
    fn transform(&self, data: serde_json::Value) -> serde_json::Value {
        match data {
            serde_json::Value::String(s) => match serde_json::from_str(&s) {
                Ok(v) => v,
                Err(_) => serde_json::Value::Null,
            },
            _ => data,
        }
    }
}

/// Transformer that filters specific fields
#[derive(Clone, Debug)]
pub struct FilterFieldsTransformer {
    pub fields: Vec<String>,
}

impl FilterFieldsTransformer {
    #[must_use]
    pub const fn new(fields: Vec<String>) -> Self {
        Self { fields }
    }
}

impl StreamTransformer for FilterFieldsTransformer {
    fn transform(&self, data: serde_json::Value) -> serde_json::Value {
        match data {
            serde_json::Value::Object(mut map) => {
                let keys_to_keep: std::collections::HashSet<_> = self.fields.iter().collect();
                map.retain(|k, _| keys_to_keep.contains(k));
                serde_json::Value::Object(map)
            }
            _ => data,
        }
    }
}

/// Transformer that batches events
#[derive(Clone, Debug)]
pub struct BatchTransformer {
    pub size: usize,
}

impl BatchTransformer {
    #[must_use]
    pub const fn new(size: usize) -> Self {
        Self { size }
    }
}

impl StreamTransformer for BatchTransformer {
    fn transform(&self, data: serde_json::Value) -> serde_json::Value {
        // Batching requires stateful accumulation of events
        // This transformer defines the batching size; the runtime
        // manages the actual batching logic using this configuration
        data
    }
}

// Rust guideline compliant 2026-05-21

#[cfg(test)]
mod tests {
    use super::{
        EventEmitter, MessageBatchConfig, MessageChunk, MessageStreamMetadata, StreamConfig,
        StreamEvent, StreamMode, StreamResumption, ToolsEvent,
    };
    use crate::state::{FieldsChanged, State};

    /// Minimal state implementation for `EventEmitter` tests.
    #[derive(Clone, Debug)]
    struct TestState;

    impl State for TestState {
        type Update = TestStateUpdate;

        fn apply(&mut self, _update: Self::Update) -> FieldsChanged {
            FieldsChanged(0)
        }

        fn reset_ephemeral(&mut self) {}
    }

    #[derive(Clone, Debug, Default)]
    struct TestStateUpdate;

    #[test]
    fn message_batch_config_default() {
        let config = MessageBatchConfig::default();
        assert_eq!(config.max_chunks, 10);
        assert_eq!(config.flush_interval_ms, Some(100));
    }

    #[test]
    fn message_batch_config_no_batching() {
        let config = MessageBatchConfig::no_batching();
        assert_eq!(config.max_chunks, 1);
        assert_eq!(config.flush_interval_ms, None);
    }

    #[test]
    fn message_batch_config_new_custom() {
        let config = MessageBatchConfig::new(50, Some(200));
        assert_eq!(config.max_chunks, 50);
        assert_eq!(config.flush_interval_ms, Some(200));
    }

    // --- StreamResumption unit tests ---

    #[test]
    fn resumption_should_skip_returns_true_when_step_at_last_step() {
        let r = StreamResumption::new("run1".to_string(), None, Some(3));
        assert!(r.should_skip(3));
    }

    #[test]
    fn resumption_should_skip_returns_true_when_step_before_last_step() {
        let r = StreamResumption::new("run1".to_string(), None, Some(3));
        assert!(r.should_skip(2));
        assert!(r.should_skip(0));
    }

    #[test]
    fn resumption_should_skip_returns_false_when_step_after_last_step() {
        let r = StreamResumption::new("run1".to_string(), None, Some(3));
        assert!(!r.should_skip(4));
        assert!(!r.should_skip(100));
    }

    #[test]
    fn resumption_should_skip_returns_false_when_last_step_is_none() {
        let r = StreamResumption::new("run1".to_string(), None, None);
        assert!(!r.should_skip(0));
        assert!(!r.should_skip(100));
    }

    // --- StreamConfig resumption builder ---

    #[test]
    fn stream_config_default_has_no_resumption() {
        let config = StreamConfig::default();
        assert!(config.resumption.is_none());
    }

    #[test]
    fn stream_config_new_has_no_resumption() {
        let config = StreamConfig::new(StreamMode::Values);
        assert!(config.resumption.is_none());
    }

    #[test]
    fn stream_config_with_resumption_sets_field() {
        let r = StreamResumption::new("run1".to_string(), Some("cp-5".to_string()), Some(5));
        let config = StreamConfig::new(StreamMode::Values).with_resumption(r);
        assert!(config.resumption.is_some());
        let resumption = config.resumption.expect("resumption should be set");
        assert_eq!(resumption.run_id, "run1");
        assert_eq!(resumption.last_checkpoint_id, Some("cp-5".to_string()));
        assert_eq!(resumption.last_step, Some(5));
    }

    // --- EventEmitter nostream tag filtering ---

    #[test]
    fn should_emit_messages_event_without_nostream() {
        let (tx, _rx) = tokio::sync::mpsc::channel(16);
        let emitter = EventEmitter::<TestState>::new(tx, StreamMode::Messages);
        let event = StreamEvent::Messages {
            chunk: MessageChunk {
                content: "hello".to_string(),
                tool_call_chunks: Vec::new(),
                usage_delta: None,
            },
            metadata: MessageStreamMetadata {
                node: "agent".to_string(),
                model: "test".to_string(),
                tags: vec![],
                ns: Vec::new(),
            },
        };
        assert!(emitter.should_emit(&event));
    }

    #[test]
    fn should_emit_messages_event_with_nostream_suppressed() {
        let (tx, _rx) = tokio::sync::mpsc::channel(16);
        let emitter = EventEmitter::<TestState>::new(tx, StreamMode::Messages);
        let event = StreamEvent::Messages {
            chunk: MessageChunk {
                content: "hello".to_string(),
                tool_call_chunks: Vec::new(),
                usage_delta: None,
            },
            metadata: MessageStreamMetadata {
                node: "agent".to_string(),
                model: "test".to_string(),
                tags: vec!["nostream".to_string()],
                ns: Vec::new(),
            },
        };
        assert!(!emitter.should_emit(&event));
    }

    #[test]
    fn should_emit_messages_event_with_other_tags_not_suppressed() {
        let (tx, _rx) = tokio::sync::mpsc::channel(16);
        let emitter = EventEmitter::<TestState>::new(tx, StreamMode::Messages);
        let event = StreamEvent::Messages {
            chunk: MessageChunk {
                content: "hello".to_string(),
                tool_call_chunks: Vec::new(),
                usage_delta: None,
            },
            metadata: MessageStreamMetadata {
                node: "agent".to_string(),
                model: "test".to_string(),
                tags: vec!["fast".to_string(), "stream".to_string()],
                ns: Vec::new(),
            },
        };
        assert!(emitter.should_emit(&event));
    }

    #[test]
    fn should_emit_end_event_always_in_messages_mode() {
        let (tx, _rx) = tokio::sync::mpsc::channel(16);
        let emitter = EventEmitter::<TestState>::new(tx, StreamMode::Messages);
        let event = StreamEvent::End {
            output: TestState,
        };
        assert!(emitter.should_emit(&event));
    }

    #[test]
    fn should_emit_tools_event_in_tools_mode() {
        let (tx, _rx) = tokio::sync::mpsc::channel(16);
        let emitter = EventEmitter::<TestState>::new(tx, StreamMode::Tools);
        let event = StreamEvent::Tools(ToolsEvent::ToolStarted {
            tool_name: "search".to_string(),
            tool_call_id: "call_1".to_string(),
            node: "tools".to_string(),
            input: serde_json::json!({}),
        });
        assert!(emitter.should_emit(&event));
    }

    #[test]
    fn should_emit_tool_output_delta_in_tools_mode() {
        let (tx, _rx) = tokio::sync::mpsc::channel(16);
        let emitter = EventEmitter::<TestState>::new(tx, StreamMode::Tools);
        let event = StreamEvent::Tools(ToolsEvent::ToolOutputDelta {
            tool_call_id: "call_1".to_string(),
            delta: "partial".to_string(),
        });
        assert!(emitter.should_emit(&event));
    }

    #[test]
    fn should_emit_tool_finished_in_tools_mode() {
        let (tx, _rx) = tokio::sync::mpsc::channel(16);
        let emitter = EventEmitter::<TestState>::new(tx, StreamMode::Tools);
        let event = StreamEvent::Tools(ToolsEvent::ToolFinished {
            tool_call_id: "call_1".to_string(),
            output: serde_json::json!({"result": "ok"}),
            duration_ms: 100,
        });
        assert!(emitter.should_emit(&event));
    }
}
