use crate::{State, command::Command, config::RunnableConfig, error::JunctureError, node::Node};
use std::{marker::PhantomData, sync::Arc};

/// Conversion trait for creating nodes from async functions
///
/// This trait allows async functions with various signatures to be used
/// as nodes in a Juncture graph.
///
/// # Examples
///
/// ```ignore
/// use juncture_core::{IntoNode, State, Node, Command};
/// use juncture_core::node::NodeFnUpdate;
/// use std::sync::Arc;
///
/// struct MyState;
/// impl State for MyState {
///     type Update = MyStateUpdate;
/// }
///
/// struct MyStateUpdate;
///
/// // Simple function returning only state update
/// async fn simple_node(state: MyState) -> Result<MyStateUpdate, juncture_core::JunctureError> {
///     Ok(MyStateUpdate)
/// }
///
/// // Wrap the function to use it as a node
/// let node: Arc<dyn Node<MyState>> = NodeFnUpdate(simple_node).into_node("simple");
/// ```
pub trait IntoNode<S: State> {
    /// Convert this value into a node with the given name
    fn into_node(self, name: &str) -> Arc<dyn Node<S>>;
}

/// Wrapper for async functions returning `Result<S::Update, JunctureError>`
#[derive(Debug)]
pub struct NodeFnUpdate<F>(pub F);

/// Wrapper for async functions taking `RunnableConfig` and returning `Result<S::Update, JunctureError>`
#[derive(Debug)]
pub struct NodeFnUpdateWithConfig<F>(pub F);

/// Wrapper for async functions returning `Result<Command<S>, JunctureError>`
#[derive(Debug)]
pub struct NodeFnCommand<F>(pub F);

/// Wrapper for async functions taking `RunnableConfig` and returning `Result<Command<S>, JunctureError>`
#[derive(Debug)]
pub struct NodeFnCommandWithConfig<F>(pub F);

impl<S, F, Fut> IntoNode<S> for NodeFnUpdate<F>
where
    S: State,
    F: Fn(S) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<S::Update, JunctureError>> + Send + 'static,
{
    fn into_node(self, name: &str) -> Arc<dyn Node<S>> {
        Arc::new(FnNodeUpdateOnly {
            name: name.to_string(),
            func: self.0,
            _phantom: PhantomData,
        })
    }
}

impl<S, F, Fut> IntoNode<S> for NodeFnUpdateWithConfig<F>
where
    S: State,
    F: Fn(S, RunnableConfig) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<S::Update, JunctureError>> + Send + 'static,
{
    fn into_node(self, name: &str) -> Arc<dyn Node<S>> {
        Arc::new(FnNodeUpdateWithConfig {
            name: name.to_string(),
            func: self.0,
            _phantom: PhantomData,
        })
    }
}

impl<S, F, Fut> IntoNode<S> for NodeFnCommand<F>
where
    S: State,
    F: Fn(S) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<Command<S>, JunctureError>> + Send + 'static,
{
    fn into_node(self, name: &str) -> Arc<dyn Node<S>> {
        Arc::new(FnNodeCommandOnly {
            name: name.to_string(),
            func: self.0,
            _phantom: PhantomData,
        })
    }
}

impl<S, F, Fut> IntoNode<S> for NodeFnCommandWithConfig<F>
where
    S: State,
    F: Fn(S, RunnableConfig) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<Command<S>, JunctureError>> + Send + 'static,
{
    fn into_node(self, name: &str) -> Arc<dyn Node<S>> {
        Arc::new(FnNodeCommandWithConfig {
            name: name.to_string(),
            func: self.0,
            _phantom: PhantomData,
        })
    }
}

#[allow(
    dead_code,
    reason = "fields used via Node trait, not directly accessed"
)]
struct FnNodeUpdateOnly<S, F, Fut>
where
    S: State,
    F: Fn(S) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<S::Update, JunctureError>> + Send + 'static,
{
    name: String,
    func: F,
    _phantom: PhantomData<fn(S) -> Fut>,
}

impl<S, F, Fut> Node<S> for FnNodeUpdateOnly<S, F, Fut>
where
    S: State,
    F: Fn(S) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<S::Update, JunctureError>> + Send + 'static,
{
    fn call(
        &self,
        state: S,
        _config: &RunnableConfig,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Command<S>, JunctureError>> + Send + '_>,
    > {
        Box::pin(async move {
            let update = (self.func)(state).await?;
            Ok(Command::update(update))
        })
    }

    fn name(&self) -> &str {
        &self.name
    }
}

#[allow(
    dead_code,
    reason = "fields used via Node trait, not directly accessed"
)]
struct FnNodeUpdateWithConfig<S, F, Fut>
where
    S: State,
    F: Fn(S, RunnableConfig) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<S::Update, JunctureError>> + Send + 'static,
{
    name: String,
    func: F,
    _phantom: PhantomData<fn(S, RunnableConfig) -> Fut>,
}

impl<S, F, Fut> Node<S> for FnNodeUpdateWithConfig<S, F, Fut>
where
    S: State,
    F: Fn(S, RunnableConfig) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<S::Update, JunctureError>> + Send + 'static,
{
    fn call(
        &self,
        state: S,
        config: &RunnableConfig,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Command<S>, JunctureError>> + Send + '_>,
    > {
        let config = config.clone();
        Box::pin(async move {
            let update = (self.func)(state, config).await?;
            Ok(Command::update(update))
        })
    }

    fn name(&self) -> &str {
        &self.name
    }
}

#[allow(
    dead_code,
    reason = "fields used via Node trait, not directly accessed"
)]
struct FnNodeCommandOnly<S, F, Fut>
where
    S: State,
    F: Fn(S) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<Command<S>, JunctureError>> + Send + 'static,
{
    name: String,
    func: F,
    _phantom: PhantomData<fn(S) -> Fut>,
}

impl<S, F, Fut> Node<S> for FnNodeCommandOnly<S, F, Fut>
where
    S: State,
    F: Fn(S) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<Command<S>, JunctureError>> + Send + 'static,
{
    fn call(
        &self,
        state: S,
        _config: &RunnableConfig,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Command<S>, JunctureError>> + Send + '_>,
    > {
        Box::pin(async move { (self.func)(state).await })
    }

    fn name(&self) -> &str {
        &self.name
    }
}

#[allow(
    dead_code,
    reason = "fields used via Node trait, not directly accessed"
)]
struct FnNodeCommandWithConfig<S, F, Fut>
where
    S: State,
    F: Fn(S, RunnableConfig) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<Command<S>, JunctureError>> + Send + 'static,
{
    name: String,
    func: F,
    _phantom: PhantomData<fn(S, RunnableConfig) -> Fut>,
}

impl<S, F, Fut> Node<S> for FnNodeCommandWithConfig<S, F, Fut>
where
    S: State,
    F: Fn(S, RunnableConfig) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<Command<S>, JunctureError>> + Send + 'static,
{
    fn call(
        &self,
        state: S,
        config: &RunnableConfig,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Command<S>, JunctureError>> + Send + '_>,
    > {
        let config = config.clone();
        Box::pin(async move { (self.func)(state, config).await })
    }

    fn name(&self) -> &str {
        &self.name
    }
}

// Rust guideline compliant 2026-05-18
