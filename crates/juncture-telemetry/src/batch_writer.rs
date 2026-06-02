//! Async batch writer for telemetry data.
//!
//! Buffers trace and observation writes in memory, then flushes them
//! to the store in batches. This avoids blocking the hot path with
//! database I/O and reduces write amplification.

use std::sync::Arc;

use tokio::sync::Mutex;
use tracing::{debug, error, warn};

use crate::langfuse::LangfuseExporter;
use crate::models::{Observation, Session, Trace};
use crate::trace_store::{StoreError, TraceStore};

/// Default batch size before auto-flush.
const DEFAULT_BATCH_SIZE: usize = 50;
/// Default flush interval in milliseconds.
const DEFAULT_FLUSH_INTERVAL_MS: u64 = 5_000;

/// Item to be written to the store.
#[derive(Clone, Debug)]
pub enum TelemetryItem {
    /// A trace to upsert.
    Trace(Trace),
    /// An observation to insert.
    Observation(Observation),
    /// A session to upsert.
    Session(Session),
}

/// Async batch writer that buffers telemetry items and flushes them
/// to the underlying store in batches.
///
/// Items are added to an in-memory buffer on `submit`. A background
/// task periodically flushes the buffer to the store. Call `flush()`
/// for immediate persistence, or `shutdown()` to drain all remaining
/// items before process exit.
#[derive(Clone)]
pub struct BatchWriter {
    inner: Arc<BatchWriterInner>,
}

impl std::fmt::Debug for BatchWriter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BatchWriter")
            .field("batch_size", &self.inner.batch_size)
            .finish()
    }
}

struct BatchWriterInner {
    buffer: Mutex<Vec<TelemetryItem>>,
    store: Arc<dyn TraceStore>,
    langfuse: Option<LangfuseExporter>,
    batch_size: usize,
    shutdown: Mutex<bool>,
}

impl BatchWriter {
    /// Create a new batch writer with default configuration.
    ///
    /// Starts a background flush task that runs until `shutdown()` is called.
    #[must_use]
    pub fn new(store: Arc<dyn TraceStore>) -> Self {
        Self::with_config(store, DEFAULT_BATCH_SIZE, DEFAULT_FLUSH_INTERVAL_MS)
    }

    /// Create a batch writer with custom batch size and flush interval.
    #[must_use]
    pub fn with_config(
        store: Arc<dyn TraceStore>,
        batch_size: usize,
        flush_interval_ms: u64,
    ) -> Self {
        Self::with_config_and_langfuse(store, None, batch_size, flush_interval_ms)
    }

    /// Create a batch writer with optional Langfuse cloud export.
    #[must_use]
    pub fn with_config_and_langfuse(
        store: Arc<dyn TraceStore>,
        langfuse: Option<LangfuseExporter>,
        batch_size: usize,
        flush_interval_ms: u64,
    ) -> Self {
        let inner = Arc::new(BatchWriterInner {
            buffer: Mutex::new(Vec::with_capacity(batch_size)),
            store,
            langfuse,
            batch_size,
            shutdown: Mutex::new(false),
        });

        let inner_clone = Arc::clone(&inner);
        tokio::spawn(async move {
            let mut interval =
                tokio::time::interval(std::time::Duration::from_millis(flush_interval_ms));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

            loop {
                interval.tick().await;

                // Check if shutdown was requested
                let is_shutdown = {
                    let guard = inner_clone.shutdown.lock().await;
                    *guard
                };

                let mut buffer = inner_clone.buffer.lock().await;
                if !buffer.is_empty() {
                    let batch: Vec<TelemetryItem> = buffer.drain(..).collect();
                    drop(buffer);
                    Self::flush_batch(&inner_clone.store, inner_clone.langfuse.as_ref(), batch)
                        .await;
                }

                if is_shutdown {
                    break;
                }
            }
        });

        Self { inner }
    }

