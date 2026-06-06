//! Channel trait and channel types for state field access with checkpoint support
//!
//! A Channel wraps a value with specific update and checkpoint semantics.
//! Different channel types control how values are updated, persisted, and consumed.

use serde::de::DeserializeOwned;
use serde::ser::SerializeStruct;
use std::collections::HashSet;

/// Reducer trait defining merge semantics for state fields
///
/// Each field in a State can have its own reducer, defining how multiple
/// writes in the same superstep are combined.
pub trait Reducer<T> {
    /// Merge a single value (fast path avoiding Vec allocation)
    fn reduce_one(current: &mut T, value: T) {
        Self::reduce(current, vec![value]);
    }

    /// Merge multiple values into current
    ///
    /// Values are provided in the order tasks completed (not task spawn order).
    /// For deterministic results, use associative reducers like `AppendReducer`.
    fn reduce(current: &mut T, values: Vec<T>);
}

/// Replace reducer: only one writer per superstep (default)
///
/// Equivalent to `LangGraph`'s `LastValue` channel.
/// Panics if multiple nodes write to the same field in one superstep.
#[derive(Debug)]
pub struct ReplaceReducer;

impl<T> Reducer<T> for ReplaceReducer {
    fn reduce(current: &mut T, values: Vec<T>) {
        assert!(
            values.len() <= 1,
            "Replace reducer: multiple writes in same superstep"
        );
        if let Some(v) = values.into_iter().next() {
            *current = v;
        }
    }
}

/// Append reducer: accumulate all writes
///
/// Equivalent to `LangGraph`'s `BinaryOperatorAggregate` with operator.add.
/// All writes are extended in order.
#[derive(Debug)]
pub struct AppendReducer;

impl<T> Reducer<Vec<T>> for AppendReducer {
    fn reduce_one(current: &mut Vec<T>, value: Vec<T>) {
        current.extend(value);
    }

    fn reduce(current: &mut Vec<T>, values: Vec<Vec<T>>) {
        for v in values {
            current.extend(v);
        }
    }
}

/// `AnyValue` reducer: assumes all values are equal
///
/// Similar to `LastValue`, but semantically assumes all writers provide
/// the same value. Uses the last value if they differ.
#[derive(Debug)]
pub struct AnyValueReducer;

impl<T: PartialEq + Clone> Reducer<T> for AnyValueReducer {
    fn reduce(current: &mut T, values: Vec<T>) {
        if let Some(last) = values.last() {
            // Semantic check: all values should be equal
            if let Some(first) = values.first() {
                debug_assert!(
                    values.iter().all(|v| v == first),
                    "AnyValue reducer: all values should be equal"
                );
            }
            *current = last.clone();
        }
    }
}

/// `LastWriteWins` reducer: allows multiple writers, last one wins
///
/// Similar to `ReplaceReducer`, but doesn't panic on multiple writes.
#[derive(Debug)]
pub struct LastWriteWinsReducer;

impl<T> Reducer<T> for LastWriteWinsReducer {
    fn reduce(current: &mut T, values: Vec<T>) {
        if let Some(v) = values.into_iter().last() {
            *current = v;
        }
    }
}

/// Bypass reducer: overwrite value directly, bypassing normal merge
///
/// When `Overwrite<T>` is used in an update, it bypasses the field's reducer
/// and directly replaces the value. Custom serde uses `{"__overwrite__": value}`
/// wire format for `LangGraph` checkpoint compatibility.
pub struct Overwrite<T>(pub T);

impl<T: std::fmt::Debug> std::fmt::Debug for Overwrite<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Overwrite").field(&self.0).finish()
    }
}

impl<T> Overwrite<T> {
    /// Get a reference to the inner value
    #[must_use]
    pub const fn get(&self) -> &T {
        &self.0
    }

    /// Convert into the inner value
    #[must_use]
    pub fn into_inner(self) -> T {
        self.0
    }

    /// Create a new Overwrite wrapper
    #[must_use]
    pub const fn new(value: T) -> Self {
        Self(value)
    }
}

impl<T: serde::Serialize> serde::Serialize for Overwrite<T> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut s = serializer.serialize_struct("__overwrite__", 1)?;
        s.serialize_field("__overwrite__", &self.0)?;
        s.end()
    }
}

impl<'de, T: serde::Deserialize<'de>> serde::Deserialize<'de> for Overwrite<T> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(serde::Deserialize)]
        struct Wrapper<T> {
            __overwrite__: T,
        }
        let wrapper = Wrapper::deserialize(deserializer)?;
        Ok(Self(wrapper.__overwrite__))
    }
}

/// Named barrier channel: waits for all registered named sources to write
///
/// This channel implements barrier/wait-all semantics for parallel workflows.
/// The value is only available after ALL required named sources have written
/// to it. Each source must provide a unique name for tracking.
///
/// # Type Parameters
///
/// * `T` - The value type stored in the channel
/// * `R` - The reducer type that defines how multiple writes are merged
///
/// # Examples
///
/// ```
/// use juncture_core::state::channel::{NamedBarrierChannel, ReplaceReducer};
///
/// let mut channel: NamedBarrierChannel<i32, ReplaceReducer> =
///     NamedBarrierChannel::new_with_sources(0, ["node_a", "node_b"].into_iter().map(String::from));
///
/// // Initially not available
/// assert!(!channel.is_available());
///
/// // After first write, still not available
/// channel.update("node_a".to_string(), vec![42]).expect("update should succeed");
/// assert!(!channel.is_available());
///
/// // After second write, becomes available
/// channel.update("node_b".to_string(), vec![100]).expect("update should succeed");
/// assert!(channel.is_available());
/// assert_eq!(*channel.get(), 100); // Last write wins
/// ```
#[derive(Debug)]
pub struct NamedBarrierChannel<T, R: Reducer<T>> {
    value: T,
    required_sources: HashSet<String>,
    seen_sources: HashSet<String>,
    _reducer: std::marker::PhantomData<R>,
}

impl<T, R: Reducer<T>> NamedBarrierChannel<T, R> {
    /// Create a new named barrier channel with the given initial value and required sources
    ///
    /// # Arguments
    ///
    /// * `value` - The initial value for the channel
    /// * `required_sources` - Iterator of source names that must all write before the barrier completes
    #[must_use]
    pub fn new_with_sources(value: T, required_sources: impl IntoIterator<Item = String>) -> Self {
        let sources: HashSet<String> = required_sources.into_iter().collect();
        Self {
            value,
            required_sources: sources,
            seen_sources: HashSet::new(),
            _reducer: std::marker::PhantomData,
        }
    }

    /// Create a new named barrier channel with no required sources
    ///
    /// This channel will be immediately available. Use this when you plan to
    /// add required sources later or when the barrier should always be complete.
    #[must_use]
    pub fn new(value: T) -> Self {
        Self {
            value,
            required_sources: HashSet::new(),
            seen_sources: HashSet::new(),
            _reducer: std::marker::PhantomData,
        }
    }

