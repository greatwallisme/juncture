use crate::{
    State,
    command::Command,
    config::RunnableConfig,
    error::JunctureError,
    node::Node,
    runtime::Runtime,
};
use std::{marker::PhantomData, sync::Arc};

/// Conversion trait for creating nodes from async functions
///
/// This trait allows async functions with various signatures to be used
/// as nodes in a Juncture graph.
///
/// # Supported Function Signatures
///
/// The trait supports multiple function signatures for flexibility:
///
/// ## Forms A-D (existing, without Runtime)
///
/// ```ignore
/// use juncture_core::{IntoNode, State, Node, Command};
/// use juncture_core::node::NodeFnUpdate;
/// use std::sync::Arc;
///
/// # struct MyState;
/// # impl State for MyState { type Update = MyStateUpdate; }
/// # struct MyStateUpdate;
///
/// // Form A: fn(S) -> Result<S::Update>
/// async fn simple_node(state: MyState) -> Result<MyStateUpdate, juncture_core::JunctureError> {
///     Ok(MyStateUpdate)
/// }
///
/// // Form B: fn(S, RunnableConfig) -> Result<S::Update>
/// async fn with_config(state: MyState, config: RunnableConfig) -> Result<MyStateUpdate, juncture_core::JunctureError> {
///     Ok(MyStateUpdate)
/// }
///
/// // Form C: fn(S) -> Result<Command<S>>
/// async fn with_command(state: MyState) -> Result<Command<MyState>, juncture_core::JunctureError> {
///     Ok(Command::end())
/// }
///
/// // Form D: fn(S, RunnableConfig) -> Result<Command<S>>
/// async fn full_form(state: MyState, config: RunnableConfig) -> Result<Command<MyState>, juncture_core::JunctureError> {
///     Ok(Command::end())
/// }
/// ```
///
/// ## Forms E-F (new, with Runtime<C> for dependency injection)
///
/// ```ignore
/// use juncture_core::{IntoNode, State, Runtime};
/// use juncture_core::node::NodeFnUpdateWithRuntime;
///
/// # struct MyState;
/// # impl State for MyState { type Update = MyStateUpdate; }
/// # struct MyStateUpdate;
/// # struct MyContext { user_id: String }
///
/// // Form E: fn(S, Runtime<C>) -> Result<S::Update>
/// async fn with_runtime(state: MyState, runtime: Runtime<MyContext>) -> Result<MyStateUpdate, juncture_core::JunctureError> {
///     // Access runtime context
///     let user_id = &runtime.context.user_id;
///     Ok(MyStateUpdate)
/// }
///
/// // Form F: fn(S, RunnableConfig, Runtime<C>) -> Result<S::Update>
/// async fn with_all(state: MyState, config: RunnableConfig, runtime: Runtime<MyContext>) -> Result<MyStateUpdate, juncture_core::JunctureError> {
///     // Full access to state, config, and runtime
///     Ok(MyStateUpdate)
/// }
/// ```
///
/// # Examples
///
/// ```ignore
/// use juncture_core::{IntoNode, State, Runtime};
/// use juncture_core::node::NodeFnUpdateWithRuntime;
/// use std::sync::Arc;
///
/// # struct MyState;
/// # impl State for MyState { type Update = MyStateUpdate; }
/// # struct MyStateUpdate;
/// # struct MyContext { user_id: String }
///
/// // Create a Runtime with custom context
/// let runtime = Runtime::with_context(MyContext { user_id: "user-123".to_string() });
///
/// // Wrap a Runtime-aware function
/// let wrapper = NodeFnUpdateWithRuntime::new(
///     async fn my_node(state: MyState, runtime: Runtime<MyContext>) -> Result<MyStateUpdate, JunctureError> {
///         // Use runtime context
///         Ok(MyStateUpdate)
///     },
///     runtime
/// );
///
/// // Convert to a node
/// let node: Arc<dyn Node<MyState>> = wrapper.into_node("my_node");
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

