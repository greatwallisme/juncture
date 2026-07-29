//! The `#[task(...)]` attribute macro.
//!
//! Transforms an `async fn(...) -> Result<O, E>` into a callable that returns a
//! [`juncture_core::pregel::SyncAsyncFuture<O>`], applying optional `cache`,
//! `retry`, and `timeout` policies. See design `03-pregel-engine` §13.3.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{
    Expr, FnArg, GenericArgument, Ident, ItemFn, PathArguments, ReturnType, Token, Visibility,
    parse::{Parse, ParseStream, Parser},
    punctuated::Punctuated,
    spanned::Spanned,
};

/// A parsed `key = value` pair from the macro attribute.
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

/// The supported `#[task(...)]` arguments (all optional).
#[derive(Default)]
struct TaskArgs {
    cache: Option<Expr>,
    retry: Option<Expr>,
    timeout: Option<Expr>,
    name: Option<Expr>,
}

fn parse_task_args(attr: TokenStream) -> syn::Result<TaskArgs> {
    if attr.is_empty() {
        return Ok(TaskArgs::default());
    }
    let pairs = Punctuated::<KeyVal, Token![,]>::parse_terminated.parse2(attr)?;
    let mut args = TaskArgs::default();
    for kv in pairs {
        match kv.key.to_string().as_str() {
            "cache" => args.cache = Some(kv.value),
            "retry" => args.retry = Some(kv.value),
            "timeout" => args.timeout = Some(kv.value),
            "name" => args.name = Some(kv.value),
            other => {
                return Err(syn::Error::new(
                    kv.key.span(),
                    format!(
                        "unknown #[task] argument `{other}`; expected one of: cache, retry, timeout, name"
                    ),
                ));
            }
        }
    }
    Ok(args)
}

/// Extract the `O` from a `Result<O, E>` return type.
fn extract_ok_type(output: &ReturnType) -> syn::Result<syn::Type> {
    let ReturnType::Type(_, ty) = output else {
        return Err(syn::Error::new(
            proc_macro2::Span::call_site(),
            "#[task] functions must return `Result<O, E>`",
        ));
    };
    let syn::Type::Path(type_path) = ty.as_ref() else {
        return Err(syn::Error::new(
            ty.span(),
            "#[task] return type must be `Result<O, E>`",
        ));
    };
    let last =
        type_path.path.segments.last().ok_or_else(|| {
            syn::Error::new(ty.span(), "#[task] return type must be `Result<O, E>`")
        })?;
    let PathArguments::AngleBracketed(gen_args) = &last.arguments else {
        return Err(syn::Error::new(
            ty.span(),
            "#[task] return type must be `Result<O, E>`",
        ));
    };
    let Some(GenericArgument::Type(ok_ty)) = gen_args.args.first() else {
        return Err(syn::Error::new(
            ty.span(),
            "#[task] return type must be `Result<O, E>`",
        ));
    };
    Ok(ok_ty.clone())
}

/// Collect the patterns of the fn's typed parameters (the macro requires plain
/// free-fn parameters, not `self`).
fn collect_param_patterns(
    inputs: &syn::punctuated::Punctuated<FnArg, Token![,]>,
) -> syn::Result<Vec<syn::Pat>> {
    inputs
        .iter()
        .map(|arg| match arg {
            FnArg::Typed(pat_type) => Ok((*pat_type.pat).clone()),
            FnArg::Receiver(_) => Err(syn::Error::new(
                proc_macro2::Span::call_site(),
                "#[task] is only supported on free functions, not methods (no `self`)",
            )),
        })
        .collect()
}

