use proc_macro::TokenStream;
use syn::{DeriveInput, parse_macro_input};

mod entrypoint_attr;
mod state_derive;
mod task_attr;

/// Derive macro for State trait
///
/// Generates:
/// - Update struct (each field becomes `Option<T>`)
/// - Field index constants
/// - State trait implementation
#[proc_macro_derive(State, attributes(reducer, state_version, migrate_from, subset_of))]
pub fn derive_state(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    state_derive::derive_state_impl(input)
}

/// Functional-API task attribute (design `03-pregel-engine` §13.3).
///
/// Wraps an `async fn(...) -> Result<O, E>` into a callable returning a
/// `SyncAsyncFuture<O>` (from `juncture_core::pregel`), applying optional `cache`,
/// `retry`, `timeout`, and `name` policies.
///
/// # Example
///
/// ```ignore
/// #[juncture_derive::task(cache = CachePolicy::ttl(Duration::from_secs(300)))]
/// async fn analyze(input: String) -> Result<Analysis, JunctureError> { /* ... */ }
///
/// let value: Analysis = analyze("query".to_string()).result().await?;
/// ```
#[proc_macro_attribute]
pub fn task(attr: TokenStream, item: TokenStream) -> TokenStream {
    match task_attr::task_impl(attr.into(), item.into()) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

/// Functional-API entrypoint attribute (design `03-pregel-engine` §13.3).
///
/// Marks an `async fn(&S) -> Result<S::Update, JunctureError>` as a workflow
/// entrypoint and generates a `compile()` accessor that builds a
/// `CompiledGraph` via `compile_entrypoint_with_config`, wiring the `cache`,
/// `retry`, `timeout`, `name`, and optional `checkpointer` args into a
/// `TaskConfig`.
///
/// # Example
///
/// ```ignore
/// #[juncture_derive::entrypoint]
/// async fn workflow(state: &MyState) -> Result<MyStateUpdate, JunctureError> { /* ... */ }
///
/// let graph = compile::<MyState, MyState, MyState>(None)?;
/// ```
#[proc_macro_attribute]
pub fn entrypoint(attr: TokenStream, item: TokenStream) -> TokenStream {
    match entrypoint_attr::entrypoint_impl(attr.into(), item.into()) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

// Rust guideline compliant 2026-07-29