/// Wrapper for async functions taking `Runtime<C>` and returning `Result<S::Update, JunctureError>`
///
/// Form E: async functions that receive Runtime for dependency injection
/// and return a state update.
///
/// # Examples
///
/// ```ignore
/// use juncture_core::{IntoNode, State, Runtime};
/// use juncture_core::node::NodeFnUpdateWithRuntime;
///
/// struct MyContext;
/// struct MyState;
/// impl State for MyState {
///     type Update = MyStateUpdate;
/// }
///
/// async fn my_node(state: MyState, runtime: Runtime<MyContext>) -> Result<MyStateUpdate, JunctureError> {
///     Ok(MyStateUpdate)
/// }
///
/// let node = NodeFnUpdateWithRuntime::new(my_node).into_node("my_node");
/// ```
pub struct NodeFnUpdateWithRuntime<F, C>
where
    C: Clone + Send + Sync + 'static,
{
    /// The wrapped async function
    pub func: F,
    /// Runtime context to inject into the function
    pub runtime: Runtime<C>,
    _phantom: PhantomData<fn() -> C>,
}

impl<F, C> std::fmt::Debug for NodeFnUpdateWithRuntime<F, C>
where
    F: std::fmt::Debug,
    C: Clone + Send + Sync + 'static + std::fmt::Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NodeFnUpdateWithRuntime")
            .field("func", &self.func)
            .field("runtime", &self.runtime)
            .field("_phantom", &self._phantom)
            .finish()
    }
}

impl<F, C> NodeFnUpdateWithRuntime<F, C>
where
    C: Clone + Send + Sync + 'static,
{
    /// Create a new wrapper for a Runtime-aware async function
    ///
    /// # Arguments
    ///
    /// * `func` - Async function accepting (state, Runtime<C>)
    /// * `runtime` - Runtime context to inject
    #[must_use]
    pub const fn new(func: F, runtime: Runtime<C>) -> Self {
        Self {
            func,
            runtime,
            _phantom: PhantomData,
        }
    }
}

/// Wrapper for async functions taking `(S, RunnableConfig, Runtime<C>)` and returning `Result<S::Update, JunctureError>`
///
/// Form F: async functions that receive state, config, and Runtime
/// and return a state update.
pub struct NodeFnUpdateWithConfigAndRuntime<F, C>
where
    C: Clone + Send + Sync + 'static,
{
    /// The wrapped async function
    pub func: F,
    /// Runtime context to inject into the function
    pub runtime: Runtime<C>,
    _phantom: PhantomData<fn() -> C>,
}

impl<F, C> std::fmt::Debug for NodeFnUpdateWithConfigAndRuntime<F, C>
where
    F: std::fmt::Debug,
    C: Clone + Send + Sync + 'static + std::fmt::Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NodeFnUpdateWithConfigAndRuntime")
            .field("func", &self.func)
            .field("runtime", &self.runtime)
            .field("_phantom", &self._phantom)
            .finish()
    }
}

impl<F, C> NodeFnUpdateWithConfigAndRuntime<F, C>
where
    C: Clone + Send + Sync + 'static,
{
    /// Create a new wrapper for a Runtime-aware async function with config
    ///
    /// # Arguments
    ///
    /// * `func` - Async function accepting (state, config, Runtime<C>)
    /// * `runtime` - Runtime context to inject
    #[must_use]
    pub const fn new(func: F, runtime: Runtime<C>) -> Self {
        Self {
            func,
            runtime,
            _phantom: PhantomData,
        }
    }
}

/// Wrapper for async functions taking `Runtime<C>` and returning `Result<Command<S>, JunctureError>`
///
/// Form E (Command variant): async functions that receive Runtime
/// and return a Command.
pub struct NodeFnCommandWithRuntime<F, C>
where
    C: Clone + Send + Sync + 'static,
{
    /// The wrapped async function
    pub func: F,
    /// Runtime context to inject into the function
    pub runtime: Runtime<C>,
    _phantom: PhantomData<fn() -> C>,
}