    /// Add a required source to the barrier
    ///
    /// If the source has already written, this will immediately mark it as seen.
    pub fn add_required_source(&mut self, source: String) {
        self.required_sources.insert(source);
    }

    /// Check if all required sources have written
    ///
    /// Returns `true` only when ALL required sources have written to this channel.
    #[must_use]
    pub fn is_available(&self) -> bool {
        if self.required_sources.is_empty() {
            return true;
        }
        self.required_sources
            .iter()
            .all(|source| self.seen_sources.contains(source))
    }

    /// Get the set of required source names
    #[must_use]
    pub const fn required_sources(&self) -> &HashSet<String> {
        &self.required_sources
    }

    /// Get the set of source names that have written so far
    #[must_use]
    pub const fn seen_sources(&self) -> &HashSet<String> {
        &self.seen_sources
    }

    /// Check if a specific source has written
    #[must_use]
    pub fn has_written(&self, source: &str) -> bool {
        self.seen_sources.contains(source)
    }

    /// Reset the barrier, clearing seen sources while keeping required sources
    ///
    /// This is useful for reusing the barrier across multiple supersteps.
    pub fn reset(&mut self) {
        self.seen_sources.clear();
    }
}

impl<T, R> Channel<T> for NamedBarrierChannel<T, R>
where
    T: Default + Clone + Send + Sync + serde::Serialize + DeserializeOwned + 'static,
    R: Reducer<T> + Send + Sync + 'static,
{
    fn update(&mut self, values: Vec<T>) -> bool {
        // Channel trait update doesn't support named sources.
        // When using the generic Channel trait, we apply all values directly
        // to the channel. This is useful when the caller doesn't care about
        // named barrier tracking and just wants to update the value.
        if values.is_empty() {
            return false;
        }
        // Apply the reducer to merge all values
        R::reduce(&mut self.value, values);
        // Mark all required sources as seen since we received an update
        self.seen_sources = self.required_sources.clone();
        true
    }

    fn get(&self) -> &T {
        &self.value
    }

    fn consume(&mut self) -> bool {
        false
    }

    fn checkpoint(&self) -> Option<serde_json::Value> {
        // Persist the value and seen sources
        serde_json::to_value(&(self.value.clone(), self.seen_sources.clone())).ok()
    }

    fn from_checkpoint(value: serde_json::Value) -> Result<Self, String>
    where
        Self: Sized,
    {
        let (parsed_value, seen_sources): (T, HashSet<String>) = serde_json::from_value(value)
            .map_err(|e| format!("checkpoint deserialization failed: {e}"))?;
        Ok(Self {
            value: parsed_value,
            required_sources: HashSet::new(),
            seen_sources,
            _reducer: std::marker::PhantomData,
        })
    }
}

impl<T, R: Reducer<T>> NamedBarrierChannel<T, R> {
    /// Update from a named source
    ///
    /// This is the primary method for `NamedBarrierChannel`, allowing named sources
    /// to write to the channel. The barrier completes only after all required sources
    /// have written.
    ///
    /// # Panics
    ///
    /// Panics if `source_name` is not in the set of required sources (when sources are configured).
    pub fn update(&mut self, source_name: String, values: Vec<T>) -> bool {
        assert!(
            self.required_sources.is_empty() || self.required_sources.contains(&source_name),
            "NamedBarrierChannel: source '{source_name}' not in required sources"
        );

        if values.is_empty() {
            return false;
        }

        R::reduce(&mut self.value, values);
        self.seen_sources.insert(source_name);
        true
    }
}

/// Topic channel: accumulates all published values into a list
///
/// This channel implements pub/sub messaging patterns where all writes
/// are accumulated into a list. Each value is appended independently,
/// allowing multiple publishers to send messages to the same topic.
///
/// # Type Parameters
///
/// * `T` - The message type stored in the topic
///
/// # Examples
///
/// ```
/// use juncture_core::state::channel::TopicChannel;
///
/// let mut channel: TopicChannel<String> = TopicChannel::new();
///
/// // Publish messages
/// channel.update(vec!["hello".to_string()]);
/// channel.update(vec!["world".to_string()]);
///
/// // Get all accumulated messages
/// let messages = channel.get();
/// assert_eq!(messages.len(), 2);
/// assert_eq!(messages[0], "hello");
/// assert_eq!(messages[1], "world");
///
/// // Reset for next superstep
/// channel.reset();
/// assert!(messages.is_empty());
/// ```
#[derive(Debug, Clone)]
pub struct TopicChannel<T> {
    messages: Vec<T>,
}

impl<T> TopicChannel<T> {
    /// Create a new empty topic channel
    #[must_use]
    pub const fn new() -> Self {
        Self {
            messages: Vec::new(),
        }
    }

    /// Get the number of messages in the topic
    #[must_use]
    pub const fn len(&self) -> usize {
        self.messages.len()
    }

    /// Check if the topic is empty
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }

    /// Reset the topic, clearing all messages
    ///
    /// This is typically called at the start of each superstep to clear
    /// ephemeral message accumulations.
    pub fn reset(&mut self) {
        self.messages.clear();
    }

    /// Get an iterator over the messages
    pub fn iter(&self) -> std::slice::Iter<'_, T> {
        self.messages.iter()
    }
}

impl<T> Default for TopicChannel<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<'a, T> IntoIterator for &'a TopicChannel<T> {
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<T> Channel<Vec<T>> for TopicChannel<T>
where
    T: Clone + Send + Sync + serde::Serialize + DeserializeOwned + 'static,
{
    fn update(&mut self, values: Vec<Vec<T>>) -> bool {
        if values.is_empty() {
            return false;
        }
        // Extend messages with all new values (flatten the vec of vecs)
        for batch in values {
            self.messages.extend(batch);
        }
        true
    }

    fn get(&self) -> &Vec<T> {
        &self.messages
    }

    fn consume(&mut self) -> bool {
        let was_empty = self.messages.is_empty();
        self.messages.clear();
        !was_empty
    }

    fn checkpoint(&self) -> Option<serde_json::Value> {
        serde_json::to_value(&self.messages).ok()
    }

    fn from_checkpoint(value: serde_json::Value) -> Result<Self, String>
    where
        Self: Sized,
    {
        let messages: Vec<T> = serde_json::from_value(value)
            .map_err(|e| format!("checkpoint deserialization failed: {e}"))?;
        Ok(Self { messages })
    }
}

/// Channel trait for state field access with checkpoint support
///
/// A Channel wraps a value with specific update and checkpoint semantics.
/// Different channel types control how values are updated, persisted, and consumed.
pub trait Channel<T>: Send + Sync + 'static {
    /// Update the channel with new values. Returns true if the value changed.
    fn update(&mut self, values: Vec<T>) -> bool;

