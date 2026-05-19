//! Pregel protocol trait for unified graph execution interface
//!
//! Provides [`PregelProtocol`] as a common interface supporting both
//! local and remote graph execution.

use crate::State;
use crate::config::RunnableConfig;
use crate::pregel::stream::StreamMode;
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use std::pin::Pin;

/// Pregel protocol trait for unified graph execution
///
/// Provides a common interface for executing graphs, supporting both
/// local compiled graphs and remote graph services.
///
/// # Type Parameters
///
/// * `S` - State type implementing the [`State`] trait
///
/// # Examples
///
/// ```ignore
/// use juncture_core::pregel::protocol::PregelProtocol;
///
/// let result = graph.invoke(state, &config).await?;
/// ```
pub trait PregelProtocol<S: State>: Send + Sync + 'static {
    /// Execute the graph synchronously, blocking until completion
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - Graph execution fails
    /// - Recursion limit is exceeded
    /// - Cancellation is requested
    fn invoke(
        &self,
        input: S,
        config: &RunnableConfig,
    ) -> BoxFuture<'_, Result<S, crate::JunctureError>>;

    /// Execute the graph with streaming output
    ///
    /// Returns a stream of [`crate::pregel::stream::StreamEvent`] items as
    /// the graph executes, enabling real-time observation of execution progress.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - Graph initialization fails
    /// - The stream cannot be created
    #[allow(
        clippy::type_complexity,
        reason = "boxed stream return type requires complex generic"
    )]
    fn stream(
        &self,
        input: S,
        config: &RunnableConfig,
        mode: StreamMode,
    ) -> BoxFuture<
        '_,
        Result<
            Pin<
                Box<
                    BoxStream<
                        'static,
                        Result<crate::pregel::stream::StreamEvent<S>, crate::JunctureError>,
                    >,
                >,
            >,
            crate::JunctureError,
        >,
    >;

    /// Get current state from the checkpoint
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - No checkpointer is configured
    /// - Checkpoint loading fails
    fn get_state(
        &self,
        config: &RunnableConfig,
    ) -> BoxFuture<'_, Result<Option<crate::checkpoint::StateSnapshot<S>>, crate::JunctureError>>;

    /// Update state manually
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - No checkpointer is configured
    /// - State update fails
    fn update_state(
        &self,
        config: &RunnableConfig,
        update: S::Update,
        as_node: Option<&str>,
    ) -> BoxFuture<'_, Result<RunnableConfig, crate::JunctureError>>;
}

// Rust guideline compliant 2026-05-19