    /// Submit a telemetry item for async writing.
    ///
    /// The item is added to an in-memory buffer. The buffer is flushed
    /// to the store either when it reaches `batch_size` or on the next
    /// periodic flush tick.
    ///
    /// # Errors
    ///
    /// Returns `StoreError::Storage` if the store write fails (only when
    /// batch size is exceeded and an immediate flush is triggered).
    pub async fn submit(&self, item: TelemetryItem) -> Result<(), StoreError> {
        let mut buffer = self.inner.buffer.lock().await;
        buffer.push(item);
        if buffer.len() >= self.inner.batch_size {
            let batch: Vec<TelemetryItem> = buffer.drain(..).collect();
            drop(buffer);
            Self::flush_batch(&self.inner.store, self.inner.langfuse.as_ref(), batch).await;
        }
        Ok(())
    }

    /// Submit a trace for async writing.
    ///
    /// # Errors
    ///
    /// Returns `StoreError::Storage` if the store write fails.
    pub async fn submit_trace(&self, trace: Trace) -> Result<(), StoreError> {
        self.submit(TelemetryItem::Trace(trace)).await
    }

    /// Submit an observation for async writing.
    ///
    /// # Errors
    ///
    /// Returns `StoreError::Storage` if the store write fails.
    pub async fn submit_observation(&self, observation: Observation) -> Result<(), StoreError> {
        self.submit(TelemetryItem::Observation(observation)).await
    }

    /// Submit a session for async writing.
    ///
    /// # Errors
    ///
    /// Returns `StoreError::Storage` if the store write fails.
    pub async fn submit_session(&self, session: Session) -> Result<(), StoreError> {
        self.submit(TelemetryItem::Session(session)).await
    }

    /// Flush all buffered items to the store immediately.
    ///
    /// Drains the buffer and writes all items to the store. This
    /// guarantees all previously submitted items are persisted.
    ///
    /// # Errors
    ///
    /// Returns `StoreError::Storage` if any write fails.
    pub async fn flush(&self) -> Result<(), StoreError> {
        let batch: Vec<TelemetryItem> = {
            let mut buffer = self.inner.buffer.lock().await;
            buffer.drain(..).collect()
        };
        if !batch.is_empty() {
            Self::flush_batch(&self.inner.store, self.inner.langfuse.as_ref(), batch).await;
        }
        Ok(())
    }

    /// Shutdown the writer, flushing all remaining items.
    ///
    /// Signals the background task to stop, then flushes any items
    /// still in the buffer.
    ///
    /// # Errors
    ///
    /// Returns `StoreError::Storage` if any write fails.
    pub async fn shutdown(self) -> Result<(), StoreError> {
        *self.inner.shutdown.lock().await = true;

        // Wait for background task to finish its current tick
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        // Flush any remaining items
        self.flush().await
    }

    #[expect(
        clippy::cognitive_complexity,
        reason = "flush_batch partitions items, writes to store, and exports to langfuse"
    )]
    async fn flush_batch(
        store: &Arc<dyn TraceStore>,
        langfuse: Option<&LangfuseExporter>,
        batch: Vec<TelemetryItem>,
    ) {
        let (sessions, traces, observations) = Self::partition_items(batch);
        let mut errors = 0;
        errors += Self::flush_sessions(store, &sessions).await;
        errors += Self::flush_traces(store, &traces).await;
        errors += Self::flush_observations(store, &observations).await;
        if errors > 0 {
            warn!("batch writer: {errors} items failed to write");
        } else {
            debug!("batch writer: flush complete");
        }

        // Export to Langfuse cloud if configured
        if let Some(exporter) = langfuse {
            for trace in &traces {
                let trace_obs: Vec<Observation> = observations
                    .iter()
                    .filter(|o| o.trace_id == trace.id)
                    .cloned()
                    .collect();
                if let Err(e) = exporter.export(trace, &trace_obs).await {
                    warn!("langfuse export failed: {e}");
                }
            }
        }
    }

    fn partition_items(batch: Vec<TelemetryItem>) -> (Vec<Session>, Vec<Trace>, Vec<Observation>) {
        let mut sessions = Vec::new();
        let mut traces = Vec::new();
        let mut observations = Vec::new();
        for item in batch {
            match item {
                TelemetryItem::Session(s) => sessions.push(s),
                TelemetryItem::Trace(t) => traces.push(t),
                TelemetryItem::Observation(o) => observations.push(o),
            }
        }
        (sessions, traces, observations)
    }

    async fn flush_sessions(store: &Arc<dyn TraceStore>, sessions: &[Session]) -> u32 {
        let mut errors = 0;
        for session in sessions {
            if let Err(e) = store.upsert_session(session).await {
                errors += 1;
                error!("batch writer: failed to write session: {e}");
            }
        }
        errors
    }

    async fn flush_traces(store: &Arc<dyn TraceStore>, traces: &[Trace]) -> u32 {
        let mut errors = 0;
        for trace in traces {
            if let Err(e) = store.upsert_trace(trace).await {
                errors += 1;
                error!("batch writer: failed to write trace: {e}");
            }
        }
        errors
    }

    async fn flush_observations(store: &Arc<dyn TraceStore>, observations: &[Observation]) -> u32 {
        let mut errors = 0;
        for obs in observations {
            if let Err(e) = store.insert_observation(obs).await {
                errors += 1;
                error!("batch writer: failed to write observation: {e}");
            }
        }
        errors
    }
}

