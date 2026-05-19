//! Message type extensions and utilities for LLM integration.
//!
//! This module re-exports the core message types from `juncture-core` and
//! provides additional utilities for working with messages in LLM contexts.

/// Token usage information from LLM API responses.
///
/// Provides detailed breakdown of token consumption for both input and output.
pub type TokenUsage = juncture_core::state::messages::TokenUsage;

// Rust guideline compliant 2026-05-19
