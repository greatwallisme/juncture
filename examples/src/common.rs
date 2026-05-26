//! Shared environment loading and LLM client initialization for real examples.

use juncture::llm::ChatOpenAI;

/// Load `.env` and build a `ChatOpenAI` client from environment variables.
///
/// # Environment variables
///
/// - `OPENAI_API_KEY` (required) -- API key for the OpenAI-compatible provider
/// - `OPENAI_BASE_URL` (optional) -- base URL, defaults to `https://api.openai.com/v1`
/// - `OPENAI_MODEL` (optional) -- model name, defaults to `gpt-4o`
///
/// # Errors
///
/// Returns a human-readable error string if `OPENAI_API_KEY` is missing.
pub fn load_llm() -> Result<ChatOpenAI, String> {
    // Best-effort .env loading -- silently ignores missing file so that
    // examples also work when env vars are set externally (CI, Docker, etc.).
    let _ = dotenvy::dotenv();

    let api_key = std::env::var("OPENAI_API_KEY").map_err(|_err| {
        "OPENAI_API_KEY not set. Copy .env.example to .env and fill in your key.".to_string()
    })?;

    let base_url = std::env::var("OPENAI_BASE_URL")
        .unwrap_or_else(|_| "https://api.openai.com/v1".to_string());
    let model = std::env::var("OPENAI_MODEL").unwrap_or_else(|_| "gpt-4o".to_string());

    let llm = ChatOpenAI::new(api_key)
        .with_base_url(base_url)
        .with_model(model);

    Ok(llm)
}

// Rust guideline compliant 2026-05-26
