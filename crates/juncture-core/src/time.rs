//! Platform-compatible time abstractions.
//!
//! - Native: re-exports [`std::time::Instant`].
//! - Browser WASM (`wasm32-unknown-unknown`): re-exports [`web_time::Instant`]
//!   which uses `performance.now()` instead of panicking.
//! - WASI (`wasm32-wasip1`/`wasm32-wasip2`): re-exports [`std::time::Instant`]
//!   which works natively via WASI clock APIs.

/// Platform-compatible instant type.
///
/// - Native / WASI: [`std::time::Instant`]
/// - Browser WASM: [`web_time::Instant`] (uses `performance.now()`)
#[cfg(not(all(target_family = "wasm", target_os = "unknown")))]
pub use std::time::Instant;

/// Platform-compatible instant type.
///
/// - Native / WASI: [`std::time::Instant`]
/// - Browser WASM: [`web_time::Instant`] (uses `performance.now()`)
#[cfg(all(target_family = "wasm", target_os = "unknown"))]
pub use web_time::Instant;
