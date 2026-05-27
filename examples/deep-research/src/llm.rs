//! LLM model builder with middleware chain for observability and resilience.

use juncture::llm::{
    ChatOpenAI, CircuitBreaker, CircuitBreakerConfig, LoggingMiddleware, MiddlewareModel,
};

/// Build a `ChatOpenAI` model with middleware chain.
///
/// This wraps the base model with:
/// - `LoggingMiddleware`: Logs all LLM invocations for observability
/// - `CircuitBreaker`: Prevents cascading failures by blocking unhealthy LLM providers
///
/// # Arguments
///
/// * `api_key` - `OpenAI` API key
/// * `base_url` - Optional base URL for `OpenAI`-compatible API
/// * `model_name` - Model name (e.g., "gpt-4o")
///
/// # Examples
///
/// ```
/// use deep_research::llm::build_model_with_middleware;
///
/// let model = build_model_with_middleware(
///     "sk-...".to_string(),
///     Some("https://api.openai.com/v1".to_string()),
///     "gpt-4o"
/// );
/// ```
#[must_use]
pub fn build_model_with_middleware(
    api_key: String,
    base_url: Option<String>,
    model_name: &str,
) -> MiddlewareModel<ChatOpenAI> {
    // Build base ChatOpenAI model
    let mut model = ChatOpenAI::new(api_key);
    if let Some(base_url) = base_url {
        model = model.with_base_url(base_url);
    }
    model = model.with_model(model_name);

    // Configure circuit breaker for resilience
    // Opens circuit after 3 consecutive failures
    // Waits 60 seconds before transitioning to half-open
    // Allows 1 test call in half-open state
    let circuit_breaker = CircuitBreaker::new(CircuitBreakerConfig {
        failure_threshold: 3,
        recovery_timeout: std::time::Duration::from_secs(60),
        half_open_max_calls: 1,
    });

    // Wrap model with middleware chain
    // Order matters: logging executes first (pre), then circuit breaker (pre/post)
    MiddlewareModel::new(model)
        .with_middleware(LoggingMiddleware::new().with_model_name(model_name))
        .with_middleware(circuit_breaker)
}

// Rust guideline compliant 2026-05-27