impl<F, C> std::fmt::Debug for NodeFnCommandWithRuntime<F, C>
where
    F: std::fmt::Debug,
    C: Clone + Send + Sync + 'static + std::fmt::Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NodeFnCommandWithRuntime")
            .field("func", &self.func)
            .field("runtime", &self.runtime)
            .field("_phantom", &self._phantom)
            .finish()
    }
}

impl<F, C> NodeFnCommandWithRuntime<F, C>
where
    C: Clone + Send + Sync + 'static,
{
    /// Create a new wrapper for a Runtime-aware async function returning Command
    ///
    /// # Arguments
    ///
    /// * `func` - Async function accepting (state, Runtime<C>)
    /// * `runtime` - Runtime context to inject
    #[must_use]
    pub const fn new(func: F, runtime: Runtime<C>) -> Self {
        Self {
            func,
            runtime,
            _phantom: PhantomData,
        }
    }
}

/// Wrapper for async functions taking `(S, RunnableConfig, Runtime<C>)` and returning `Result<Command<S>, JunctureError>`
///
/// Form F (Command variant): async functions that receive state, config, and Runtime
/// and return a Command.
pub struct NodeFnCommandWithConfigAndRuntime<F, C>
where
    C: Clone + Send + Sync + 'static,
{
    /// The wrapped async function
    pub func: F,
    /// Runtime context to inject into the function
    pub runtime: Runtime<C>,
    _phantom: PhantomData<fn() -> C>,
}

impl<F, C> std::fmt::Debug for NodeFnCommandWithConfigAndRuntime<F, C>
where
    F: std::fmt::Debug,
    C: Clone + Send + Sync + 'static + std::fmt::Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NodeFnCommandWithConfigAndRuntime")
            .field("func", &self.func)
            .field("runtime", &self.runtime)
            .field("_phantom", &self._phantom)
            .finish()
    }
}

impl<F, C> NodeFnCommandWithConfigAndRuntime<F, C>
where
    C: Clone + Send + Sync + 'static,
{
    /// Create a new wrapper for a Runtime-aware async function with config returning Command
    ///
    /// # Arguments
    ///
    /// * `func` - Async function accepting (state, config, Runtime<C>)
    /// * `runtime` - Runtime context to inject
    #[must_use]
    pub const fn new(func: F, runtime: Runtime<C>) -> Self {
        Self {
            func,
            runtime,
            _phantom: PhantomData,
        }
    }
}

// Existing blanket impls for forms A-D

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

// Form E (Update variant): Runtime<C> parameter, returns Update
impl<S, F, Fut, C> IntoNode<S> for NodeFnUpdateWithRuntime<F, C>
where
    S: State,
    C: Clone + Send + Sync + 'static,
    F: Fn(S, Runtime<C>) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<S::Update, JunctureError>> + Send + 'static,
{
    fn into_node(self, name: &str) -> Arc<dyn Node<S>> {
        Arc::new(FnNodeUpdateWithRuntime {
            name: name.to_string(),
            func: self.func,
            runtime: self.runtime,
            _phantom: PhantomData,
        })
    }
}

// Form F (Update variant): config + Runtime<C> parameter, returns Update
impl<S, F, Fut, C> IntoNode<S> for NodeFnUpdateWithConfigAndRuntime<F, C>
where
    S: State,
    C: Clone + Send + Sync + 'static,
    F: Fn(S, RunnableConfig, Runtime<C>) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<S::Update, JunctureError>> + Send + 'static,
{
    fn into_node(self, name: &str) -> Arc<dyn Node<S>> {
        Arc::new(FnNodeUpdateWithConfigAndRuntime {
            name: name.to_string(),
            func: self.func,
            runtime: self.runtime,
            _phantom: PhantomData,
        })
    }
}

