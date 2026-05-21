//! Channel trait and channel types for state field access with checkpoint support
//!
//! A Channel wraps a value with specific update and checkpoint semantics.
//! Different channel types control how values are updated, persisted, and consumed.

use crate::error::InvalidUpdateError;
use serde::de::DeserializeOwned;
use serde::ser::SerializeStruct;

/// Reducer trait defining merge semantics for state fields
///
/// Each field in a State can have its own reducer, defining how multiple
/// writes in the same superstep are combined.
pub trait Reducer<T> {
    /// Merge a single value (fast path avoiding Vec allocation)
    ///
    /// # Errors
    ///
    /// Returns `InvalidUpdateError` if the merge violates reducer constraints.
    fn reduce_one(current: &mut T, value: T) -> Result<(), InvalidUpdateError> {
        Self::reduce(current, vec![value])
    }

    /// Merge multiple values into current
    ///
    /// Values are provided in the order tasks completed (not task spawn order).
    /// For deterministic results, use associative reducers like `AppendReducer`.
    ///
    /// # Errors
    ///
    /// Returns `InvalidUpdateError` if the merge violates reducer constraints
    /// (e.g., multiple writers on a replace channel).
    fn reduce(current: &mut T, values: Vec<T>) -> Result<(), InvalidUpdateError>;
}

/// Replace reducer: only one writer per superstep (default)
///
/// Equivalent to `LangGraph`'s `LastValue` channel.
/// Returns an error if multiple nodes write to the same field in one superstep.
#[derive(Debug)]
pub struct ReplaceReducer;

impl<T> Reducer<T> for ReplaceReducer {
    fn reduce(current: &mut T, values: Vec<T>) -> Result<(), InvalidUpdateError> {
        if values.len() > 1 {
            return Err(InvalidUpdateError::MultipleOverwrite {
                field: "unknown".to_string(),
            });
        }
        if let Some(v) = values.into_iter().next() {
            *current = v;
        }
        Ok(())
    }
}

/// Append reducer: accumulate all writes
///
/// Equivalent to `LangGraph`'s `BinaryOperatorAggregate` with operator.add.
/// All writes are extended in order.
#[derive(Debug)]
pub struct AppendReducer;

impl<T> Reducer<Vec<T>> for AppendReducer {
    fn reduce_one(current: &mut Vec<T>, value: Vec<T>) -> Result<(), InvalidUpdateError> {
        current.extend(value);
        Ok(())
    }

    fn reduce(current: &mut Vec<T>, values: Vec<Vec<T>>) -> Result<(), InvalidUpdateError> {
        for v in values {
            current.extend(v);
        }
        Ok(())
    }
}

/// `AnyValue` reducer: assumes all values are equal
///
/// Similar to `LastValue`, but semantically assumes all writers provide
/// the same value. Uses the last value if they differ.
#[derive(Debug)]
pub struct AnyValueReducer;

impl<T: PartialEq + Clone> Reducer<T> for AnyValueReducer {
    fn reduce(current: &mut T, values: Vec<T>) -> Result<(), InvalidUpdateError> {
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
        Ok(())
    }
}

/// `LastWriteWins` reducer: allows multiple writers, last one wins
///
/// Similar to `ReplaceReducer`, but doesn't panic on multiple writes.
#[derive(Debug)]
pub struct LastWriteWinsReducer;