    /// Get the current value
    fn get(&self) -> &T;

    /// Check if the channel has been consumed (for trigger-based activation)
    fn consume(&mut self) -> bool;

    /// Create a checkpoint of the current value for persistence
    fn checkpoint(&self) -> Option<serde_json::Value>;

    /// Restore from a checkpoint value
    ///
    /// # Errors
    ///
    /// Returns an error if the checkpoint value cannot be deserialized into
    /// the channel's value type.
    fn from_checkpoint(value: serde_json::Value) -> Result<Self, String>
    where
        Self: Sized;
}

/// Untracked channel: value is not persisted across checkpoints
///
/// Wraps a value with a reducer. Checkpoints return `None` so the value
/// is never persisted. This is useful for transient computation state
/// that should not survive a restart.
#[derive(Debug)]
pub struct UntrackedChannel<T, R: Reducer<T>> {
    value: T,
    _reducer: std::marker::PhantomData<R>,
}

impl<T, R: Reducer<T>> UntrackedChannel<T, R> {
    /// Create a new untracked channel with the given initial value
    #[must_use]
    pub const fn new(value: T) -> Self {
        Self {
            value,
            _reducer: std::marker::PhantomData,
        }
    }
}

impl<T: Default + Send + Sync + 'static, R: Reducer<T> + Send + Sync + 'static> Channel<T>
    for UntrackedChannel<T, R>
{
    fn update(&mut self, values: Vec<T>) -> bool {
        if values.is_empty() {
            return false;
        }
        R::reduce(&mut self.value, values);
        true
    }

    fn get(&self) -> &T {
        &self.value
    }

    fn consume(&mut self) -> bool {
        false
    }

    fn checkpoint(&self) -> Option<serde_json::Value> {
        None
    }

    fn from_checkpoint(_value: serde_json::Value) -> Result<Self, String> {
        Ok(Self::new(T::default()))
    }
}

/// Ephemeral channel: value is cleared at the start of each superstep
///
/// Has a `consumed` flag set by `consume()`. The value resets between
/// supersteps and is never persisted.
#[derive(Debug)]
pub struct EphemeralChannel<T, R: Reducer<T>> {
    value: T,
    consumed: bool,
    _reducer: std::marker::PhantomData<R>,
}

impl<T, R: Reducer<T>> EphemeralChannel<T, R> {
    /// Create a new ephemeral channel with the given initial value
    #[must_use]
    pub const fn new(value: T) -> Self {
        Self {
            value,
            consumed: false,
            _reducer: std::marker::PhantomData,
        }
    }
}

impl<T: Default + Send + Sync + 'static, R: Reducer<T> + Send + Sync + 'static> Channel<T>
    for EphemeralChannel<T, R>
{
    fn update(&mut self, values: Vec<T>) -> bool {
        if values.is_empty() {
            return false;
        }
        self.consumed = false;
        R::reduce(&mut self.value, values);
        true
    }

    fn get(&self) -> &T {
        &self.value
    }

    fn consume(&mut self) -> bool {
        let was_consumed = self.consumed;
        self.consumed = true;
        was_consumed
    }

    fn checkpoint(&self) -> Option<serde_json::Value> {
        None
    }

    fn from_checkpoint(_value: serde_json::Value) -> Result<Self, String> {
        Ok(Self::new(T::default()))
    }
}

/// Last-value-after-finish channel: value only available after `finish()` is called
///
/// Before `finish()`, `get()` returns the default value. After `finish()`,
/// the written value becomes available. Checkpoints persist only if finished.
#[derive(Debug)]
pub struct LastValueAfterFinishChannel<T, R: Reducer<T>> {
    value: T,
    finished_value: Option<T>,
    is_finished: bool,
    _reducer: std::marker::PhantomData<R>,
}

impl<T, R: Reducer<T>> LastValueAfterFinishChannel<T, R> {
    /// Create a new channel with the given default value
    #[must_use]
    pub const fn new(value: T) -> Self {
        Self {
            value,
            finished_value: None,
            is_finished: false,
            _reducer: std::marker::PhantomData,
        }
    }

    /// Mark the channel as finished, making the value available
    pub const fn finish(&mut self) {
        self.is_finished = true;
    }

    /// Check if the channel has been finished and the value is available
    #[must_use]
    pub const fn is_available(&self) -> bool {
        self.is_finished
    }
}

impl<T, R> Channel<T> for LastValueAfterFinishChannel<T, R>
where
    T: Default + Clone + Send + Sync + serde::Serialize + DeserializeOwned + 'static,
    R: Reducer<T> + Send + Sync + 'static,
{
    fn update(&mut self, values: Vec<T>) -> bool {
        if values.is_empty() {
            return false;
        }
        R::reduce(&mut self.value, values);
        if self.is_finished {
            self.finished_value = Some(self.value.clone());
        }
        true
    }

    fn get(&self) -> &T {
        if self.is_finished {
            self.finished_value.as_ref().unwrap_or(&self.value)
        } else {
            &self.value
        }
    }

    fn consume(&mut self) -> bool {
        false
    }

    fn checkpoint(&self) -> Option<serde_json::Value> {
        // Only checkpoint if finished (preserves original semantic)
        // Save both value and is_finished state for complete restoration
        if self.is_finished {
            serde_json::to_value(&(self.value.clone(), self.is_finished)).ok()
        } else {
            None
        }
    }

    fn from_checkpoint(value: serde_json::Value) -> Result<Self, String> {
        // Try to parse as (value, is_finished) tuple first (new format)
        if let Ok((parsed_value, is_finished)) = serde_json::from_value::<(T, bool)>(value.clone())
        {
            let finished_value = is_finished.then(|| parsed_value.clone());
            return Ok(Self {
                value: parsed_value,
                finished_value,
                is_finished,
                _reducer: std::marker::PhantomData,
            });
        }

        // Fallback: try parsing as value only (old format for backward compatibility)
        let parsed_value: T = serde_json::from_value(value)
            .map_err(|e| format!("checkpoint deserialization failed: {e}"))?;
        Ok(Self {
            value: parsed_value,
            finished_value: None,
            is_finished: false,
            _reducer: std::marker::PhantomData,
        })
    }
}

/// Delta channel: append-heavy optimization with periodic snapshots
///
/// Tracks updates since the last snapshot and can replay writes for
/// restoring from a delta-based checkpoint. The `snapshot_frequency`
/// controls how often a full snapshot is taken instead of just recording
/// the delta.
#[derive(Debug)]
pub struct DeltaChannel<T, R: Reducer<T>> {
    value: T,
    /// How many updates between full snapshots (minimum 1)
    snapshot_frequency: usize,
    update_count_since_snapshot: usize,
    _reducer: std::marker::PhantomData<R>,
}

