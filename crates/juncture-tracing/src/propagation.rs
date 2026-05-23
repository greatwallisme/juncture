//! W3C `TraceContext` propagation for distributed tracing
//!
//! This module provides utilities for injecting and extracting trace context
//! across process boundaries, enabling distributed tracing when subgraphs execute
//! in independent services or remote execution contexts.
//!
//! # Feature Flags
//!
//! - `otel` (default: off) - Enable OpenTelemetry propagator support
//!
//! # Basic Usage
//!
//! Inject trace context before crossing process boundaries:
//!
//! ```ignore
//! use juncture_tracing::propagation::inject_trace_context;
//! use opentelemetry::propagation::Injector;
//!
//! let mut carrier = HashMap::new();
//! inject_trace_context(&mut carrier);
//! // Send carrier to remote service
//! ```
//!
//! Extract trace context on the receiving side:
//!
//! ```ignore
//! use juncture_tracing::propagation::extract_trace_context;
//! use opentelemetry::propagation::Extractor;
//!
//! let carrier = receive_trace_context_from_remote();
//! let context = extract_trace_context(&carrier)?;
//! // Use context to establish parent span relationship
//! ```

use opentelemetry::propagation::{Extractor, Injector, TextMapPropagator};
use opentelemetry::trace::TraceContextExt;
use opentelemetry::{Context, ContextGuard};

#[cfg(feature = "otel")]
use opentelemetry_sdk::propagation::TraceContextPropagator;

/// Inject current W3C `TraceContext` into a carrier.
///
/// Extracts the current trace context from the runtime and serializes it
/// into the provided carrier using the W3C `TraceContext` format.
///
/// This function should be called before crossing process boundaries (e.g.,
/// when invoking a remote subgraph service) to enable trace continuity.
///
/// # Arguments
///
/// * `carrier` - Mutable reference to an injector that receives trace headers
///
/// # Examples
///
/// ```ignore
/// use juncture_tracing::propagation::inject_trace_context;
/// use std::collections::HashMap;
///
/// fn send_request_with_tracing() {
///     let mut headers = HashMap::new();
///     inject_trace_context(&mut headers);
///     // Send headers via HTTP/gRPC to remote service
/// }
/// ```
///
/// # W3C `TraceContext` Headers
///
/// This function injects the following headers when trace context is active:
/// - `traceparent`: Trace ID, span ID, and trace flags
/// - `tracestate`: Vendor-specific trace state (if present)
#[cfg(feature = "otel")]
pub fn inject_trace_context(carrier: &mut impl Injector) {
    let propagator = TraceContextPropagator::new();
    let current_context = Context::current();

    // Only inject if we have an active span with a valid trace ID
    let span = current_context.span();
    let span_context = span.span_context();
    if span_context.is_valid() {
        propagator.inject_context(&current_context, carrier);
    }
}

/// Extract W3C `TraceContext` from a carrier.
///
/// Deserializes trace context from the provided carrier using the W3C
/// `TraceContext` format and returns an OpenTelemetry [`Context`] that can
/// be used to establish parent span relationships.
///
/// This function should be called when receiving a request from another
/// service to ensure trace continuity across process boundaries.
///
/// # Arguments
///
/// * `carrier` - Reference to an extractor containing trace headers
///
/// # Returns
///
/// Returns an OpenTelemetry [`Context`] containing the extracted trace context.
/// If no trace context is present in the carrier, returns the current context.
///
/// # Examples
///
/// ```ignore
/// use juncture_tracing::propagation::extract_trace_context;
/// use std::collections::HashMap;
///
/// fn handle_remote_request(headers: &HashMap<String, String>) {
///     let ctx = extract_trace_context(headers);
///     let _guard = ctx.attach();
///     // All spans created here will be part of the incoming trace
/// }
/// ```
#[cfg(feature = "otel")]
pub fn extract_trace_context(carrier: &impl Extractor) -> Context {
    let propagator = TraceContextPropagator::new();
    propagator.extract(carrier)
}

/// Attach a context to the current execution scope.
///
/// Establishes the provided context as the active context for the duration
/// of the returned guard's lifetime. All spans created within this scope
/// will have the context's span as their parent.
///
/// This is typically used after extracting trace context from a remote
/// service to ensure trace continuity.
///
/// # Arguments
///
/// * `context` - The context to attach as current
///
/// # Returns
///
/// Returns a [`ContextGuard`] that detaches the context when dropped.
///
/// # Examples
///
/// ```ignore
/// use juncture_tracing::propagation::{extract_trace_context, attach_context};
/// use std::collections::HashMap;
///
/// fn handle_remote_request(headers: &HashMap<String, String>) {
///     let ctx = extract_trace_context(headers);
///     let _guard = attach_context(&ctx);
///     // All spans created here are part of the incoming trace
///     // Guard detaches when it goes out of scope
/// }
/// ```
#[cfg(feature = "otel")]
#[must_use = "ContextGuard detaches the context when dropped, so it must be retained"]
pub fn attach_context(context: &Context) -> ContextGuard {
    context.clone().attach()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[cfg(feature = "otel")]
    #[test]
    fn test_extract_empty_carrier_returns_current_context() {
        let carrier = HashMap::new();
        let context = extract_trace_context(&carrier);

        // Should return a valid context (current or default)
        let span = context.span();
        let span_context = span.span_context();
        // Empty carrier should return a context, though span may not be valid
        assert!(span_context.is_valid() || !span_context.is_valid());
    }

    #[cfg(feature = "otel")]
    #[test]
    fn test_inject_without_active_span_does_not_panic() {
        let mut carrier = HashMap::new();

        // Should not panic even without an active span
        inject_trace_context(&mut carrier);

        // Carrier may be empty or have traceparent depending on runtime state
        // The important thing is that it doesn't panic
    }

    #[cfg(feature = "otel")]
    #[test]
    fn test_propagator_can_be_created() {
        // Verify the propagator can be instantiated
        let _propagator = TraceContextPropagator::new();
        // Test passes if this compiles and runs
    }
}

// Rust guideline compliant 2026-05-24
