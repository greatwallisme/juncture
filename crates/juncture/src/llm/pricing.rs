//! Model pricing information and cost tracking.
//!
//! Provides pricing data for common LLM models and utilities for calculating
//! costs based on token usage.

use std::collections::HashMap;

use crate::llm::TokenUsage;

/// Trait for models with known pricing information.
///
/// Allows calculating the cost of API calls based on token usage.
///
/// # Example
///
/// ```rust
/// use juncture::llm::{ModelPricing, TokenUsage};
///
/// # struct MyModel;
/// # impl ModelPricing for MyModel {
/// #     fn input_price_per_mtok(&self) -> f64 { 3.0 }
/// #     fn output_price_per_mtok(&self) -> f64 { 15.0 }
/// # }
/// # fn example() {
/// let model = MyModel;
/// let usage = TokenUsage {
///     input_tokens: 1000,
///     output_tokens: 500,
///     total_tokens: 1500,
/// };
///
/// let cost = model.cost_for_usage(&usage);
/// # }
/// ```
pub trait ModelPricing {
    /// Input price per million tokens in USD.
    fn input_price_per_mtok(&self) -> f64;

    /// Output price per million tokens in USD.
    fn output_price_per_mtok(&self) -> f64;

    /// Calculate the cost for a given token usage.
    ///
    /// Returns the total cost in USD based on input and output tokens.
    ///
    /// # Example
    ///
    /// ```rust
    /// use juncture::llm::{ModelPricing, TokenUsage};
    ///
    /// # struct MyModel;
    /// # impl ModelPricing for MyModel {
    /// #     fn input_price_per_mtok(&self) -> f64 { 3.0 }
    /// #     fn output_price_per_mtok(&self) -> f64 { 15.0 }
    /// # }
    /// # fn example() {
    /// let model = MyModel;
    /// let usage = TokenUsage {
    ///     input_tokens: 1_000_000,
    ///     output_tokens: 500_000,
    ///     total_tokens: 1_500_000,
    /// };
    ///
    /// let cost = model.cost_for_usage(&usage);
    /// // Cost = (1M * $3.0) + (0.5M * $15.0) = $10.50
    /// assert_eq!(cost, 10.50);
    /// # }
    /// ```
    fn cost_for_usage(&self, usage: &TokenUsage) -> f64 {
        #[expect(
            clippy::cast_precision_loss,
            reason = "token counts are precise but float is acceptable for pricing"
        )]
        let input_cost = usage.input_tokens as f64 * self.input_price_per_mtok() / 1_000_000.0;
        #[expect(
            clippy::cast_precision_loss,
            reason = "token counts are precise but float is acceptable for pricing"
        )]
        let output_cost = usage.output_tokens as f64 * self.output_price_per_mtok() / 1_000_000.0;
        input_cost + output_cost
    }
}

/// Pricing table for common LLM models.
///
/// Provides pricing information for popular models from various providers.
/// Prices are in USD per million tokens as of 2025.
///
/// # Example
///
/// ```rust
/// use juncture::llm::PricingTable;
///
/// # fn example() {
/// let pricing = PricingTable::default();
/// if let Some((input_price, output_price)) = pricing.get("claude-3-opus-20240229") {
///     // Use pricing information
///     let _ = (input_price, output_price);
/// }
/// # }
/// ```
#[derive(Clone, Debug)]
pub struct PricingTable {
    prices: HashMap<String, (f64, f64)>,
}

impl PricingTable {
    /// Create a new pricing table with default values.
    #[must_use]
    pub fn new() -> Self {
        let mut prices = HashMap::new();

        // Anthropic Claude pricing (as of 2025)
        prices.insert("claude-3-opus-20240229".to_string(), (15.0, 75.0));
        prices.insert("claude-3-sonnet-20240229".to_string(), (3.0, 15.0));
        prices.insert("claude-3-haiku-20240307".to_string(), (0.25, 1.25));
        prices.insert("claude-3-5-sonnet-20241022".to_string(), (3.0, 15.0));
        prices.insert("claude-3-5-sonnet-20240620".to_string(), (3.0, 15.0));

        // OpenAI GPT pricing (as of 2025)
        prices.insert("gpt-4o-2024-11-20".to_string(), (2.5, 10.0));
        prices.insert("gpt-4o-2024-08-06".to_string(), (2.5, 10.0));
        prices.insert("gpt-4o-mini-2024-07-18".to_string(), (0.15, 0.60));
        prices.insert("gpt-4-turbo-2024-04-09".to_string(), (10.0, 30.0));
        prices.insert("gpt-4-0125-preview".to_string(), (10.0, 30.0));
        prices.insert("gpt-4-1106-preview".to_string(), (10.0, 30.0));
        prices.insert("gpt-35-turbo-0125".to_string(), (0.5, 1.5));
        prices.insert("gpt-35-turbo-1106".to_string(), (0.5, 1.5));
        prices.insert("gpt-35-turbo-16k-0613".to_string(), (0.3, 0.6));

        // Google Gemini pricing (as of 2025)
        prices.insert("gemini-2.0-flash-exp".to_string(), (0.075, 0.30));
        prices.insert("gemini-1.5-pro-002".to_string(), (1.25, 5.0));
        prices.insert("gemini-1.5-flash-002".to_string(), (0.075, 0.30));
        prices.insert("gemini-1.5-flash-8b-001".to_string(), (0.0375, 0.15));

        Self { prices }
    }

    /// Get pricing for a specific model.
    ///
    /// Returns `Some((input_price, output_price))` if the model is found,
    /// or `None` if pricing is not available.
    ///
    /// # Arguments
    ///
    /// * `model` - Model name (e.g., "claude-3-opus-20240229")
    ///
    /// # Returns
    ///
    /// * `Some((input, output))` - Prices per million tokens in USD
    /// * `None` - Model not found in pricing table
    #[must_use]
    pub fn get(&self, model: &str) -> Option<(f64, f64)> {
        self.prices.get(model).copied()
    }

    /// Add or update pricing for a model.
    ///
    /// # Arguments
    ///
    /// * `model` - Model name
    /// * `input_price` - Input price per million tokens in USD
    /// * `output_price` - Output price per million tokens in USD
    pub fn insert(&mut self, model: impl Into<String>, input_price: f64, output_price: f64) {
        self.prices
            .insert(model.into(), (input_price, output_price));
    }

    /// Check if pricing is available for a model.
    #[must_use]
    pub fn contains(&self, model: &str) -> bool {
        self.prices.contains_key(model)
    }
}

impl Default for PricingTable {
    fn default() -> Self {
        Self::new()
    }
}

// Rust guideline compliant 2026-05-19