impl<T, R: Reducer<T>> DeltaChannel<T, R> {
    /// Create a new delta channel with the given initial value and snapshot frequency
    ///
    /// The snapshot frequency is clamped to a minimum of 1.
    #[must_use]
    pub fn new(value: T, snapshot_frequency: usize) -> Self {
        Self {
            value,
            snapshot_frequency: snapshot_frequency.max(1),
            update_count_since_snapshot: 0,
            _reducer: std::marker::PhantomData,
        }
    }

    /// Replay a sequence of writes to restore state from a checkpoint
    ///
    /// During checkpoint recovery, finds the last `Overwrite<T>` in the sequence
    /// and uses it as the baseline, then applies only the writes after it via
    /// the reducer. This implements the design specification for ancestor replay.
    pub fn replay_writes(&mut self, values: &[T])
    where
        T: Clone + serde::Serialize + DeserializeOwned,
    {
        if values.is_empty() {
            return;
        }

        // Find last Overwrite as baseline, only replay writes after it
        // This implements the design spec: "Find the last Overwrite as baseline,
        // only replay writes after it"
        let mut base = self.value.clone();
        let mut start_idx = 0;

        // Try to detect Overwrite wrappers in the sequence
        // Since we have &[T] not &[Overwrite<T>], we need to check
        // if any values were deserialized from Overwrite format
        for (i, v) in values.iter().enumerate() {
            // Check if this value is an Overwrite by attempting to detect
            // the special wire format. Since values are already deserialized,
            // we check if the JSON representation has __overwrite__ key
            if let Ok(json) = serde_json::to_value(v)
                && let Some(obj) = json.as_object()
                && obj.contains_key("__overwrite__")
            {
                // This is an Overwrite<T> value
                if let Ok(inner) = serde_json::from_value::<T>(
                    obj.get("__overwrite__").cloned().unwrap_or_default(),
                ) {
                    base = inner;
                    start_idx = i + 1;
                }
            }
        }

        // Apply remaining writes to baseline
        let remaining: Vec<T> = values[start_idx..].to_vec();
        if !remaining.is_empty() {
            R::reduce(&mut base, remaining);
        }
        self.value = base;
        self.update_count_since_snapshot = 0;
    }

    /// Check if a snapshot is due based on the update count
    #[must_use]
    pub const fn should_snapshot(&self) -> bool {
        self.update_count_since_snapshot >= self.snapshot_frequency
    }

    /// Mark the channel as finished, forcing a snapshot on the next checkpoint call
    ///
    /// This ensures the state is persisted when execution completes.
    pub const fn finish(&mut self) {
        self.update_count_since_snapshot = self.snapshot_frequency;
    }
}

impl<T, R> Channel<T> for DeltaChannel<T, R>
where
    T: Default + Clone + Send + Sync + serde::Serialize + DeserializeOwned + 'static,
    R: Reducer<T> + Send + Sync + 'static,
{
    fn update(&mut self, values: Vec<T>) -> bool {
        if values.is_empty() {
            return false;
        }
        R::reduce(&mut self.value, values);
        self.update_count_since_snapshot += 1;
        true
    }

    fn get(&self) -> &T {
        &self.value
    }

    fn consume(&mut self) -> bool {
        false
    }

    fn checkpoint(&self) -> Option<serde_json::Value> {
        serde_json::to_value(&self.value).ok()
    }

    fn from_checkpoint(value: serde_json::Value) -> Result<Self, String> {
        let value: T = serde_json::from_value(value)
            .map_err(|e| format!("checkpoint deserialization failed: {e}"))?;
        Ok(Self {
            value,
            snapshot_frequency: 10,
            update_count_since_snapshot: 0,
            _reducer: std::marker::PhantomData,
        })
    }
}

/// Delta blob for representing checkpoint state
///
/// A `DeltaBlob` represents the persisted state of a delta channel.
/// `Missing` indicates no checkpoint data is available.
/// `Snapshot` contains a full snapshot of the value.
#[derive(Clone, Debug)]
pub enum DeltaBlob<T>
where
    T: Clone + serde::Serialize + serde::de::DeserializeOwned,
{
    /// No checkpoint data available
    Missing,
    /// Full snapshot of the channel value
    Snapshot(T),
}

/// Ring buffer channel for append-heavy fields with bounded size
///
/// Wraps a `Vec<T>` with a maximum capacity. When new values are appended
/// and the capacity is exceeded, the oldest elements are removed. This
/// prevents unbounded memory growth for append-heavy fields like messages,
/// events, and logs.
///
/// # Examples
///
/// ```ignore
/// use juncture_core::state::channel::RingBufferChannel;
///
/// let mut ch = RingBufferChannel::new(Vec::new(), 100);
/// ch.update(vec!["msg1".to_string(), "msg2".to_string()]);
/// assert_eq!(ch.get().len(), 2);
/// ```
#[derive(Clone, Debug)]
pub struct RingBufferChannel<T> {
    /// Current values in the ring buffer
    values: Vec<T>,
    /// Maximum capacity of the ring buffer
    capacity: usize,
}

impl<T> RingBufferChannel<T> {
    /// Create a new ring buffer channel with the given initial values and capacity
    ///
    /// The capacity is clamped to a minimum of 1. If `values` exceeds the
    /// capacity, the oldest elements are trimmed.
    #[must_use]
    pub fn new(values: Vec<T>, capacity: usize) -> Self {
        let mut channel = Self {
            values,
            capacity: capacity.max(1),
        };
        channel.trim_to_capacity();
        channel
    }

    /// Get the current capacity
    #[must_use]
    pub const fn capacity(&self) -> usize {
        self.capacity
    }

    /// Get the current number of elements
    #[must_use]
    pub fn len(&self) -> usize {
        self.values.len()
    }

    /// Check if the ring buffer is empty
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// Trim the values to the configured capacity
    ///
    /// Removes the oldest elements (from the front) if the buffer exceeds capacity.
    fn trim_to_capacity(&mut self) {
        if self.values.len() > self.capacity {
            let excess = self.values.len() - self.capacity;
            self.values.drain(..excess);
        }
    }
}

