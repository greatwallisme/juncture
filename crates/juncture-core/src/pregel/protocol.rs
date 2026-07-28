//! Pregel protocol trait for unified graph execution interface
//!
//! Provides [`PregelProtocol`] as a common interface supporting both
//! local and remote graph execution.

use crate::config::RunnableConfig;
use crate::{State, stream::StreamMode};
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
    /// Returns a stream of [`crate::stream::StreamEvent`] items as
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
                    BoxStream<'static, Result<crate::stream::StreamEvent<S>, crate::JunctureError>>,
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

/// Local implementor of [`PregelProtocol`] (design `03-pregel-engine` §11.6:
/// "提供图执行的统一接口，支持本地和远程图").
///
/// This makes [`PregelProtocol`] a live abstraction rather than dead trait
/// code: `CompiledGraph<S, S, S>` (the common `I = S`, `O = S` shape)
/// implements all four protocol methods by delegating to its real local
/// execution APIs.
///
/// `RemoteGraph` deliberately does NOT implement `PregelProtocol`: it offers
/// equivalent operations via its own client-typed API (`invoke`/`get_state`/
/// `update_state`/`resume` in `crate::graph::remote`), but the HTTP
/// [`GraphClient`](crate::client::GraphClient) has no SSE streaming endpoint
/// and its `client::StateSnapshot` shape differs from the checkpoint
/// `StateSnapshot` the protocol returns. Implementing the protocol there
/// would require fabricating unavailable data (a simplification), so the
/// remote side stays on its honest client API.
impl<S> PregelProtocol<S> for crate::graph::CompiledGraph<S, S, S>
where
    S: State + Clone + Send + Sync + serde::Serialize + serde::de::DeserializeOwned + 'static,
    S::Update: serde::Serialize,
{
    fn invoke(
        &self,
        input: S,
        config: &RunnableConfig,
    ) -> BoxFuture<'_, Result<S, crate::JunctureError>> {
        let config = config.clone();
        Box::pin(async move {
            let output = self.invoke(input, &config)?;
            Ok(output.value)
        })
    }

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
                    BoxStream<'static, Result<crate::stream::StreamEvent<S>, crate::JunctureError>>,
                >,
            >,
            crate::JunctureError,
        >,
    > {
        let config = config.clone();
        Box::pin(async move {
            let handle = self.stream(input, &config, mode).await?;
            Ok(Box::pin(handle.stream))
        })
    }

    fn get_state(
        &self,
        config: &RunnableConfig,
    ) -> BoxFuture<'_, Result<Option<crate::checkpoint::StateSnapshot<S>>, crate::JunctureError>>
    {
        let config = config.clone();
        Box::pin(async move { self.get_state(&config).await })
    }

    fn update_state(
        &self,
        config: &RunnableConfig,
        update: S::Update,
        as_node: Option<&str>,
    ) -> BoxFuture<'_, Result<RunnableConfig, crate::JunctureError>> {
        let config = config.clone();
        let as_node = as_node.map(std::string::ToString::to_string);
        Box::pin(async move {
            self.update_state(
                &config,
                crate::graph::StateUpdate {
                    update,
                    label: None,
                    as_node,
                },
            )
            .await
        })
    }
}

// Rust guideline compliant 2026-05-19