// Form E (Command variant): Runtime<C> parameter, returns Command
impl<S, F, Fut, C> IntoNode<S> for NodeFnCommandWithRuntime<F, C>
where
    S: State,
    C: Clone + Send + Sync + 'static,
    F: Fn(S, Runtime<C>) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<Command<S>, JunctureError>> + Send + 'static,
{
    fn into_node(self, name: &str) -> Arc<dyn Node<S>> {
        Arc::new(FnNodeCommandWithRuntime {
            name: name.to_string(),
            func: self.func,
            runtime: self.runtime,
            _phantom: PhantomData,
        })
    }
}

// Form F (Command variant): config + Runtime<C> parameter, returns Command
impl<S, F, Fut, C> IntoNode<S> for NodeFnCommandWithConfigAndRuntime<F, C>
where
    S: State,
    C: Clone + Send + Sync + 'static,
    F: Fn(S, RunnableConfig, Runtime<C>) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<Command<S>, JunctureError>> + Send + 'static,
{
    fn into_node(self, name: &str) -> Arc<dyn Node<S>> {
        Arc::new(FnNodeCommandWithConfigAndRuntime {
            name: name.to_string(),
            func: self.func,
            runtime: self.runtime,
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

// Form E (Update variant): Runtime<C> parameter
#[allow(
    dead_code,
    reason = "fields used via Node trait, not directly accessed"
)]
struct FnNodeUpdateWithRuntime<S, F, Fut, C>
where
    S: State,
    C: Clone + Send + Sync + 'static,
    F: Fn(S, Runtime<C>) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<S::Update, JunctureError>> + Send + 'static,
{
    name: String,
    func: F,
    runtime: Runtime<C>,
    #[allow(
        clippy::type_complexity,
        reason = "PhantomData needs to capture all generic parameters including complex Future type"
    )]
    _phantom: PhantomData<fn(S, Runtime<C>) -> Fut>,
}

impl<S, F, Fut, C> Node<S> for FnNodeUpdateWithRuntime<S, F, Fut, C>
where
    S: State,
    C: Clone + Send + Sync + 'static,
    F: Fn(S, Runtime<C>) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<S::Update, JunctureError>> + Send + 'static,
{
    fn call(
        &self,
        state: S,
        _config: &RunnableConfig,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Command<S>, JunctureError>> + Send + '_>,
    > {
        let runtime = self.runtime.clone();
        Box::pin(async move {
            let update = (self.func)(state, runtime).await?;
            Ok(Command::update(update))
        })
    }

    fn name(&self) -> &str {
        &self.name
    }
}

// Form F (Update variant): config + Runtime<C> parameter
#[allow(
    dead_code,
    reason = "fields used via Node trait, not directly accessed"
)]
struct FnNodeUpdateWithConfigAndRuntime<S, F, Fut, C>
where
    S: State,
    C: Clone + Send + Sync + 'static,
    F: Fn(S, RunnableConfig, Runtime<C>) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<S::Update, JunctureError>> + Send + 'static,
{
    name: String,
    func: F,
    runtime: Runtime<C>,
    #[allow(
        clippy::type_complexity,
        reason = "PhantomData needs to capture all generic parameters including complex Future type"
    )]
    _phantom: PhantomData<fn(S, RunnableConfig, Runtime<C>) -> Fut>,
}

impl<S, F, Fut, C> Node<S> for FnNodeUpdateWithConfigAndRuntime<S, F, Fut, C>
where
    S: State,
    C: Clone + Send + Sync + 'static,
    F: Fn(S, RunnableConfig, Runtime<C>) -> Fut + Send + Sync + 'static,
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
        let runtime = self.runtime.clone();
        Box::pin(async move {
            let update = (self.func)(state, config, runtime).await?;
            Ok(Command::update(update))
        })
    }

    fn name(&self) -> &str {
        &self.name
    }
}