impl<T: Send + Sync + serde::Serialize + serde::de::DeserializeOwned + 'static> Channel<Vec<T>>
    for RingBufferChannel<T>
{
    fn update(&mut self, values: Vec<Vec<T>>) -> bool {
        let had_non_empty = values.iter().any(|v| !v.is_empty());
        for v in values {
            self.values.extend(v);
        }
        self.trim_to_capacity();
        had_non_empty
    }

    fn get(&self) -> &Vec<T> {
        &self.values
    }

    fn consume(&mut self) -> bool {
        false
    }

    fn checkpoint(&self) -> Option<serde_json::Value> {
        serde_json::to_value(serde_json::json!({
            "values": &self.values,
            "capacity": self.capacity,
        }))
        .ok()
    }

    fn from_checkpoint(value: serde_json::Value) -> Result<Self, String> {
        // Support both legacy format (plain array) and new format (object with capacity)
        if let Some(obj) = value.as_object() {
            let values: Vec<T> =
                serde_json::from_value(obj.get("values").cloned().unwrap_or_default())
                    .map_err(|e| format!("checkpoint deserialization failed: {e}"))?;
            let capacity = obj
                .get("capacity")
                .and_then(serde_json::Value::as_u64)
                .map_or(1000, |c| usize::try_from(c).unwrap_or(1000))
                .max(1); // Enforce minimum capacity of 1 (same as new())
            Ok(Self { values, capacity })
        } else {
            // Legacy format: plain array without capacity
            let values: Vec<T> = serde_json::from_value(value)
                .map_err(|e| format!("checkpoint deserialization failed: {e}"))?;
            Ok(Self {
                values,
                capacity: 1000,
            })
        }
    }
}

/// Remove-message identifier for message deletion
///
/// Used to identify which message should be removed from the message list
/// during state updates.
#[derive(Clone, Debug)]
pub struct RemoveMessage {
    /// ID of the message to remove
    pub id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn untracked_channel_update_returns_true_on_change() {
        let mut ch: UntrackedChannel<i32, ReplaceReducer> = UntrackedChannel::new(0);
        assert!(!ch.update(vec![]));
        assert!(ch.update(vec![42]));
        assert_eq!(*ch.get(), 42);
    }

    #[test]
    fn untracked_channel_consume_always_false() {
        let mut ch: UntrackedChannel<i32, ReplaceReducer> = UntrackedChannel::new(1);
        assert!(!ch.consume());
    }

    #[test]
    fn untracked_channel_checkpoint_is_none() {
        let ch: UntrackedChannel<i32, ReplaceReducer> = UntrackedChannel::new(5);
        assert!(ch.checkpoint().is_none());
    }

    #[test]
    fn untracked_channel_from_checkpoint_uses_default() {
        let ch: UntrackedChannel<i32, ReplaceReducer> =
            UntrackedChannel::from_checkpoint(serde_json::json!(99)).expect("should succeed");
        assert_eq!(*ch.get(), 0);
    }

    #[test]
    fn ephemeral_channel_consume_tracks_state() {
        let mut ch: EphemeralChannel<i32, ReplaceReducer> = EphemeralChannel::new(0);
        assert!(!ch.consume()); // first consume returns false (was not consumed)
        assert!(ch.consume()); // second consume returns true (was consumed)
    }

    #[test]
    fn ephemeral_channel_update_resets_consumed() {
        let mut ch: EphemeralChannel<i32, ReplaceReducer> = EphemeralChannel::new(0);
        assert!(!ch.consume());
        assert!(ch.update(vec![7]));
        assert!(!ch.consume()); // consumed was reset by update
    }

    #[test]
    fn ephemeral_channel_checkpoint_is_none() {
        let ch: EphemeralChannel<i32, ReplaceReducer> = EphemeralChannel::new(3);
        assert!(ch.checkpoint().is_none());
    }

    #[test]
    fn last_value_after_finish_channel_not_available_before_finish() {
        let ch: LastValueAfterFinishChannel<i32, ReplaceReducer> =
            LastValueAfterFinishChannel::new(0);
        assert!(!ch.is_available());
    }

    #[test]
    fn last_value_after_finish_channel_available_after_finish() {
        let mut ch: LastValueAfterFinishChannel<i32, ReplaceReducer> =
            LastValueAfterFinishChannel::new(0);
        ch.finish();
        assert!(ch.is_available());
    }

    #[test]
    fn last_value_after_finish_channel_checkpoint_only_if_finished() {
        let ch: LastValueAfterFinishChannel<i32, ReplaceReducer> =
            LastValueAfterFinishChannel::new(5);
        assert!(ch.checkpoint().is_none());

        let mut ch2: LastValueAfterFinishChannel<i32, ReplaceReducer> =
            LastValueAfterFinishChannel::new(5);
        ch2.finish();
        assert!(ch2.checkpoint().is_some());
    }

    #[test]
    fn delta_channel_snapshot_frequency_clamped_to_one() {
        let ch: DeltaChannel<i32, ReplaceReducer> = DeltaChannel::new(0, 0);
        assert_eq!(ch.snapshot_frequency, 1);
    }

    #[test]
    fn delta_channel_replay_writes_restores_state() {
        let mut ch: DeltaChannel<Vec<i32>, AppendReducer> = DeltaChannel::new(vec![], 10);
        ch.replay_writes(&[vec![1, 2], vec![3, 4]]);
        assert_eq!(*ch.get(), vec![1, 2, 3, 4]);
        assert_eq!(ch.update_count_since_snapshot, 0);
    }

    #[test]
    fn delta_channel_checkpoint_returns_snapshot() {
        let ch: DeltaChannel<i32, ReplaceReducer> = DeltaChannel::new(42, 5);
        let cp = ch.checkpoint().expect("should have checkpoint");
        assert_eq!(cp, serde_json::json!(42));
    }

    #[test]
    fn delta_channel_should_snapshot() {
        let mut ch: DeltaChannel<i32, ReplaceReducer> = DeltaChannel::new(0, 2);
        assert!(!ch.should_snapshot());
        ch.update(vec![1]);
        assert!(!ch.should_snapshot());
        ch.update(vec![2]);
        assert!(ch.should_snapshot());
    }

    #[test]
    fn delta_blob_missing_variant_exists() {
        let blob: DeltaBlob<i32> = DeltaBlob::Missing;
        assert!(matches!(blob, DeltaBlob::Missing));
    }

    #[test]
    fn delta_blob_snapshot_holds_value() {
        let blob: DeltaBlob<i32> = DeltaBlob::Snapshot(42);
        assert!(matches!(blob, DeltaBlob::Snapshot(_)));
    }

    #[test]
    fn delta_blob_clone() {
        let blob: DeltaBlob<String> = DeltaBlob::Snapshot("hello".to_string());
        let cloned = blob.clone();
        if let DeltaBlob::Snapshot(v) = cloned {
            assert_eq!(v, "hello");
        } else {
            panic!("expected Snapshot variant");
        }
        // Use original to prove clone created independent copy
        if let DeltaBlob::Snapshot(v) = blob {
            assert_eq!(v, "hello");
        } else {
            panic!("expected Snapshot variant");
        }
    }

    #[test]
    fn remove_message_holds_id() {
        let rm = RemoveMessage {
            id: "msg-123".to_string(),
        };
        assert_eq!(rm.id, "msg-123");
    }

