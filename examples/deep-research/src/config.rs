//! Research agent configuration loaded from CLI args and environment.

use anyhow::Result;

/// Research agent configuration.
#[derive(Debug, Clone)]
#[allow(
    dead_code,
    reason = "Public API fields may not all be used in current binary"
)]
pub struct ResearchConfig {
    /// `OpenAI` API key
    pub openai_api_key: String,
    /// Optional base URL for `OpenAI`-compatible API
    pub openai_base_url: Option<String>,
    /// LLM model name
    pub model: String,
    /// Maximum agent iterations
    #[allow(dead_code, reason = "Public API field reserved for future use")]
    pub max_iterations: u32,
    /// Optional Tavily API key for web search
    #[allow(dead_code, reason = "Public API field reserved for future use")]
    pub tavily_api_key: Option<String>,
    /// Whether to require approval for dangerous operations.
    #[allow(dead_code, reason = "Public API field reserved for future use")]
    pub require_approval: bool,
}

impl ResearchConfig {
    /// Load configuration from environment variables with CLI overrides.
    ///
    /// # Environment Variables
    ///
    /// - `OPENAI_API_KEY` (required) -- API key for `OpenAI` or compatible endpoint
    /// - `OPENAI_BASE_URL` (optional) -- Custom base URL
    /// - `TAVILY_API_KEY` (optional) -- Tavily search API key
    ///
    /// # Errors
    ///
    /// Returns error if `OPENAI_API_KEY` is not set.
    pub fn from_env(model: &str, max_iterations: u32, require_approval: bool) -> Result<Self> {
        let openai_api_key = std::env::var("OPENAI_API_KEY")
            .map_err(|e| anyhow::anyhow!("OPENAI_API_KEY environment variable is required: {e}"))?;

        let openai_base_url = std::env::var("OPENAI_BASE_URL").ok();
        let tavily_api_key = std::env::var("TAVILY_API_KEY").ok();

        // Use CLI model arg unless it's the default and OPENAI_MODEL is set in env
        let model = if model == "gpt-4o" {
            std::env::var("OPENAI_MODEL").unwrap_or_else(|_| model.to_string())
        } else {
            model.to_string()
        };

        Ok(Self {
            openai_api_key,
            openai_base_url,
            model,
            max_iterations,
            tavily_api_key,
            require_approval,
        })
    }
}

// Rust guideline compliant 2026-05-27

// Rust guideline compliant 2026-05-27
