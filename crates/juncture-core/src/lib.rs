pub mod chat;
pub mod checkpoint;
pub mod client;
pub mod command;
pub mod config;
pub mod edge;
pub mod error;
pub mod graph;
pub mod interrupt;
pub mod llm;
pub mod node;
pub mod observability;
pub mod prebuilt;
pub mod pregel;
pub mod runtime;
pub mod send;
pub mod state;
pub mod store;
pub mod stream;
pub mod subgraph;
pub mod tools;

/// Interrupt macro for human-in-the-loop interactions (task-local version)
///
/// When called, execution either returns a resume value (if resuming)
/// or sends an interrupt signal and returns an error.
///
/// This macro uses task-local storage to access the interrupt context,
/// so it doesn't need to be passed explicitly. The task-local must be
/// set by the Pregel engine before spawning node tasks.
///
/// # Syntax
///
/// ```ignore
/// // Anonymous interrupt (auto-generated ID from node name + index):
/// interrupt!(payload)
///
/// // Named interrupt (user-specified ID for targeted resume):
/// interrupt!(id, payload)
/// ```
///
/// # Examples
///
/// ```ignore
/// use juncture_core::interrupt;
/// use serde_json::json;
///
/// async fn my_node(state: MyState) -> Result<MyStateUpdate, JunctureError> {
///     // Anonymous interrupt -- ID is auto-generated from node + index
///     let decision: serde_json::Value = interrupt!(
///         json!({"question": "Continue?", "options": ["yes", "no"]})
///     )?;
///
///     // Named interrupt -- caller can resume by ID
///     let approval: serde_json::Value = interrupt!(
///         "approve_step",
///         json!({"question": "Approve this action?", "action": "delete"})
///     )?;
///
///     Ok(MyStateUpdate::default())
/// }
/// ```
#[macro_export]
macro_rules! interrupt {
    // Named interrupt: interrupt!("my_id", json!({...}))
    ($id:expr, $payload:expr) => {{
        $crate::interrupt::INTERRUPT_CONTEXT
            .try_with(|ctx| {
                Box::pin($crate::interrupt::__interrupt_impl(
                    &**ctx,
                    ::serde_json::to_value(&$payload)
                        .expect("interrupt payload must be serializable"),
                    Some($id),
                ))
                .await
            })
            .unwrap_or_else(|_| {
                Err($crate::JunctureError::execution(
                    "interrupt context not set in task-local",
                ))
            })
    }};
    // Anonymous interrupt: interrupt!(json!({...}))
    ($payload:expr) => {{
        $crate::interrupt::INTERRUPT_CONTEXT
            .try_with(|ctx| {
                Box::pin($crate::interrupt::__interrupt_impl(
                    &**ctx,
                    ::serde_json::to_value(&$payload)
                        .expect("interrupt payload must be serializable"),
                    None,
                ))
                .await
            })
            .unwrap_or_else(|_| {
                Err($crate::JunctureError::execution(
                    "interrupt context not set in task-local",
                ))
            })
    }};
}

/// Interrupt macro for human-in-the-loop interactions (explicit context)
///
/// When called, execution either returns a resume value (if resuming)
/// or sends an interrupt signal and returns an error.
///
/// This macro requires the context to be passed explicitly. Use the
/// `interrupt!` macro for the task-local version.
///
/// # Syntax
///
/// ```ignore
/// // Anonymous interrupt (auto-generated ID from node name + index):
/// interrupt_with_ctx!(context, payload)
///
/// // Named interrupt (user-specified ID for targeted resume):
/// interrupt_with_ctx!(context, id, payload)
/// ```
///
/// # Examples
///
/// ```ignore
/// use juncture_core::interrupt;
/// use serde_json::json;
///
/// async fn my_node(state: MyState, ctx: &InterruptContext) -> Result<MyStateUpdate, JunctureError> {
///     // Anonymous interrupt -- ID is auto-generated from node + index
///     let decision: serde_json::Value = interrupt_with_ctx!(
///         ctx,
///         json!({"question": "Continue?", "options": ["yes", "no"]})
///     )?;
///
///     // Named interrupt -- caller can resume by ID
///     let approval: serde_json::Value = interrupt_with_ctx!(
///         ctx,
///         "approve_step",
///         json!({"question": "Approve this action?", "action": "delete"})
///     )?;
///
///     Ok(MyStateUpdate::default())
/// }
/// ```
#[macro_export]
macro_rules! interrupt_with_ctx {
    // Named interrupt: interrupt_with_ctx!(ctx, "my_id", json!({...}))
    ($ctx:expr, $id:expr, $payload:expr) => {{
        $crate::interrupt::__interrupt_impl(
            $ctx,
            ::serde_json::to_value(&$payload).expect("interrupt payload must be serializable"),
            Some($id),
        )
        .await
    }};
    // Anonymous interrupt: interrupt_with_ctx!(ctx, json!({...}))
    ($ctx:expr, $payload:expr) => {{
        $crate::interrupt::__interrupt_impl(
            $ctx,
            ::serde_json::to_value(&$payload).expect("interrupt payload must be serializable"),
            None,
        )
        .await
    }};
}