    #[test]
    fn overwrite_serialize_round_trip() {
        let original = Overwrite(42);
        let json = serde_json::to_string(&original).expect("should serialize");
        assert_eq!(json, r#"{"__overwrite__":42}"#);

        let deserialized: Overwrite<i32> = serde_json::from_str(&json).expect("should deserialize");
        assert_eq!(deserialized.0, 42);
    }

    #[test]
    fn overwrite_serialize_complex_type() {
        let original = Overwrite(vec![1, 2, 3]);
        let json = serde_json::to_string(&original).expect("should serialize");
        assert_eq!(json, r#"{"__overwrite__":[1,2,3]}"#);

        let deserialized: Overwrite<Vec<i32>> =
            serde_json::from_str(&json).expect("should deserialize");
        assert_eq!(deserialized.0, vec![1, 2, 3]);
    }

    #[test]
    fn overwrite_debug_format() {
        let ov = Overwrite(42);
        let debug_str = format!("{ov:?}");
        assert_eq!(debug_str, "Overwrite(42)");
    }

    #[test]
    fn replace_reducer_single_value_succeeds() {
        let mut val = 0;
        ReplaceReducer::reduce(&mut val, vec![42]);
        assert_eq!(val, 42);
    }

    #[test]
    fn replace_reducer_empty_values_succeeds() {
        let mut val = 99;
        ReplaceReducer::reduce(&mut val, vec![]);
        assert_eq!(val, 99);
    }

    #[test]
    #[should_panic(expected = "Replace reducer: multiple writes in same superstep")]
    fn replace_reducer_multiple_values_panics() {
        let mut val = 0;
        ReplaceReducer::reduce(&mut val, vec![1, 2]);
    }

    #[test]
    #[should_panic(expected = "Replace reducer: multiple writes in same superstep")]
    fn untracked_channel_multiple_writes_panics() {
        let mut ch: UntrackedChannel<i32, ReplaceReducer> = UntrackedChannel::new(0);
        ch.update(vec![1, 2]);
    }

    // NamedBarrierChannel tests
    #[test]
    fn named_barrier_channel_not_available_initially() {
        let ch: NamedBarrierChannel<i32, ReplaceReducer> = NamedBarrierChannel::new_with_sources(
            0,
            ["node_a", "node_b"].into_iter().map(String::from),
        );
        assert!(!ch.is_available());
    }

    #[test]
    fn named_barrier_channel_available_after_all_sources_write() {
        let mut ch: NamedBarrierChannel<i32, ReplaceReducer> =
            NamedBarrierChannel::new_with_sources(
                0,
                ["node_a", "node_b"].into_iter().map(String::from),
            );
        assert!(!ch.is_available());

        ch.update("node_a".to_string(), vec![42]);
        assert!(!ch.is_available());

        ch.update("node_b".to_string(), vec![100]);
        assert!(ch.is_available());
        assert_eq!(*ch.get(), 100); // Last write wins with ReplaceReducer
    }

    #[test]
    fn named_barrier_channel_empty_required_sources_is_available() {
        let ch: NamedBarrierChannel<i32, ReplaceReducer> = NamedBarrierChannel::new(42);
        assert!(ch.is_available());
    }

    #[test]
    fn named_barrier_channel_has_written_tracks_sources() {
        let mut ch: NamedBarrierChannel<i32, ReplaceReducer> =
            NamedBarrierChannel::new_with_sources(
                0,
                ["node_a", "node_b", "node_c"].into_iter().map(String::from),
            );

        assert!(!ch.has_written("node_a"));
        ch.update("node_a".to_string(), vec![1]);
        assert!(ch.has_written("node_a"));
        assert!(!ch.has_written("node_b"));
    }

    #[test]
    fn named_barrier_channel_reset_clears_seen_sources() {
        let mut ch: NamedBarrierChannel<i32, ReplaceReducer> =
            NamedBarrierChannel::new_with_sources(
                0,
                ["node_a", "node_b"].into_iter().map(String::from),
            );

        ch.update("node_a".to_string(), vec![1]);
        ch.update("node_b".to_string(), vec![2]);
        assert!(ch.is_available());

        ch.reset();
        assert!(!ch.is_available());
        assert!(!ch.has_written("node_a"));
        assert!(!ch.has_written("node_b"));
    }

    #[test]
    fn named_barrier_channel_add_required_source() {
        let mut ch: NamedBarrierChannel<i32, ReplaceReducer> = NamedBarrierChannel::new(0);
        assert!(ch.is_available());

        ch.add_required_source("node_a".to_string());
        assert!(!ch.is_available());

        ch.update("node_a".to_string(), vec![42]);
        assert!(ch.is_available());
    }

    #[test]
    #[should_panic(expected = "NamedBarrierChannel: source")]
    fn named_barrier_channel_unknown_source_panics() {
        let mut ch: NamedBarrierChannel<i32, ReplaceReducer> =
            NamedBarrierChannel::new_with_sources(0, vec!["node_a".to_string()]);

        ch.update("unknown_node".to_string(), vec![42]);
    }

    #[test]
    fn named_barrier_channel_checkpoint_persists_state() {
        let mut ch: NamedBarrierChannel<i32, ReplaceReducer> =
            NamedBarrierChannel::new_with_sources(0, vec!["node_a".to_string()]);

        ch.update("node_a".to_string(), vec![42]);

        let checkpoint = ch.checkpoint().expect("should have checkpoint");
        // Checkpoint is a tuple (value, seen_sources)
        assert!(checkpoint.is_array() || checkpoint.is_object());

        let restored: NamedBarrierChannel<i32, ReplaceReducer> =
            NamedBarrierChannel::from_checkpoint(checkpoint).expect("should restore");
        assert_eq!(*restored.get(), 42);
        assert!(restored.has_written("node_a"));
    }

    #[test]
    fn named_barrier_channel_generic_update_marks_all_sources_seen() {
        let mut ch: NamedBarrierChannel<i32, ReplaceReducer> =
            NamedBarrierChannel::new_with_sources(0, ["node_a".to_string(), "node_b".to_string()]);

        // Using the generic Channel trait update
        Channel::update(&mut ch, vec![42]);
        assert!(ch.is_available());
        assert!(ch.has_written("node_a"));
        assert!(ch.has_written("node_b"));
    }

    // TopicChannel tests
    #[test]
    fn topic_channel_new_is_empty() {
        let ch: TopicChannel<String> = TopicChannel::new();
        assert!(ch.is_empty());
        assert_eq!(ch.len(), 0);
    }

    #[test]
    fn topic_channel_default_is_empty() {
        let ch: TopicChannel<String> = TopicChannel::default();
        assert!(ch.is_empty());
    }

    #[test]
    fn topic_channel_accumulates_messages() {
        let mut ch: TopicChannel<String> = TopicChannel::new();

        ch.update(vec![vec!["hello".to_string()]]);
        assert_eq!(ch.len(), 1);
        assert_eq!(ch.get()[0], "hello");

        ch.update(vec![vec!["world".to_string()]]);
        assert_eq!(ch.len(), 2);
        assert_eq!(ch.get()[1], "world");
    }

    #[test]
    fn topic_channel_update_with_multiple_messages() {
        let mut ch: TopicChannel<i32> = TopicChannel::new();

        ch.update(vec![vec![1, 2, 3]]);
        assert_eq!(ch.len(), 3);
        assert_eq!(ch.get(), &[1, 2, 3]);
    }

    #[test]
    fn topic_channel_update_with_multiple_batches() {
        let mut ch: TopicChannel<i32> = TopicChannel::new();

        ch.update(vec![vec![1, 2], vec![3, 4]]);
        assert_eq!(ch.len(), 4);
        assert_eq!(ch.get(), &[1, 2, 3, 4]);
    }

    #[test]
    fn topic_channel_reset_clears_messages() {
        let mut ch: TopicChannel<String> = TopicChannel::new();

        ch.update(vec![vec!["test".to_string()]]);
        assert_eq!(ch.len(), 1);

        ch.reset();
        assert!(ch.is_empty());
        assert_eq!(ch.len(), 0);
    }

    #[test]
    fn topic_channel_consume_clears_and_returns_status() {
        let mut ch: TopicChannel<String> = TopicChannel::new();

        let had_content = ch.consume();
        assert!(!had_content); // Empty channel returns false

        ch.update(vec![vec!["test".to_string()]]);
        let had_content_after = ch.consume();
        assert!(had_content_after); // Non-empty channel returns true
        assert!(ch.is_empty());
    }

    #[test]
    fn topic_channel_iter_messages() {
        let mut ch: TopicChannel<i32> = TopicChannel::new();

        ch.update(vec![vec![1, 2, 3]]);

        let mut iter = ch.iter();
        assert_eq!(iter.next(), Some(&1));
        assert_eq!(iter.next(), Some(&2));
        assert_eq!(iter.next(), Some(&3));
        assert_eq!(iter.next(), None);
    }

    #[test]
    fn topic_channel_checkpoint_persists_messages() {
        let mut ch: TopicChannel<i32> = TopicChannel::new();

        ch.update(vec![vec![1, 2, 3]]);

        let checkpoint = ch.checkpoint().expect("should have checkpoint");
        assert_eq!(checkpoint, serde_json::json!([1, 2, 3]));

        let restored: TopicChannel<i32> =
            TopicChannel::from_checkpoint(checkpoint).expect("should restore");
        assert_eq!(restored.len(), 3);
        assert_eq!(restored.get(), &[1, 2, 3]);
    }

    #[test]
    fn topic_channel_from_checkpoint_empty() {
        let ch: TopicChannel<i32> =
            TopicChannel::from_checkpoint(serde_json::json!([])).expect("should restore");
        assert!(ch.is_empty());
    }

    // Tests for LastValueAfterFinishChannel checkpoint fix (Task 2)
    #[test]
    fn last_value_after_finish_checkpoint_saves_is_finished_state() {
        let mut ch: LastValueAfterFinishChannel<i32, ReplaceReducer> =
            LastValueAfterFinishChannel::new(10);
        ch.update(vec![42]);
        ch.finish();

        let checkpoint = ch
            .checkpoint()
            .expect("should have checkpoint when finished");
        // Checkpoint should be a tuple (value, is_finished)
        assert!(checkpoint.is_array());
        let arr = checkpoint.as_array().expect("should be array");
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0], serde_json::json!(42)); // value
        assert_eq!(arr[1], serde_json::json!(true)); // is_finished
    }

