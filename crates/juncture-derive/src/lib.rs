use proc_macro::TokenStream;
use syn::{DeriveInput, parse_macro_input};

mod state_derive;

/// Derive macro for State trait
///
/// Generates:
/// - Update struct (each field becomes Option<T>)
/// - `FieldVersions` struct (each field becomes u64)
/// - Field index constants
/// - State trait implementation
#[proc_macro_derive(State, attributes(reducer, state_version, migrate_from))]
pub fn derive_state(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    state_derive::derive_state_impl(input)
}

// Rust guideline compliant 2025-01-18
