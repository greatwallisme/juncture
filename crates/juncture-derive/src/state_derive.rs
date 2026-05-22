use proc_macro2::{Ident, Span};
use quote::quote;
use syn::spanned::Spanned;
use syn::{Data, DataStruct, DeriveInput, Meta, MetaList, Type};

/// Main State derive implementation
#[allow(
    clippy::cognitive_complexity,
    clippy::too_many_lines,
    clippy::needless_pass_by_value,
    reason = "proc-macro code generation requires complex parsing and code generation"
)]
pub fn derive_state_impl(input: DeriveInput) -> proc_macro::TokenStream {
    let struct_name = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    // Parse container-level attributes
    let mut state_version = 1u32;
    let mut migrate_functions: Vec<(u32, syn::Path)> = Vec::new();
    let mut subset_of_parent: Option<Type> = None;

    for attr in &input.attrs {
        if attr.path().is_ident("state_version")
            && let Meta::List(MetaList { tokens, .. }) = &attr.meta
            && let Ok(version) = tokens.to_string().trim().parse::<u32>()
        {
            state_version = version;
        } else if attr.path().is_ident("migrate_from")
            && let Meta::List(MetaList { tokens, .. }) = &attr.meta
        {
            let tokens_str = tokens.to_string();
            let parts: Vec<&str> = tokens_str.split(',').collect();
            if parts.len() == 2
                && let Ok(version) = parts[0].trim().parse::<u32>()
            {
                let func_path: proc_macro::TokenStream =
                    parts[1].trim().parse().unwrap_or_default();
                if let Ok(path) = syn::parse::<syn::Path>(func_path) {
                    migrate_functions.push((version, path));
                }
            }
        } else if attr.path().is_ident("subset_of") {
            if subset_of_parent.is_some() {
                return syn::Error::new(
                    attr.path().span(),
                    "only one #[subset_of(...)] attribute allowed per struct",
                )
                .to_compile_error()
                .into();
            }
            let nested = match attr.parse_args::<Type>() {
                Ok(ty) => ty,
                Err(e) => return e.to_compile_error().into(),
            };
            subset_of_parent = Some(nested);
        }
    }

    // Validate struct and extract fields
    let Data::Struct(DataStruct { fields, .. }) = &input.data else {
        return syn::Error::new(
            Span::call_site(),
            "State can only be derived for structs with named fields",
        )
        .to_compile_error()
        .into();
    };

    if fields.is_empty() {
        return syn::Error::new(
            Span::call_site(),
            "State struct must have at least one field",
        )
        .to_compile_error()
        .into();
    }

    let field_count = fields.len();
    if field_count > 64 {
        return syn::Error::new(
            Span::call_site(),
            format!(
                "State has {field_count} fields, exceeds u64 capacity of 64. \
                 Enable 'wide-state' feature to use FixedBitSet-based FieldsChanged."
            ),
        )
        .to_compile_error()
        .into();
    }

    // Collect field info
    let mut field_names = Vec::new();
    let mut field_name_strs = Vec::new();
    let mut field_reducers = Vec::new();
    let mut update_fields = Vec::new();
    let mut field_constant_decls = Vec::new();

    for (idx, field) in fields.iter().enumerate() {
        let Some(field_name) = &field.ident else {
            continue;
        };
        let field_type = &field.ty;

        field_names.push(field_name.clone());
        field_name_strs.push(proc_macro2::Literal::string(&field_name.to_string()));

        let reducer = parse_reducer_attr(field);
        field_reducers.push(reducer);

        update_fields.push(quote! {
            pub #field_name: Option<#field_type>
        });

        let const_name = Ident::new(
            &format!("FIELD_{}", field_name.to_string().to_uppercase()),
            Span::call_site(),
        );
        field_constant_decls.push(quote! {
            pub const #const_name: usize = #idx;
        });
    }

    // Generate apply() body per field based on reducer type
    let apply_arms = generate_apply_arms(&field_names, &field_reducers);
    let try_apply_arms = generate_try_apply_arms(&field_names, &field_reducers);
    let reset_ephemeral_arms = generate_reset_ephemeral_arms(&field_names, &field_reducers);

    // Collect replace field indices for multi-writer detection at the Pregel engine level
    let replace_field_indices: Vec<proc_macro2::TokenStream> = field_names
        .iter()
        .zip(field_reducers.iter())
        .enumerate()
        .filter(|(_, (_, reducer))| matches!(reducer, ReducerType::Replace))
        .map(|(idx, _)| {
            let idx_val = idx;
            quote! { #idx_val }
        })
        .collect();

    // Collect replace_after_finish field indices for finish semantics at the Pregel engine level
    let replace_after_finish_indices: Vec<proc_macro2::TokenStream> = field_names
        .iter()
        .zip(field_reducers.iter())
        .enumerate()
        .filter(|(_, (_, reducer))| matches!(reducer, ReducerType::ReplaceAfterFinish))
        .map(|(idx, _)| {
            let idx_val = idx;
            quote! { #idx_val }
        })
        .collect();

    // Collect ephemeral field indices for consume semantics at the Pregel engine level
    let consume_field_indices: Vec<proc_macro2::TokenStream> = field_names
        .iter()
        .zip(field_reducers.iter())
        .enumerate()
        .filter(|(_, (_, reducer))| matches!(reducer, ReducerType::Ephemeral))
        .map(|(idx, _)| {
            let idx_val = idx;
            quote! { #idx_val }
        })
        .collect();

    // Generate finish_field() match arms for replace_after_finish fields
    let finish_field_arms = generate_finish_field_arms(&field_names, &field_reducers);

    // Generate consume_field() match arms for ephemeral fields
    let consume_field_arms = generate_consume_field_arms(&field_names, &field_reducers);

    // Generate field_is_set match arms for efficient field checking without serialization
    let field_is_set_arms = generate_field_is_set_arms(&field_names);

    // Generate migrate match arms
    let migrate_arms = migrate_functions.iter().map(|(from_ver, func_path)| {
        let next_ver = from_ver + 1;
        quote! {
            #from_ver => {
                let value = #func_path(value);
                <Self as juncture_core::State>::migrate(#next_ver, value)
            }
        }
    });

    let update_name = Ident::new(&format!("{struct_name}Update"), Span::call_site());

    // Generate StateSubset impl if #[subset_of(Parent)] is present
    let subset_impl = subset_of_parent.as_ref().map(|parent_type| {
        generate_subset_impl(struct_name, &input.generics, parent_type, &field_names)
    });

    let expanded = quote! {
        // 1. Generate Update struct
        #[derive(Default, Clone, Debug, serde::Serialize, serde::Deserialize)]
        pub struct #update_name #ty_generics #where_clause {
            #(#update_fields,)*
        }

        // 2. Field index constants + replace field indices + replace_after_finish field indices + field_is_set helper
        impl #impl_generics #struct_name #ty_generics #where_clause {
            #(#field_constant_decls)*

            /// Indices of fields that use the `replace` reducer.
            ///
            /// Used by the Pregel engine to detect multiple writers in a single
            /// superstep before applying any writes.
            #[must_use]
            pub const REPLACE_FIELD_INDICES: &'static [usize] = &[#(#replace_field_indices),*];

            /// Indices of fields that use the `replace_after_finish` reducer.
            ///
            /// Used by the Pregel engine to call `finish_field()` only for
            /// fields that need finish semantics, avoiding unnecessary work.
            #[must_use]
            pub const REPLACE_AFTER_FINISH_FIELD_INDICES: &'static [usize] = &[#(#replace_after_finish_indices),*];

            /// Indices of fields that use the `ephemeral` reducer.
            ///
            /// Used by the Pregel engine to call `consume_field()` only for
            /// fields that need consume semantics, avoiding unnecessary work.
            #[must_use]
            pub const CONSUME_FIELD_INDICES: &'static [usize] = &[#(#consume_field_indices),*];

            /// Check if a specific field is set (Some) in an update.
            ///
            /// Provides efficient field-level inspection without serialization,
            /// used by the Pregel engine for multi-writer conflict detection.
            #[must_use]
            pub fn field_is_set(update: &<Self as juncture_core::State>::Update, field_idx: usize) -> bool {
                match field_idx {
                    #(#field_is_set_arms)*
                    _ => false,
                }
            }
        }

        // 4. State trait implementation
        impl #impl_generics juncture_core::State for #struct_name #ty_generics
        where
            Self: Clone + Send + Sync + std::fmt::Debug + 'static,
        {
            type Update = #update_name #ty_generics;
            type FieldVersions = juncture_core::state::FieldVersions;

            fn apply(&mut self, update: Self::Update) -> juncture_core::FieldsChanged {
                let mut changed = juncture_core::FieldsChanged::default();
                #(#apply_arms)*
                changed
            }

            fn try_apply(
                &mut self,
                update: Self::Update,
            ) -> Result<juncture_core::FieldsChanged, juncture_core::error::InvalidUpdateError> {
                let mut changed = juncture_core::FieldsChanged::default();
                #(#try_apply_arms)*
                Ok(changed)
            }

            fn reset_ephemeral(&mut self) {
                #(#reset_ephemeral_arms)*
            }

            fn schema_version() -> u32 {
                #state_version
            }

            fn migrate(from_version: u32, value: serde_json::Value) -> serde_json::Value {
                if from_version >= Self::schema_version() {
                    return value;
                }
                match from_version {
                    #(#migrate_arms)*
                    _ => value,
                }
            }

            fn replace_field_indices() -> &'static [usize] {
                Self::REPLACE_FIELD_INDICES
            }

            fn replace_after_finish_field_indices() -> &'static [usize] {
                Self::REPLACE_AFTER_FINISH_FIELD_INDICES
            }

            fn consume_field_indices() -> &'static [usize] {
                Self::CONSUME_FIELD_INDICES
            }

            fn finish_field(&mut self, field_idx: usize) {
                match field_idx {
                    #(#finish_field_arms)*
                    _ => {}
                }
            }

            fn consume_field(&mut self, field_idx: usize) {
                match field_idx {
                    #(#consume_field_arms)*
                    _ => {}
                }
            }

            fn field_is_set(update: &Self::Update, field_idx: usize) -> bool {
                Self::field_is_set(update, field_idx)
            }

            fn field_count() -> usize {
                #field_count
            }

            fn field_names() -> &'static [&'static str] {
                &[#(#field_name_strs),* ]
            }
        }

        #subset_impl
    };

    expanded.into()
}