// Form E (Command variant): Runtime<C> parameter
#[allow(
    dead_code,
    reason = "fields used via Node trait, not directly accessed"
)]
struct FnNodeCommandWithRuntime<S, F, Fut, C>
where
    S: State,
    C: Clone + Send + Sync + 'static,
    F: Fn(S, Runtime<C>) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<Command<S>, JunctureError>> + Send + 'static,
{
    name: String,
    func: F,
    runtime: Runtime<C>,
    #[allow(
        clippy::type_complexity,
        reason = "PhantomData needs to capture all generic parameters including complex Future type"
    )]
    _phantom: PhantomData<fn(S, Runtime<C>) -> Fut>,
}

impl<S, F, Fut, C> Node<S> for FnNodeCommandWithRuntime<S, F, Fut, C>
where
    S: State,
    C: Clone + Send + Sync + 'static,
    F: Fn(S, Runtime<C>) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<Command<S>, JunctureError>> + Send + 'static,
{
    fn call(
        &self,
        state: S,
        _config: &RunnableConfig,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Command<S>, JunctureError>> + Send + '_>,
    > {
        let runtime = self.runtime.clone();
        Box::pin(async move { (self.func)(state, runtime).await })
    }

    fn name(&self) -> &str {
        &self.name
    }
}

// Form F (Command variant): config + Runtime<C> parameter
#[allow(
    dead_code,
    reason = "fields used via Node trait, not directly accessed"
)]
struct FnNodeCommandWithConfigAndRuntime<S, F, Fut, C>
where
    S: State,
    C: Clone + Send + Sync + 'static,
    F: Fn(S, RunnableConfig, Runtime<C>) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<Command<S>, JunctureError>> + Send + 'static,
{
    name: String,
    func: F,
    runtime: Runtime<C>,
    #[allow(
        clippy::type_complexity,
        reason = "PhantomData needs to capture all generic parameters including complex Future type"
    )]
    _phantom: PhantomData<fn(S, RunnableConfig, Runtime<C>) -> Fut>,
}