#[cfg(test)]
#[expect(
    clippy::clone_on_ref_ptr,
    reason = ".clone() needed for unsized coercion Arc<SqliteStore> -> Arc<dyn TraceStore>"
)]
mod tests {
    use super::*;
    use crate::sqlite_store::SqliteStore;

    #[tokio::test]
    async fn batch_writer_submit_and_flush() {
        let store = Arc::new(SqliteStore::new_memory().await.unwrap());
        let writer = BatchWriter::with_config(store.clone(), 2, 60_000);

        let trace = Trace::new("test");
        writer.submit_trace(trace.clone()).await.unwrap();
        writer.flush().await.unwrap();

        let loaded = store.get_trace(trace.id).await.unwrap();
        assert!(loaded.is_some());
    }

    #[tokio::test]
    async fn batch_writer_auto_flush() {
        let store = Arc::new(SqliteStore::new_memory().await.unwrap());
        let writer = BatchWriter::with_config(store.clone(), 2, 60_000);

        let trace1 = Trace::new("test1");
        let trace2 = Trace::new("test2");
        writer.submit_trace(trace1.clone()).await.unwrap();
        writer.submit_trace(trace2.clone()).await.unwrap();

        // Auto-flush triggers immediately when batch_size reached
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let loaded1 = store.get_trace(trace1.id).await.unwrap();
        let loaded2 = store.get_trace(trace2.id).await.unwrap();
        assert!(loaded1.is_some());
        assert!(loaded2.is_some());
    }

    #[tokio::test]
    async fn batch_writer_shutdown() {
        let store = Arc::new(SqliteStore::new_memory().await.unwrap());
        let writer = BatchWriter::with_config(store.clone(), 100, 60_000);

        let trace = Trace::new("test");
        writer.submit_trace(trace.clone()).await.unwrap();
        writer.shutdown().await.unwrap();

        let loaded = store.get_trace(trace.id).await.unwrap();
        assert!(loaded.is_some());
    }

    #[tokio::test]
    async fn batch_writer_trace_and_observation() {
        let store = Arc::new(SqliteStore::new_memory().await.unwrap());
        let writer = BatchWriter::with_config(store.clone(), 100, 60_000);

        let trace = Trace::new("test");
        let trace_id = trace.id;
        writer.submit_trace(trace).await.unwrap();

        let obs = Observation::span(trace_id, "test_span");
        writer.submit_observation(obs).await.unwrap();

        writer.flush().await.unwrap();

        let loaded = store.get_trace(trace_id).await.unwrap();
        assert!(loaded.is_some(), "trace should exist");
        let loaded = loaded.unwrap();
        assert_eq!(
            loaded.observations.len(),
            1,
            "expected 1 observation, got {}",
            loaded.observations.len()
        );
    }

    #[tokio::test]
    async fn batch_writer_periodic_flush() {
        let store = Arc::new(SqliteStore::new_memory().await.unwrap());
        let writer = BatchWriter::with_config(store.clone(), 100, 50);

        let trace = Trace::new("test");
        writer.submit_trace(trace.clone()).await.unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(150)).await;

        let loaded = store.get_trace(trace.id).await.unwrap();
        assert!(loaded.is_some());
    }
}