/// Generate `StateSubset<Parent>` implementation for shared-state subgraph mode.
///
/// Constructs the parent Update type name by convention (`{ParentIdent}Update`)
/// so that struct-literal syntax can be used in `map_update()`.
#[allow(
    clippy::needless_pass_by_value,
    reason = "proc-macro code generation requires owning the generics reference"
)]
fn generate_subset_impl(
    struct_name: &Ident,
    generics: &syn::Generics,
    parent_type: &Type,
    field_names: &[Ident],
) -> proc_macro2::TokenStream {
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

    // Build the parent Update type by appending "Update" to the parent type's
    // final path segment ident. This follows the convention established by
    // #[derive(State)] which generates `{StructName}Update`.
    let parent_update_type = build_parent_update_type(parent_type);

    // Extract arms: child_field: parent.child_field.clone()
    let extract_fields = field_names.iter().map(|name| {
        quote! { #name: parent.#name.clone() }
    });

    // map_update arms: child_field: update.child_field (Option<T> pass-through)
    let map_update_fields = field_names.iter().map(|name| {
        quote! { #name: update.#name }
    });

    // Extend the where clause predicates to also require Parent: State.
    // Use only the predicates (not the full WhereClause with its `where` keyword)
    // to avoid emitting a duplicate `where` token in the generated impl block.
    let mut extended_predicates = where_clause
        .map(|wc| wc.predicates.clone())
        .unwrap_or_default();
    extended_predicates.push(syn::parse_quote!(#parent_type: juncture_core::State));

    quote! {
        impl #impl_generics juncture_core::subgraph::StateSubset<#parent_type>
            for #struct_name #ty_generics
        where
            Self: Clone + Send + Sync + std::fmt::Debug + 'static,
            #extended_predicates
        {
            fn extract(parent: &#parent_type) -> Self {
                Self {
                    #(#extract_fields,)*
                }
            }

            fn map_update(update: <Self as juncture_core::State>::Update) -> <#parent_type as juncture_core::State>::Update {
                #parent_update_type {
                    #(#map_update_fields,)*
                    ..Default::default()
                }
            }
        }
    }
}

/// Build the parent Update type path by appending "Update" to the last segment.
///
/// Handles simple types (`ParentState` -> `ParentStateUpdate`), generic types
/// (`ParentState<T>` -> `ParentStateUpdate<T>`), and qualified paths
/// (`module::ParentState` -> `module::ParentStateUpdate`).
fn build_parent_update_type(parent_type: &Type) -> Type {
    let mut ty = parent_type.clone();
    let Type::Path(type_path) = &mut ty else {
        return ty;
    };
    if let Some(last_segment) = type_path.path.segments.last_mut() {
        let ident_str = last_segment.ident.to_string();
        let update_ident = Ident::new(&format!("{ident_str}Update"), last_segment.ident.span());
        last_segment.ident = update_ident;
    }
    ty
}

/// Parse #[reducer(...)] attribute from a field
fn parse_reducer_attr(field: &syn::Field) -> ReducerType {
    for attr in &field.attrs {
        if attr.path().is_ident("reducer")
            && let Meta::List(MetaList { tokens, .. }) = &attr.meta
        {
            let ts: proc_macro::TokenStream = tokens.clone().into();
            if let Ok(parsed) = syn::parse::<ReducerAttr>(ts) {
                return parsed.reducer;
            }
        }
    }
    ReducerType::Replace
}

/// Generate `apply()` match arms for each field
fn generate_apply_arms(
    field_names: &[Ident],
    field_reducers: &[ReducerType],
) -> Vec<proc_macro2::TokenStream> {
    field_names
        .iter()
        .zip(field_reducers.iter())
        .map(|(name, reducer)| {
            let const_name = Ident::new(
                &format!("FIELD_{}", name.to_string().to_uppercase()),
                Span::call_site(),
            );
            match reducer {
                ReducerType::Append => {
                    quote! {
                        if let Some(v) = update.#name {
                            self.#name.extend(v);
                            changed.set_field(Self::#const_name);
                        }
                    }
                }
                ReducerType::Custom(func_path) => {
                    quote! {
                        if let Some(v) = update.#name {
                            #func_path(&mut self.#name, v);
                            changed.set_field(Self::#const_name);
                        }
                    }
                }
                // Replace, Untracked, Ephemeral, ReplaceAfterFinish, LastWriteWins, Any
                // all use simple assignment semantics
                _ => {
                    quote! {
                        if let Some(v) = update.#name {
                            self.#name = v;
                            changed.set_field(Self::#const_name);
                        }
                    }
                }
            }
        })
        .collect()
}

/// Generate `try_apply()` match arms for each field.
///
/// Identical to `apply()` logic but returns `Result<FieldsChanged, InvalidUpdateError>`.
/// Replace fields return `InvalidUpdateError::MultipleOverwrite` if a second write
/// is detected within the same superstep; all other reducer types always succeed.
fn generate_try_apply_arms(
    field_names: &[Ident],
    field_reducers: &[ReducerType],
) -> Vec<proc_macro2::TokenStream> {
    field_names
        .iter()
        .zip(field_reducers.iter())
        .map(|(name, reducer)| {
            let const_name = Ident::new(
                &format!("FIELD_{}", name.to_string().to_uppercase()),
                Span::call_site(),
            );
            match reducer {
                ReducerType::Append => {
                    quote! {
                        if let Some(v) = update.#name {
                            self.#name.extend(v);
                            changed.set_field(Self::#const_name);
                        }
                    }
                }
                ReducerType::Custom(func_path) => {
                    quote! {
                        if let Some(v) = update.#name {
                            #func_path(&mut self.#name, v);
                            changed.set_field(Self::#const_name);
                        }
                    }
                }
                // Replace, Untracked, Ephemeral, ReplaceAfterFinish, LastWriteWins, Any
                // all use simple assignment semantics.
                // Multi-writer detection for Replace fields happens at the
                // Pregel engine level via check_replace_conflicts() which
                // inspects all task outputs before calling try_apply().
                _ => {
                    quote! {
                        if let Some(v) = update.#name {
                            self.#name = v;
                            changed.set_field(Self::#const_name);
                        }
                    }
                }
            }
        })
        .collect()
}

/// Generate `field_is_set()` match arms for efficient field checking
fn generate_field_is_set_arms(field_names: &[Ident]) -> Vec<proc_macro2::TokenStream> {
    field_names
        .iter()
        .enumerate()
        .map(|(idx, name)| {
            let idx_val = idx;
            quote! {
                #idx_val => update.#name.is_some(),
            }
        })
        .collect()
}

/// Generate `reset_ephemeral()` arms
fn generate_reset_ephemeral_arms(
    field_names: &[Ident],
    field_reducers: &[ReducerType],
) -> Vec<proc_macro2::TokenStream> {
    field_names
        .iter()
        .zip(field_reducers.iter())
        .filter(|(_, reducer)| matches!(reducer, ReducerType::Ephemeral))
        .map(|(name, _)| {
            quote! {
                self.#name = Default::default();
            }
        })
        .collect()
}

/// Generate `finish_field()` match arms for `replace_after_finish` fields.
///
/// For `replace_after_finish` fields, the value is already stored in the field
/// by `apply()`. The `finish_field()` call is a lifecycle notification that
/// signals the value is now finalized. Since plain struct fields don't have
/// Channel-level withholding semantics, the match arm simply acknowledges the
/// call (the field value is already correct).
///
/// Fields with other reducer types are omitted from the match arms, falling
/// through to the wildcard `_ => {}` no-op in the generated implementation.
fn generate_finish_field_arms(
    field_names: &[Ident],
    field_reducers: &[ReducerType],
) -> Vec<proc_macro2::TokenStream> {
    field_names
        .iter()
        .zip(field_reducers.iter())
        .enumerate()
        .filter(|(_, (_, reducer))| matches!(reducer, ReducerType::ReplaceAfterFinish))
        .map(|(idx, _)| {
            let idx_val = idx;
            // The field value is already stored by apply(). finish_field()
            // serves as a lifecycle hook for consumers that need to know when
            // the value became finalized (e.g., for Channel integration or
            // checkpoint persistence decisions at a higher level).
            quote! {
                #idx_val => {
                    // replace_after_finish field finalized -- value already in place
                }
            }
        })
        .collect()
}

/// Generate `consume_field()` match arms for `ephemeral` fields.
///
/// For ephemeral fields, the `consume()` call marks the channel's value as
/// consumed after writes have been applied in the superstep. This establishes
/// the consume lifecycle hook point, matching the design spec where all
/// triggered channels call `consume()` after `apply_writes()`.
///
/// Since the state struct stores plain values (not Channel wrappers), the
/// consumed flag tracking happens at the `EphemeralChannel` level. The
/// generated arm serves as the hook that connects the Pregel engine's
/// consume call to the channel layer.
///
/// Fields with other reducer types are omitted from the match arms, falling
/// through to the wildcard `_ => {}` no-op in the generated implementation.
fn generate_consume_field_arms(
    field_names: &[Ident],
    field_reducers: &[ReducerType],
) -> Vec<proc_macro2::TokenStream> {
    field_names
        .iter()
        .zip(field_reducers.iter())
        .enumerate()
        .filter(|(_, (_, reducer))| matches!(reducer, ReducerType::Ephemeral))
        .map(|(idx, _)| {
            let idx_val = idx;
            // The ephemeral field value is already in place after apply_writes().
            // consume_field() marks the channel as consumed; the value will be
            // cleared by the subsequent reset_ephemeral() call.
            quote! {
                #idx_val => {
                    // ephemeral field consumed -- reset_ephemeral will clear the value
                }
            }
        })
        .collect()
}

