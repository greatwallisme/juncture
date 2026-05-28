//! WASM-compatible tracing helpers.
//!
//! On WASM, `tracing::info_span!()` and `Instrumented::poll()` call
//! `std::time::Instant::now()` which panics. This module provides
//! `cfg`-gated replacements that completely avoid `Instant::now()`.

/// A wrapper around `tracing::Span` that makes `.instrument()` a no-op.
///
/// On WASM, `info_span!()` returns `WasmSpan(Span::none())`. When
/// `.instrument(wasm_span)` is called, it returns the future unchanged
/// instead of wrapping it in `Instrumented<F>` (which calls `Instant::now()`).
#[cfg(target_family = "wasm")]
pub struct WasmSpan(pub tracing::Span);

/// Create a span that is a no-op on WASM.
///
/// On WASM: returns `WasmSpan(Span::none())` - `.instrument()` becomes a no-op.
/// On native: delegates to `tracing::info_span!()`.
#[cfg(target_family = "wasm")]
#[macro_export]
macro_rules! info_span {
    ($($args:tt)*) => {
        $crate::tracing_wasm::WasmSpan(tracing::Span::none())
    };
}

/// Create a span (native path).
///
/// On native: delegates to `tracing::info_span!()`.
#[cfg(not(target_family = "wasm"))]
#[macro_export]
macro_rules! info_span {
    ($($args:tt)*) => {
        tracing::info_span!($($args)*)
    };
}

/// Extension trait that makes `.instrument(WasmSpan)` a no-op on WASM.
///
/// This avoids creating `Instrumented<F>` which calls `Instant::now()`.
#[cfg(target_family = "wasm")]
pub trait WasmInstrument: std::future::Future + Sized {
    fn instrument(self, _span: WasmSpan) -> Self {
        self
    }
}

#[cfg(target_family = "wasm")]
impl<F: std::future::Future> WasmInstrument for F {}
