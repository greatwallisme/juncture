//! Platform-compatible time abstractions.
//!
//! On native targets, re-exports [`std::time::Instant`].
//! On WASM targets, re-exports [`web_time::Instant`] which uses
//! `performance.now()` instead of panicking.

/// Platform-compatible instant type.
///
/// - Native: [`std::time::Instant`]
/// - WASM: [`web_time::Instant`] (uses `performance.now()`)
#[cfg(not(target_family = "wasm"))]
pub use std::time::Instant;

/// Platform-compatible instant type.
///
/// - Native: [`std::time::Instant`]
/// - WASM: [`web_time::Instant`] (uses `performance.now()`)
#[cfg(target_family = "wasm")]
pub use web_time::Instant;