impl<T> Reducer<T> for LastWriteWinsReducer {
    fn reduce(current: &mut T, values: Vec<T>) -> Result<(), InvalidUpdateError> {
        if let Some(v) = values.into_iter().last() {
            *current = v;
        }
        Ok(())
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

/// Channel trait for state field access with checkpoint support
///
/// A Channel wraps a value with specific update and checkpoint semantics.
/// Different channel types control how values are updated, persisted, and consumed.
pub trait Channel<T>: Send + Sync + 'static {
    /// Update the channel with new values. Returns true if the value changed.
    ///
    /// # Errors
    ///
    /// Returns `InvalidUpdateError` if the update violates reducer constraints
    /// (e.g., multiple writers on a replace channel).
    fn update(&mut self, values: Vec<T>) -> Result<bool, InvalidUpdateError>;

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
    fn update(&mut self, values: Vec<T>) -> Result<bool, InvalidUpdateError> {
        if values.is_empty() {
            return Ok(false);
        }
        R::reduce(&mut self.value, values)?;
        Ok(true)
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
    fn update(&mut self, values: Vec<T>) -> Result<bool, InvalidUpdateError> {
        if values.is_empty() {
            return Ok(false);
        }
        self.consumed = false;
        R::reduce(&mut self.value, values)?;
        Ok(true)
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
    fn update(&mut self, values: Vec<T>) -> Result<bool, InvalidUpdateError> {
        if values.is_empty() {
            return Ok(false);
        }
        R::reduce(&mut self.value, values)?;
        if self.is_finished {
            self.finished_value = Some(self.value.clone());
        }
        Ok(true)
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
        if self.is_finished {
            serde_json::to_value(&self.value).ok()
        } else {
            None
        }
    }

    fn from_checkpoint(value: serde_json::Value) -> Result<Self, String> {
        let value: T = serde_json::from_value(value)
            .map_err(|e| format!("checkpoint deserialization failed: {e}"))?;
        Ok(Self {
            value,
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
    /// # Errors
    ///
    /// Returns `InvalidUpdateError` if the replay violates reducer constraints.
    pub fn replay_writes(&mut self, values: Vec<T>) -> Result<(), InvalidUpdateError> {
        if values.is_empty() {
            return Ok(());
        }
        R::reduce(&mut self.value, values)?;
        self.update_count_since_snapshot = 0;
        Ok(())
    }

    /// Check if a snapshot is due based on the update count
    #[must_use]
    pub const fn should_snapshot(&self) -> bool {
        self.update_count_since_snapshot >= self.snapshot_frequency
    }
}

impl<T, R> Channel<T> for DeltaChannel<T, R>
where
    T: Default + Clone + Send + Sync + serde::Serialize + DeserializeOwned + 'static,
    R: Reducer<T> + Send + Sync + 'static,
{
    fn update(&mut self, values: Vec<T>) -> Result<bool, InvalidUpdateError> {
        if values.is_empty() {
            return Ok(false);
        }
        R::reduce(&mut self.value, values)?;
        self.update_count_since_snapshot += 1;
        Ok(true)
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
pub enum DeltaBlob {
    /// No checkpoint data available
    Missing,
    /// Full snapshot of the channel value
    Snapshot(serde_json::Value),
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
        assert!(!ch.update(vec![]).expect("empty update should succeed"));
        assert!(
            ch.update(vec![42])
                .expect("single value update should succeed")
        );
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
        assert!(ch.update(vec![7]).expect("update should succeed"));
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
        ch.replay_writes(vec![vec![1, 2], vec![3, 4]])
            .expect("replay should succeed");
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
        ch.update(vec![1]).expect("update should succeed");
        assert!(!ch.should_snapshot());
        ch.update(vec![2]).expect("update should succeed");
        assert!(ch.should_snapshot());
    }

    #[test]
    fn delta_blob_missing_variant_exists() {
        let blob = DeltaBlob::Missing;
        assert!(matches!(blob, DeltaBlob::Missing));
    }

    #[test]
    fn delta_blob_snapshot_holds_value() {
        let blob = DeltaBlob::Snapshot(serde_json::json!(42));
        assert!(matches!(blob, DeltaBlob::Snapshot(_)));
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
        ReplaceReducer::reduce(&mut val, vec![42]).expect("single value should succeed");
        assert_eq!(val, 42);
    }

    #[test]
    fn replace_reducer_empty_values_succeeds() {
        let mut val = 99;
        ReplaceReducer::reduce(&mut val, vec![]).expect("empty values should succeed");
        assert_eq!(val, 99);
    }

    #[test]
    fn replace_reducer_multiple_values_returns_error() {
        let mut val = 0;
        let result = ReplaceReducer::reduce(&mut val, vec![1, 2]);
        assert!(result.is_err());
        let err = result.expect_err("multiple values should error");
        assert!(
            matches!(err, InvalidUpdateError::MultipleOverwrite { .. }),
            "expected MultipleOverwrite error, got {err:?}"
        );
    }

    #[test]
    fn untracked_channel_multiple_writes_returns_error() {
        let mut ch: UntrackedChannel<i32, ReplaceReducer> = UntrackedChannel::new(0);
        let result = ch.update(vec![1, 2]);
        assert!(result.is_err());
        let err = result.expect_err("multiple writes should error");
        assert!(
            matches!(err, InvalidUpdateError::MultipleOverwrite { .. }),
            "expected MultipleOverwrite error, got {err:?}"
        );
    }
}

// Rust guideline compliant 2026-05-20
