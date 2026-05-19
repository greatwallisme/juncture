use proc_macro2::{Ident, Span};
use quote::quote;
use syn::{Data, DataStruct, DeriveInput, Meta, MetaList};

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
    let mut field_reducers = Vec::new();
    let mut update_fields = Vec::new();
    let mut version_fields = Vec::new();
    let mut field_constant_decls = Vec::new();

    for (idx, field) in fields.iter().enumerate() {
        let Some(field_name) = &field.ident else {
            continue;
        };
        let field_type = &field.ty;

        field_names.push(field_name.clone());

        let reducer = parse_reducer_attr(field);
        field_reducers.push(reducer);

        update_fields.push(quote! {
            pub #field_name: Option<#field_type>
        });

        version_fields.push(quote! {
            pub #field_name: u64
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
    let reset_ephemeral_arms = generate_reset_ephemeral_arms(&field_names, &field_reducers);

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
    let versions_name = Ident::new(&format!("{struct_name}FieldVersions"), Span::call_site());

    let expanded = quote! {
        // 1. Generate Update struct
        #[derive(Default, Clone, Debug, serde::Serialize, serde::Deserialize)]
        pub struct #update_name #ty_generics #where_clause {
            #(#update_fields,)*
        }

        // 2. Generate FieldVersions struct
        #[derive(Default, Clone, Debug, serde::Serialize, serde::Deserialize)]
        pub struct #versions_name #ty_generics #where_clause {
            #(#version_fields,)*
        }

        // 3. Field index constants
        impl #impl_generics #struct_name #ty_generics #where_clause {
            #(#field_constant_decls)*
        }

        // 4. State trait implementation
        impl #impl_generics juncture_core::State for #struct_name #ty_generics
        where
            Self: Clone + Send + Sync + std::fmt::Debug + 'static,
        {
            type Update = #update_name #ty_generics;
            type FieldVersions = #versions_name #ty_generics;

            fn apply(&mut self, update: Self::Update) -> juncture_core::FieldsChanged {
                let mut changed = juncture_core::FieldsChanged::default();
                #(#apply_arms)*
                changed
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
        }
    };

    expanded.into()
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

// Rust guideline compliant 2025-01-18
