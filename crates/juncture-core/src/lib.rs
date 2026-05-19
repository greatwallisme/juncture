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
/// interrupt!(payload)
/// ```
///
/// # Examples
///
/// ```ignore
/// use juncture_core::interrupt;
/// use serde_json::json;
///
/// async fn my_node(state: MyState) -> Result<MyStateUpdate, JunctureError> {
///     // Request human input (task-local context)
///     let decision: serde_json::Value = interrupt!(
///         json!({"question": "Continue?", "options": ["yes", "no"]})
///     )?;
///
///     // Process human decision...
///     Ok(MyStateUpdate::default())
/// }
/// ```
#[macro_export]
macro_rules! interrupt {
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
/// interrupt_with_ctx!(context, payload)
/// ```
///
/// # Examples
///
/// ```ignore
/// use juncture_core::interrupt;
/// use serde_json::json;
///
/// async fn my_node(state: MyState, ctx: &InterruptContext) -> Result<MyStateUpdate, JunctureError> {
///     // Request human input with explicit context
///     let decision: serde_json::Value = interrupt_with_ctx!(
///         ctx,
///         json!({"question": "Continue?", "options": ["yes", "no"]})
///     )?;
///
///     // Process human decision...
///     Ok(MyStateUpdate::default())
/// }
/// ```
#[macro_export]
macro_rules! interrupt_with_ctx {
    ($ctx:expr, $payload:expr) => {{
        $crate::interrupt::__interrupt_impl(
            $ctx,
            ::serde_json::to_value(&$payload).expect("interrupt payload must be serializable"),
            None,
        )
        .await
    }};
}

pub use chat::{ChatAnthropic, ChatOllama, ChatOpenAI};
pub use checkpoint::{CheckpointNamespace, CheckpointSaver, NamespaceSegment};
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
    StateFilter, StateGraph, StateUpdate, SubgraphInfo, TopologyError,
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
pub use observability::{CacheKeyInput, MetricsRegistry, ServerInfo};
pub use prebuilt::{PromptSource, ReactAgentConfig};
pub use pregel::{
    BubbleUp, BudgetConfig, BudgetExceededAction, BudgetExceededReason, BudgetTracker, BudgetUsage,
    Durability, ExecutionConfig, ExecutionContext, FieldVersionTracker, GraphDrained,
    GraphInterrupt, LoopStatus, PendingTask, PregelLoop, PregelProtocol,
    StreamEvent as PregelStreamEvent, SuperstepResult, SyncAsyncFuture, TaskOutput, TaskTrigger,
    TimeoutPolicy, TriggerToNodes, apply_writes, compute_next_tasks, execute_superstep,
};
pub use runtime::{
    ExecutionInfo, Heartbeat, ManagedValues, RunControl, Runtime, RuntimeStore, StreamWriter,
};
pub use send::Send;
pub use state::{
    AnyValueReducer, AppendReducer, Channel, Content, ContentPart, CowState, DeltaBlob,
    DeltaChannel, EphemeralChannel, FieldsChanged, FromState, ImageData, ImageSource, IntoState,
    LastValueAfterFinishChannel, LastWriteWinsReducer, Message, MessagesState, MessagesStateUpdate,
    Overwrite, REMOVE_ALL_MESSAGES, Reducer, RemoveMessage, ReplaceReducer, Role, State,
    TokenUsage, ToolCall, UntrackedChannel, messages_reducer,
};
pub use store::{
    FilterExpr, IndexConfig, Item, MemoryStore, SearchItem, SearchQuery, SearchResult, Store,
    StoreError, StoreOp, StoreResult,
};
pub use stream::{
    BatchTransformer, DebugEvent, EventEmitter, FilterFieldsTransformer, JsonParseTransformer,
    StreamChannel, StreamConfig, StreamEvent, StreamEventWriter, StreamMode, StreamPart,
    StreamResumption, StreamTransformer, TaskEventType, ToolsEvent,
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