/// Reducer type parsed from #[reducer(...)] attribute
#[derive(Debug)]
enum ReducerType {
    Replace,
    Append,
    Ephemeral,
    Custom(syn::Path),
    LastWriteWins,
    Untracked,
    ReplaceAfterFinish,
    Any,
}

/// Parser for reducer attribute content
struct ReducerAttr {
    reducer: ReducerType,
}

impl syn::parse::Parse for ReducerAttr {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let ident: syn::Ident = input.parse()?;
        let ident_str = ident.to_string();

        match ident_str.as_str() {
            "replace" => Ok(Self {
                reducer: ReducerType::Replace,
            }),
            "append" => Ok(Self {
                reducer: ReducerType::Append,
            }),
            "ephemeral" => Ok(Self {
                reducer: ReducerType::Ephemeral,
            }),
            "last_write_wins" => Ok(Self {
                reducer: ReducerType::LastWriteWins,
            }),
            "untracked" => Ok(Self {
                reducer: ReducerType::Untracked,
            }),
            "replace_after_finish" => Ok(Self {
                reducer: ReducerType::ReplaceAfterFinish,
            }),
            "any" => Ok(Self {
                reducer: ReducerType::Any,
            }),
            "custom" => {
                input.parse::<syn::Token![=]>()?;
                let func: syn::Path = input.parse()?;
                Ok(Self {
                    reducer: ReducerType::Custom(func),
                })
            }
            _ => Err(syn::Error::new(
                ident.span(),
                format!(
                    "Unknown reducer type: {ident_str}. Expected one of: replace, append, ephemeral, custom = fn, last_write_wins, untracked, replace_after_finish, any"
                ),
            )),
        }
    }
}

// Rust guideline compliant 2026-05-21