impl<S, F, Fut, C> Node<S> for FnNodeCommandWithConfigAndRuntime<S, F, Fut, C>
where
    S: State,
    C: Clone + Send + Sync + 'static,
    F: Fn(S, RunnableConfig, Runtime<C>) -> Fut + Send + Sync + 'static,
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
        let runtime = self.runtime.clone();
        Box::pin(async move { (self.func)(state, config, runtime).await })
    }

    fn name(&self) -> &str {
        &self.name
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FieldsChanged;
    use crate::state::FieldVersions;

    // Test state types
    #[derive(Debug, Clone, Default, PartialEq)]
    struct TestState {
        value: i32,
    }

    #[derive(Debug, Clone, Default, PartialEq)]
    struct TestStateUpdate {
        value: Option<i32>,
    }

    impl State for TestState {
        type Update = TestStateUpdate;
        type FieldVersions = FieldVersions;

        fn apply(&mut self, update: Self::Update) -> FieldsChanged {
            if update.value.is_some() {
                self.value = update.value.unwrap();
                FieldsChanged(1u64) // Field 0 changed
            } else {
                FieldsChanged(0)
            }
        }

        fn reset_ephemeral(&mut self) {
            // No ephemeral fields in TestState
        }
    }

    // Test context type
    #[derive(Debug, Clone, Default)]
    struct TestContext {
        user_id: String,
    }

    // Test helper functions for forms E/F
    async fn form_e_update_node(
        state: TestState,
        runtime: Runtime<TestContext>,
    ) -> Result<TestStateUpdate, JunctureError> {
        assert_eq!(runtime.context.user_id, "test-user");
        Ok(TestStateUpdate {
            value: Some(state.value + 10),
        })
    }

    async fn form_f_update_node(
        state: TestState,
        config: RunnableConfig,
        _runtime: Runtime<TestContext>,
    ) -> Result<TestStateUpdate, JunctureError> {
        assert_eq!(config.recursion_limit, 0);
        Ok(TestStateUpdate {
            value: Some(state.value + 20),
        })
    }

    async fn form_e_command_node(
        state: TestState,
        runtime: Runtime<TestContext>,
    ) -> Result<Command<TestState>, JunctureError> {
        assert_eq!(runtime.context.user_id, "test-user-3");
        Ok(Command::update(TestStateUpdate {
            value: Some(state.value + 30),
        }))
    }

    async fn form_f_command_node(
        state: TestState,
        config: RunnableConfig,
        _runtime: Runtime<TestContext>,
    ) -> Result<Command<TestState>, JunctureError> {
        assert_eq!(config.recursion_limit, 0);
        Ok(Command::update(TestStateUpdate {
            value: Some(state.value + 40),
        }))
    }

    async fn shared_runtime_node(
        state: TestState,
        _runtime: Runtime<TestContext>,
    ) -> Result<TestStateUpdate, JunctureError> {
        Ok(TestStateUpdate {
            value: Some(state.value + 1),
        })
    }

    // Form E test: Runtime<C> parameter, returns Update
    #[tokio::test]
    async fn test_form_e_update_with_runtime() {
        let runtime = Runtime::with_context(TestContext {
            user_id: "test-user".to_string(),
        });

        let wrapper = NodeFnUpdateWithRuntime::new(form_e_update_node, runtime);
        let node = wrapper.into_node("test_node");

        let result = node
            .call(TestState { value: 5 }, &RunnableConfig::default())
            .await
            .unwrap();

        assert_eq!(result.update.unwrap().value, Some(15));
        assert_eq!(node.name(), "test_node");
    }

    // Form F test: config + Runtime<C> parameter, returns Update
    #[tokio::test]
    async fn test_form_f_update_with_config_and_runtime() {
        let runtime = Runtime::with_context(TestContext {
            user_id: "test-user-2".to_string(),
        });

        let wrapper = NodeFnUpdateWithConfigAndRuntime::new(form_f_update_node, runtime);
        let node = wrapper.into_node("test_node");

        let result = node
            .call(TestState { value: 5 }, &RunnableConfig::default())
            .await
            .unwrap();

        assert_eq!(result.update.unwrap().value, Some(25));
    }

    // Form E test: Runtime<C> parameter, returns Command
    #[tokio::test]
    async fn test_form_e_command_with_runtime() {
        let runtime = Runtime::with_context(TestContext {
            user_id: "test-user-3".to_string(),
        });

        let wrapper = NodeFnCommandWithRuntime::new(form_e_command_node, runtime);
        let node = wrapper.into_node("test_node");

        let result = node
            .call(TestState { value: 5 }, &RunnableConfig::default())
            .await
            .unwrap();

        assert_eq!(result.update.unwrap().value, Some(35));
    }

    // Form F test: config + Runtime<C> parameter, returns Command
    #[tokio::test]
    async fn test_form_f_command_with_config_and_runtime() {
        let runtime = Runtime::with_context(TestContext {
            user_id: "test-user-4".to_string(),
        });

        let wrapper = NodeFnCommandWithConfigAndRuntime::new(form_f_command_node, runtime);
        let node = wrapper.into_node("test_node");

        let result = node
            .call(TestState { value: 5 }, &RunnableConfig::default())
            .await
            .unwrap();

        assert_eq!(result.update.unwrap().value, Some(45));
    }

    // Test that Runtime can be cloned and used across multiple invocations
    #[tokio::test]
    async fn test_runtime_clone_multiple_invocations() {
        let runtime = Runtime::with_context(TestContext {
            user_id: "shared-user".to_string(),
        });

        let wrapper = NodeFnUpdateWithRuntime::new(shared_runtime_node, runtime);
        let node = wrapper.into_node("test_node");

        // First invocation
        let result1 = node
            .call(TestState { value: 0 }, &RunnableConfig::default())
            .await
            .unwrap();
        assert_eq!(result1.update.unwrap().value, Some(1));

        // Second invocation (should use same Runtime)
        let result2 = node
            .call(TestState { value: 10 }, &RunnableConfig::default())
            .await
            .unwrap();
        assert_eq!(result2.update.unwrap().value, Some(11));
    }
}

// Rust guideline compliant 2026-05-23