    #[test]
    fn last_value_after_finish_from_checkpoint_restores_is_finished() {
        let checkpoint_data = serde_json::json!([99, true]); // (value, is_finished)

        let restored: LastValueAfterFinishChannel<i32, ReplaceReducer> =
            LastValueAfterFinishChannel::from_checkpoint(checkpoint_data)
                .expect("should restore from checkpoint");

        assert_eq!(*restored.get(), 99);
        assert!(restored.is_available());
    }

    #[test]
    fn last_value_after_finish_from_checkpoint_old_format_backward_compat() {
        // Old format: just the value, no is_finished
        let checkpoint_data = serde_json::json!(55);

        let restored: LastValueAfterFinishChannel<i32, ReplaceReducer> =
            LastValueAfterFinishChannel::from_checkpoint(checkpoint_data)
                .expect("should restore from old checkpoint format");

        assert_eq!(*restored.get(), 55);
        assert!(!restored.is_available()); // Should default to not finished
    }

    #[test]
    fn last_value_after_finish_checkpoint_round_trip() {
        let mut ch1: LastValueAfterFinishChannel<i32, ReplaceReducer> =
            LastValueAfterFinishChannel::new(0);
        ch1.update(vec![123]);
        ch1.finish();

        let checkpoint = ch1.checkpoint().expect("should checkpoint");
        let ch2: LastValueAfterFinishChannel<i32, ReplaceReducer> =
            LastValueAfterFinishChannel::from_checkpoint(checkpoint).expect("should restore");

        assert_eq!(*ch1.get(), *ch2.get());
        assert_eq!(ch1.is_available(), ch2.is_available());
    }

    // Tests for Overwrite helper methods
    #[test]
    fn overwrite_get_returns_inner_value() {
        let ov = Overwrite(42);
        assert_eq!(*ov.get(), 42);
    }

    #[test]
    fn overwrite_into_inner_consumes_wrapper() {
        let ov = Overwrite(100);
        assert_eq!(ov.into_inner(), 100);
    }

    #[test]
    fn overwrite_new_creates_wrapper() {
        let ov = Overwrite::new(999);
        assert_eq!(*ov.get(), 999);
    }

    // Tests for DeltaChannel replay_writes with Overwrite detection (Task 1)
    #[test]
    fn delta_channel_replay_writes_handles_empty_sequence() {
        let mut ch: DeltaChannel<i32, ReplaceReducer> = DeltaChannel::new(5, 10);
        ch.replay_writes(&[]);
        assert_eq!(*ch.get(), 5); // Value unchanged
    }

    #[test]
    fn delta_channel_replay_writes_single_value() {
        let mut ch: DeltaChannel<i32, ReplaceReducer> = DeltaChannel::new(0, 10);
        ch.replay_writes(&[42]);
        assert_eq!(*ch.get(), 42);
    }

    #[test]
    fn delta_channel_replay_writes_multiple_values() {
        let mut ch: DeltaChannel<Vec<i32>, AppendReducer> = DeltaChannel::new(vec![], 10);
        ch.replay_writes(&[vec![1, 2], vec![3, 4]]);
        assert_eq!(*ch.get(), vec![1, 2, 3, 4]);
    }