#[allow(
    clippy::too_many_lines,
    reason = "proc-macro codegen is inherently linear; splitting the attribute-to-code expansion into helpers would obscure the generated output"
)]
pub fn task_impl(attr: TokenStream, item: TokenStream) -> syn::Result<TokenStream> {
    let args = parse_task_args(attr)?;
    let mut func: ItemFn = syn::parse2(item)?;

    if func.sig.asyncness.is_none() {
        return Err(syn::Error::new(
            func.sig.ident.span(),
            "#[task] functions must be `async`",
        ));
    }

    let wrapper_ident = func.sig.ident.clone();
    let inner_ident = format_ident!("{}__task_inner", wrapper_ident);
    let vis = func.vis.clone();
    let generics = func.sig.generics.clone();
    let inputs = func.sig.inputs.clone();
    let where_clause = func.sig.generics.where_clause.clone();
    let ok_ty = extract_ok_type(&func.sig.output)?;
    let patterns = collect_param_patterns(&inputs)?;

    // The namespace used in the cache key: the `name` arg if provided,
    // otherwise the function's own name.
    let namespace_expr: TokenStream = args.name.as_ref().map_or_else(
        || quote! { ::std::string::String::from(stringify!(#wrapper_ident)) },
        |expr| quote! { ::std::string::ToString::to_string(&(#expr)) },
    );

    // Rename the original fn to the private inner fn.
    func.sig.ident = inner_ident.clone();
    func.vis = Visibility::Inherited;
    let inner_fn = func;

    // Patterns for building the args tuple / closure destructure / inner call.
    let tuple_expr = quote! { ( #(#patterns,)* ) };
    let destructure = quote! { ( #(#patterns,)* ) };
    let call_args = quote! { #(#patterns),* };

    let retry_expr = args.retry.as_ref().map_or_else(
        || quote! { ::std::option::Option::None },
        |e| quote! { ::std::option::Option::Some(#e) },
    );
    let timeout_expr = args.timeout.as_ref().map_or_else(
        || quote! { ::std::option::Option::None },
        |e| quote! { ::std::option::Option::Some(#e) },
    );

    // Cache lookup (only when `cache` is configured).
    let cache_lookup = args.cache.as_ref().map_or_else(
        || quote! {},
        |cache_expr| {
            quote! {
                static __TASK_CACHE: ::std::sync::OnceLock<::juncture_core::config::CachePolicy> =
                    ::std::sync::OnceLock::new();
                let __task_namespace = #namespace_expr;
                let __task_key = {
                    let __args_value = ::serde_json::to_value(&#tuple_expr)
                        .unwrap_or(::serde_json::Value::Null);
                    ::juncture_core::func::task_cache_key(&__task_namespace, &__args_value)
                };
                let __task_policy = __TASK_CACHE.get_or_init(|| #cache_expr);
                if let ::std::option::Option::Some(__task_hit) =
                    __task_policy
                        .get(&__task_key)
                        .and_then(|__v| ::serde_json::from_value::<#ok_ty>(__v).ok())
                {
                    return ::juncture_core::pregel::SyncAsyncFuture::ready(__task_hit);
                }
            }
        },
    );

    // Cache store on success (only when `cache` is configured).
    let cache_store = if args.cache.is_some() {
        quote! {
            if let ::std::result::Result::Ok(ref __task_ok) = __task_result {
                if let ::std::result::Result::Ok(__task_value) = ::serde_json::to_value(__task_ok) {
                    __task_policy.put(__task_key, __task_value);
                }
            }
        }
    } else {
        quote! {}
    };

    let expanded = quote! {
        #[allow(
            non_snake_case,
            clippy::unused_async,
            reason = "inner task body generated by the #[task] attribute macro; `async` is required so the function returns a future that the task runtime awaits, even when the body has no direct await"
        )]
        #inner_fn

        #vis fn #wrapper_ident #generics ( #inputs )
            -> ::juncture_core::pregel::SyncAsyncFuture< #ok_ty >
            #where_clause
        {
            #cache_lookup

            ::juncture_core::pregel::SyncAsyncFuture::pending(async move {
                let __task_retry = #retry_expr;
                let __task_timeout = #timeout_expr;
                let __task_args = #tuple_expr;
                let __task_result: ::std::result::Result<#ok_ty, ::juncture_core::JunctureError> =
                    ::juncture_core::func::run_task(
                        __task_retry,
                        __task_timeout,
                        __task_args,
                        |__a| async move {
                            let #destructure = __a;
                            #inner_ident(#call_args).await
                        },
                    )
                    .await;
                #cache_store
                __task_result
            })
        }
    };
    Ok(expanded)
}
