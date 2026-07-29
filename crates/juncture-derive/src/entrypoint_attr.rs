//! The `#[entrypoint(...)]` attribute macro.
//!
//! Marks an `async fn(&S) -> Result<S::Update, JunctureError>` as a functional
//! API entrypoint and generates a `compile()` accessor that builds a
//! `CompiledGraph` via `compile_entrypoint_with_config`, wiring the `cache`,
//! `retry`, `timeout`, and `name` args into a `TaskConfig`. See design
//! `03-pregel-engine` §13.3.
//!
//! The caller supplies the `S`/`I`/`O` type parameters (as with
//! `compile_entrypoint::<S, I, O, _>` today).

use proc_macro2::TokenStream;
use quote::quote;
use syn::{
    Expr, FnArg, Ident, ItemFn, Token,
    parse::{Parse, ParseStream, Parser},
    punctuated::Punctuated,
    spanned::Spanned,
};

struct KeyVal {
    key: Ident,
    value: Expr,
}

impl Parse for KeyVal {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let key: Ident = input.parse()?;
        let _eq: Token![=] = input.parse()?;
        let value: Expr = input.parse()?;
        Ok(Self { key, value })
    }
}

#[derive(Default)]
struct EntrypointArgs {
    checkpointer: Option<Expr>,
    cache: Option<Expr>,
    retry: Option<Expr>,
    timeout: Option<Expr>,
    name: Option<Expr>,
}

fn parse_entrypoint_args(attr: TokenStream) -> syn::Result<EntrypointArgs> {
    if attr.is_empty() {
        return Ok(EntrypointArgs::default());
    }
    let pairs = Punctuated::<KeyVal, Token![,]>::parse_terminated.parse2(attr)?;
    let mut args = EntrypointArgs::default();
    for kv in pairs {
        match kv.key.to_string().as_str() {
            "checkpointer" => args.checkpointer = Some(kv.value),
            "cache" => args.cache = Some(kv.value),
            "retry" => args.retry = Some(kv.value),
            "timeout" => args.timeout = Some(kv.value),
            "name" => args.name = Some(kv.value),
            other => {
                return Err(syn::Error::new(
                    kv.key.span(),
                    format!(
                        "unknown #[entrypoint] argument `{other}`; expected one of: checkpointer, cache, retry, timeout, name"
                    ),
                ));
            }
        }
    }
    Ok(args)
}

pub fn entrypoint_impl(attr: TokenStream, item: TokenStream) -> syn::Result<TokenStream> {
    let args = parse_entrypoint_args(attr)?;
    let func: ItemFn = syn::parse2(item)?;
    let fn_ident = func.sig.ident.clone();
    let vis = func.vis.clone();
    let state_ty = extract_state_type(&func.sig.inputs)?;

    let retry_field = args.retry.as_ref().map_or_else(
        || quote! { ::std::option::Option::None },
        |e| quote! { ::std::option::Option::Some(#e) },
    );
    let cache_field = args.cache.as_ref().map_or_else(
        || quote! { ::std::option::Option::None },
        |e| quote! { ::std::option::Option::Some(#e) },
    );
    let timeout_field = args.timeout.as_ref().map_or_else(
        || quote! { ::std::option::Option::None },
        |e| quote! { ::std::option::Option::Some(#e) },
    );
    let name_field = args.name.as_ref().map_or_else(
        || quote! { ::std::option::Option::Some(::std::string::String::from(stringify!(#fn_ident))) },
        |e| quote! { ::std::option::Option::Some(::std::string::ToString::to_string(&(#e))) },
    );

    let task_config = quote! {
        ::juncture_core::config::TaskConfig {
            retry_policy: #retry_field,
            cache_policy: #cache_field,
            timeout: #timeout_field,
            name: #name_field,
        }
    };

    // When a checkpointer is supplied via the attribute, `compile()` takes no
    // checkpointer parameter; otherwise the caller provides one.
    let (compile_sig, checkpointer_arg) = args.checkpointer.as_ref().map_or_else(
        || {
            (
                quote! {
                    (checkpointer: ::std::option::Option<
                        ::std::sync::Arc<dyn ::juncture_core::checkpoint::CheckpointSaver>,
                    >)
                },
                quote! { checkpointer },
            )
        },
        |cp_expr| {
            (
                quote! { () },
                quote! { ::std::option::Option::Some(#cp_expr) },
            )
        },
    );

    Ok(quote! {
        #[allow(
            clippy::unused_async,
            reason = "entrypoint node function; `async` is required so the function returns a future the engine spawns and awaits, even when the body has no direct await"
        )]
        #func

        /// Compile this `#[entrypoint]` into an executable graph.
        ///
        /// Returns a `CompiledGraph<S, S, S>` (input and output are the state
        /// type). `S` must be `Clone` so the node body can own a snapshot of
        /// the state in its ( `'static`) future.
        ///
        /// # Errors
        ///
        /// Returns [`juncture_core::graph::TopologyError`] if the function
        /// cannot be turned into a node or the topology is invalid.
        #vis fn compile #compile_sig
            -> ::std::result::Result<
                ::juncture_core::graph::CompiledGraph<#state_ty, #state_ty, #state_ty>,
                ::juncture_core::graph::TopologyError,
            >
        {
            ::juncture_core::func::compile_entrypoint_with_config::<#state_ty, #state_ty, #state_ty, _>(
                ::juncture_core::node::NodeFnUpdate(
                    |__state: &#state_ty| {
                        // Snapshot the borrowed state so the returned future is
                        // `'static` (the engine spawns node futures).
                        let __owned = ::std::clone::Clone::clone(__state);
                        async move { #fn_ident(&__owned).await }
                    },
                ),
                &#task_config,
                #checkpointer_arg,
            )
        }
    })
}

/// Extract the state type `S` from the entrypoint fn's first parameter (`&S`).
fn extract_state_type(inputs: &Punctuated<FnArg, Token![,]>) -> syn::Result<syn::Type> {
    let first = inputs.first().ok_or_else(|| {
        syn::Error::new(
            proc_macro2::Span::call_site(),
            "#[entrypoint] fn must take `&S` as its first parameter",
        )
    })?;
    let FnArg::Typed(pt) = first else {
        return Err(syn::Error::new(
            proc_macro2::Span::call_site(),
            "#[entrypoint] is only supported on free functions, not methods (no `self`)",
        ));
    };
    let syn::Type::Reference(type_ref) = &*pt.ty else {
        return Err(syn::Error::new(
            pt.ty.span(),
            "#[entrypoint] fn's first parameter must be `&S` (a shared reference to the state)",
        ));
    };
    Ok(*type_ref.elem.clone())
}
