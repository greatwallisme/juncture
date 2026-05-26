use crate::{State, command::Command, config::RunnableConfig, error::JunctureError};

/// Node trait for graph execution
///
/// A node represents a unit of work in a Juncture graph. Nodes receive
/// an owned state snapshot and return a command indicating how to update
/// the state and where to route next.
///
/// # Examples
///
/// ```ignore
/// use juncture_core::{State, Node, Command, RunnableConfig};
/// use std::future::Ready;
/// use std::pin::Pin;
///
/// struct MyState;
/// impl State for MyState {
///     type Update = MyStateUpdate;
/// }
///
/// struct MyStateUpdate;
///
/// struct MyNode;
///
/// impl Node<MyState> for MyNode {
///     fn call(
///         &self,
///         state: MyState,
///         config: &RunnableConfig,
///     ) -> Pin<Box<dyn std::future::Future<Output = Result<Command<MyState>, JunctureError>> + '_>> {
///         Box::pin(async move {
///             Ok(Command::end())
///         })
///     }
///
///     fn name(&self) -> &str {
///         "my_node"
///     }
/// }
/// ```
pub trait Node<S: State>: Send + Sync + 'static {
    /// Execute the node logic
    ///
    /// Receives a shared state reference and configuration, returns a command
    /// indicating state updates and routing decisions.
    ///
    /// # Errors
    ///
    /// Returns a [`JunctureError`] if node execution fails. The error will be
    /// propagated to error handlers if configured.
    fn call(
        &self,
        state: &S,
        config: &RunnableConfig,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Command<S>, JunctureError>> + Send + '_>,
    >;

    /// Execute node logic with `Arc<S>` to avoid O(N) state clone.
    ///
    /// When the caller already holds `Arc<S>` (e.g., the Pregel runner), this
    /// method avoids the `O(state_size)` clone that `call()` requires for lifetime
    /// extension into async blocks. The default implementation delegates to
    /// [`call`](Self::call), which still clones. Wrapper types override this to
    /// use O(1) `Arc::clone` instead.
    ///
    /// # Errors
    ///
    /// Returns a [`JunctureError`] if node execution fails.
    fn call_arc(
        &self,
        state: std::sync::Arc<S>,
        config: &RunnableConfig,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Command<S>, JunctureError>> + Send + '_>,
    > {
        self.call(&state, config)
    }

    /// Get the node name
    ///
    /// Returns the node's identifier used for logging, tracing, and error messages.
    fn name(&self) -> &str;
}

// Rust guideline compliant 2025-01-18