/// Parent command macro for subgraph-to-parent routing
///
/// Allows a node inside a subgraph to request routing to a specific node
/// in the parent graph. This works as an exception mechanism: the macro
/// returns a `JunctureError::parent_command(target)` which the
/// `SubgraphNode` wrapper catches and converts to `Command::goto(target)`.
///
/// # Syntax
///
/// ```ignore
/// parent_command!("target_node_name")
/// ```
///
/// # Examples
///
/// ```ignore
/// use juncture_core::parent_command;
///
/// async fn my_subgraph_node(state: SubState) -> Result<SubStateUpdate, JunctureError> {
///     if should_exit() {
///         // Route directly to "publish" node in the parent graph
///         parent_command!("publish");
///     }
///     Ok(SubStateUpdate::default())
/// }
/// ```
#[macro_export]
macro_rules! parent_command {
    ($target:expr) => {
        return Err($crate::JunctureError::parent_command($target))
    };
}

pub use chat::{ChatAnthropic, ChatOllama, ChatOpenAI};
pub use checkpoint::{
    CHECKPOINT_NS_SEPARATOR, CheckpointNamespace, CheckpointSaver, DeltaCounters,
    NamespaceSegment, generate_checkpoint_id,
};
pub use client::{
    AuthConfig, ClientError, GraphClient, InvokeConfig, JunctureClient, StateSnapshot, Thread,
};
pub use command::{Command, CommandGoto, Final, Goto, GraphTarget, ParentCommand, SendTarget};
pub use config::{CacheConfig, CachePolicy, EntrypointConfig, RunnableConfig, TaskConfig};
pub use edge::{END, Edge, PathMap, RouteResult, Router, START, TriggerTable};
pub use error::{ErrorCode, InvalidUpdateError, JunctureError, NodeTimeoutError};
pub use graph::{
    CompiledGraph, DrawableEdge, DrawableGraph, DrawableNode, ErrorHandlerNode, GraphOutput,
    GraphOutputMetadata, InterruptInfo, NodeMetadata, RemoteGraph, RetryPolicy, RetryingNode,
    StateFilter, StateGraph, StateUpdate, StreamHandle, SubgraphInfo, TopologyError,
};
pub use interrupt::{
    HIDDEN_TAG, InterruptContext, InterruptSignal, ResumeValue, Scratchpad, generate_interrupt_id,
    should_interrupt,
};
pub use llm::{
    CallOptions, ChatModel, JsonSchema, LlmError, MessageChunk, StructuredOutputModel, ToolChoice,
    ToolDefinition,
};
pub use node::{IntoNode, Node, NodeError};
pub use observability::{CacheKeyInput, GraphLifecycleCallback, MetricsCollector, ServerInfo};
pub use prebuilt::{PromptSource, ReactAgentConfig};
pub use pregel::{
    BubbleUp, BudgetConfig, BudgetExceededAction, BudgetExceededReason, BudgetTracker, BudgetUsage,
    Durability, ExecutionConfig, ExecutionContext, FieldVersionTracker, GraphDrained,
    GraphInterrupt, LoopStatus, PendingTask, PregelLoop, PregelProtocol,
    StreamEvent as PregelStreamEvent, SuperstepResult, SyncAsyncFuture, TaskOutput, TaskTrigger,
    TimeoutPolicy, TriggerToNodes, apply_writes, compute_next_tasks, execute_superstep,
};
pub use runtime::{ExecutionInfo, Heartbeat, ManagedValues, RunControl, Runtime, RuntimeStore};
pub use send::Send;
pub use state::{
    AnyValueReducer, AppendReducer, Channel, Content, ContentPart, CowState, DeltaBlob,
    DeltaChannel, EphemeralChannel, FieldsChanged, FromState, ImageData, ImageSource, IntoState,
    LastValueAfterFinishChannel, LastWriteWinsReducer, Message, MessagesState, MessagesStateUpdate,
    Overwrite, REMOVE_ALL_MESSAGES, Reducer, RemoveMessage, ReplaceReducer, Role, State,
    TokenUsage, ToolCall, UntrackedChannel, messages_reducer,
};
pub use store::{
    EmbeddingFunc, FilterExpr, IndexConfig, Item, MemoryStore, SearchItem, SearchQuery,
    SearchResult, Store, StoreError, StoreOp, StoreResult, TTLConfig,
};
pub use stream::{
    BatchTransformer, DebugEvent, EventEmitter, FilterFieldsTransformer, JsonParseTransformer,
    MessageBatchConfig, StreamChannel, StreamConfig, StreamEvent, StreamMode, StreamPart,
    StreamResumption, StreamTransformer, StreamWriter, TaskEventType, ToolsEvent,
    call_llm_streaming,
};
pub use subgraph::{
    StateSubset, SubgraphConfig, SubgraphMount, SubgraphNode, SubgraphPersistence,
    SubgraphTransformer,
};
pub use tools::{
    NopToolInterceptor, StatefulTool, Tool, ToolCallTransformer, ToolError, ToolExecutionTrace,
    ToolInterceptor, ToolNode, ToolNodeConfig, ToolRuntime, tools_condition,
};

// Rust guideline compliant 2026-05-19