    #[test]
    fn delta_channel_replay_writes_resets_snapshot_counter() {
        let mut ch: DeltaChannel<i32, ReplaceReducer> = DeltaChannel::new(0, 10);
        ch.update(vec![1]);
        assert_eq!(ch.update_count_since_snapshot, 1);

        ch.replay_writes(&[99]);
        assert_eq!(ch.update_count_since_snapshot, 0); // Reset after replay
    }

    #[test]
    fn delta_channel_replay_writes_with_replace_reducer() {
        let mut ch: DeltaChannel<i32, ReplaceReducer> = DeltaChannel::new(0, 10);
        // ReplaceReducer only allows one value
        ch.replay_writes(&[42]);
        assert_eq!(*ch.get(), 42);
    }

    #[test]
    fn delta_channel_replay_writes_detects_json_overwrite_format() {
        let mut ch: DeltaChannel<serde_json::Value, LastWriteWinsReducer> =
            DeltaChannel::new(serde_json::json!(null), 10);

        // Create values in Overwrite format (as they would appear in checkpoints)
        let overwrite_val = serde_json::json!({"__overwrite__": "baseline"});
        let normal_val1 = serde_json::json!("update1");
        let normal_val2 = serde_json::json!("update2");

        ch.replay_writes(&[normal_val1, overwrite_val, normal_val2.clone()]);

        // After detecting the overwrite, baseline should be "baseline",
        // then remaining values ["update1", "update2"] applied via LastWriteWinsReducer
        // LastWriteWinsReducer takes the last value
        assert_eq!(ch.get(), &normal_val2);
    }

    #[test]
    fn delta_channel_replay_writes_overwrite_at_start() {
        let mut ch: DeltaChannel<serde_json::Value, LastWriteWinsReducer> =
            DeltaChannel::new(serde_json::json!("initial"), 10);

        let overwrite_val = serde_json::json!({"__overwrite__": "new_baseline"});
        let normal_val = serde_json::json!("update");

        ch.replay_writes(&[overwrite_val, normal_val.clone()]);

        // Overwrite sets baseline to "new_baseline", then "update" applied
        assert_eq!(ch.get(), &normal_val);
    }

    #[test]
    fn delta_channel_replay_writes_overwrite_at_end() {
        let mut ch: DeltaChannel<serde_json::Value, LastWriteWinsReducer> =
            DeltaChannel::new(serde_json::json!("initial"), 10);

        let normal_val = serde_json::json!("update");
        let overwrite_val = serde_json::json!({"__overwrite__": "final_baseline"});

        ch.replay_writes(&[normal_val, overwrite_val]);

        // Overwrite at end sets baseline to "final_baseline", no remaining values
        assert_eq!(ch.get(), &serde_json::json!("final_baseline"));
    }

    #[test]
    fn delta_channel_finish_forces_snapshot() {
        let mut ch: DeltaChannel<i32, ReplaceReducer> = DeltaChannel::new(0, 10);
        assert!(!ch.should_snapshot());

        ch.finish();
        // After finish, should_snapshot should return true
        assert!(ch.should_snapshot());
    }

    // --- RingBufferChannel tests ---

    #[test]
    fn ring_buffer_channel_new_enforces_capacity() {
        let ch: RingBufferChannel<i32> = RingBufferChannel::new(vec![1, 2, 3, 4, 5], 3);
        assert_eq!(ch.capacity(), 3);
        assert_eq!(ch.len(), 3);
        assert_eq!(ch.get(), &vec![3, 4, 5]);
    }

    #[test]
    fn ring_buffer_channel_new_clamps_min_capacity() {
        let ch: RingBufferChannel<i32> = RingBufferChannel::new(vec![1, 2], 0);
        assert_eq!(ch.capacity(), 1);
    }

    #[test]
    fn ring_buffer_channel_update_appends_and_trims() {
        let mut ch: RingBufferChannel<i32> = RingBufferChannel::new(vec![], 3);
        assert!(ch.update(vec![vec![1, 2]]));
        assert_eq!(ch.get(), &vec![1, 2]);

        assert!(ch.update(vec![vec![3, 4]]));
        assert_eq!(ch.get(), &vec![2, 3, 4]);
        assert_eq!(ch.len(), 3);
    }

    #[test]
    fn ring_buffer_channel_update_returns_false_for_empty() {
        let mut ch: RingBufferChannel<i32> = RingBufferChannel::new(vec![1, 2], 5);
        assert!(!ch.update(vec![vec![]]));
        assert_eq!(ch.get(), &vec![1, 2]);
    }

    #[test]
    fn ring_buffer_channel_update_returns_false_for_empty_outer() {
        let mut ch: RingBufferChannel<i32> = RingBufferChannel::new(vec![1, 2], 5);
        assert!(!ch.update(vec![]));
        assert_eq!(ch.get(), &vec![1, 2]);
    }

    #[test]
    fn ring_buffer_channel_consume_always_false() {
        let mut ch: RingBufferChannel<i32> = RingBufferChannel::new(vec![1], 5);
        assert!(!ch.consume());
    }

    #[test]
    fn ring_buffer_channel_checkpoint_roundtrip() {
        let ch: RingBufferChannel<i32> = RingBufferChannel::new(vec![1, 2, 3], 10);
        let checkpoint = ch.checkpoint().unwrap();
        let restored = RingBufferChannel::<i32>::from_checkpoint(checkpoint).unwrap();
        assert_eq!(restored.get(), &vec![1, 2, 3]);
        assert_eq!(restored.capacity(), 10);
    }

    #[test]
    fn ring_buffer_channel_from_checkpoint_legacy_format() {
        // Legacy format: plain array without capacity
        let legacy = serde_json::json!([1, 2, 3]);
        let ch = RingBufferChannel::<i32>::from_checkpoint(legacy).unwrap();
        assert_eq!(ch.get(), &vec![1, 2, 3]);
        assert_eq!(ch.capacity(), 1000); // default capacity
    }

    #[test]
    fn ring_buffer_channel_from_checkpoint_clamps_capacity() {
        // Checkpoint with capacity 0 should be clamped to 1
        let checkpoint = serde_json::json!({"values": [1, 2], "capacity": 0});
        let ch = RingBufferChannel::<i32>::from_checkpoint(checkpoint).unwrap();
        assert_eq!(ch.capacity(), 1);
    }

    #[test]
    fn ring_buffer_channel_is_empty() {
        let ch: RingBufferChannel<i32> = RingBufferChannel::new(vec![], 5);
        assert!(ch.is_empty());

        let ch2: RingBufferChannel<i32> = RingBufferChannel::new(vec![1], 5);
        assert!(!ch2.is_empty());
    }
}

// Rust guideline compliant 2026-05-20
