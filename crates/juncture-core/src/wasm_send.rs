//! WASM-compatible `Send` wrappers.
//!
//! On WASM (`wasm32-unknown-unknown`), many types (e.g., `reqwest::Response`)
//! are `!Send` because they contain `Rc<RefCell<...>>` internally. However,
//! WASM is single-threaded, so `Send` is trivially satisfied at runtime.
//!
//! This module provides wrappers that make `!Send` types and futures `Send`
//! on WASM. On native targets, these are no-ops.
//!
//! # Safety
//!
//! This is sound on WASM because:
//! - WASM is single-threaded (no actual thread safety concerns)
//! - `Send` is only a compile-time check
//! - The wrappers are never used to actually send data across threads

/// Wrapper that makes any type `Send` on WASM.
///
/// On native targets, `WasmSend<T>` is `Send` only when `T: Send`.
/// On WASM, `WasmSend<T>` is always `Send` (single-threaded).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct WasmSend<T>(pub T);

// SAFETY: On WASM (single-threaded), Send is trivially satisfied.
#[cfg(target_family = "wasm")]
unsafe impl<T> Send for WasmSend<T> {}

impl<T> WasmSend<T> {
    /// Create a new wrapper.
    pub const fn new(value: T) -> Self {
        Self(value)
    }

    /// Unwrap the inner value.
    pub fn into_inner(self) -> T {
        self.0
    }

    /// Get a reference to the inner value.
    pub const fn inner(&self) -> &T {
        &self.0
    }

    /// Get a mutable reference to the inner value.
    #[allow(
        clippy::missing_const_for_fn,
        reason = "&mut self in const fn is unstable"
    )]
    pub fn inner_mut(&mut self) -> &mut T {
        &mut self.0
    }
}

impl<T> std::ops::Deref for WasmSend<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> std::ops::DerefMut for WasmSend<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

/// Wrap a future to be `Send` on WASM.
///
/// On WASM, wraps the future in a `Send` wrapper (single-threaded safety).
/// On native, returns the future as-is (must already be `Send`).
///
/// # Safety
///
/// On WASM, this is sound because WASM is single-threaded.
/// On native, the future must already be `Send`.
#[cfg(target_family = "wasm")]
pub fn force_send<F: std::future::Future>(
    future: F,
) -> impl std::future::Future<Output = F::Output> + Send {
    ForceSend(future)
}

/// On native, just return the future (must already be `Send`).
#[cfg(not(target_family = "wasm"))]
pub fn force_send<F: std::future::Future + Send>(
    future: F,
) -> impl std::future::Future<Output = F::Output> + Send {
    future
}

/// Wrapper that makes any future `Send` on WASM.
///
/// # Safety
///
/// On WASM (single-threaded), `Send` is trivially satisfied.
/// The future is never actually sent across threads.
#[cfg(target_family = "wasm")]
struct ForceSend<F>(F);

#[cfg(target_family = "wasm")]
// SAFETY: On WASM (single-threaded), Send is trivially satisfied.
unsafe impl<F> Send for ForceSend<F> {}

#[cfg(target_family = "wasm")]
impl<F: std::future::Future> std::future::Future for ForceSend<F> {
    type Output = F::Output;

    fn poll(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        // SAFETY: ForceSend is a structural pin wrapper.
        // We never move the inner future.
        unsafe { self.map_unchecked_mut(|s| &mut s.0).poll(cx) }
    }
}

/// Wrap a stream to be `Send` on WASM.
///
/// On WASM, wraps the stream in a `Send` wrapper (single-threaded safety).
/// On native, returns the stream as-is (must already be `Send`).
#[cfg(target_family = "wasm")]
pub fn force_send_stream<S: futures::Stream>(
    stream: S,
) -> impl futures::Stream<Item = S::Item> + Send {
    ForceSendStream(stream)
}

/// On native, just return the stream (must already be `Send`).
#[cfg(not(target_family = "wasm"))]
pub fn force_send_stream<S: futures::Stream + Send>(
    stream: S,
) -> impl futures::Stream<Item = S::Item> + Send {
    stream
}

/// Wrapper that makes any stream `Send` on WASM.
#[cfg(target_family = "wasm")]
struct ForceSendStream<S>(S);

#[cfg(target_family = "wasm")]
// SAFETY: On WASM (single-threaded), Send is trivially satisfied.
unsafe impl<S> Send for ForceSendStream<S> {}

#[cfg(target_family = "wasm")]
impl<S: futures::Stream> futures::Stream for ForceSendStream<S> {
    type Item = S::Item;

    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        // SAFETY: ForceSendStream is a structural pin wrapper.
        unsafe { self.map_unchecked_mut(|s| &mut s.0).poll_next(cx) }
    }
}
