//! Deep Research: Multi-agent research assistant powered by Juncture
//!
//! This CLI application uses a `ReAct` agent pattern with multiple tools
//! (web search, calculator, file I/O) to answer complex research questions.
//!
//! # Environment
//!
//! - `OPENAI_API_KEY` (required) -- `OpenAI` API key or compatible endpoint
//! - `OPENAI_BASE_URL` (optional) -- base URL for API requests
//! - `TAVILY_API_KEY` (optional) -- Tavily search API key for `web_search` tool
//!
//! # Examples
//!
//! ```bash
//! # Basic research query
//! cargo run -p deep-research -- "What is the current state of quantum computing?"
//!
//! # With custom model
//! cargo run -p deep-research -- --model gpt-4o-mini "Explain recent AI breakthroughs"
//!
//! # With verbose logging
//! cargo run -p deep-research -- --verbose "Research topic here"
//! ```

use anyhow::Result;
use clap::Parser;
use std::io::Write;
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

mod config;
mod llm;
mod memory;
mod orchestrator;
mod state;
mod tools;

use config::ResearchConfig;
use juncture_telemetry::init as telemetry_init;

/// Multi-agent research assistant powered by Juncture
#[derive(Parser, Debug)]
#[command(name = "deep-research")]
#[command(about = "Research assistant using web search, calculator, and file tools", long_about = None)]
struct Args {
    /// Research question to investigate
    #[arg(value_name = "QUERY")]
    query: String,

    /// LLM model name (default: gpt-4o)
    #[arg(long)]
    #[arg(default_value = "gpt-4o")]
    model: String,

    /// Enable debug logging
    #[arg(short, long)]
    verbose: bool,

    /// Maximum agent iterations (default: 10)
    #[arg(long)]
    #[arg(default_value = "10")]
    max_iterations: u32,

    /// Require approval for dangerous operations (file access)
    #[arg(long)]
    require_approval: bool,

    /// Thread ID for session persistence (checkpointing)
    #[arg(long)]
    thread_id: Option<String>,

    /// Enable embedded telemetry dashboard (<http://127.0.0.1:8123>)
    #[arg(long)]
    telemetry: bool,

    /// Telemetry dashboard port (default: 8123)
    #[arg(long, default_value = "8123")]
    telemetry_port: u16,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Load .env file (best-effort, ignores missing file)
    let _ = dotenvy::dotenv();

    // Initialize tracing subscriber
    let filter = if args.verbose {
        EnvFilter::new("debug")
    } else {
        EnvFilter::builder()
            .with_default_directive("info".parse()?)
            .with_env_var("RUST_LOG")
            .from_env_lossy()
    };

    tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer())
        .init();

    tracing::info!("Starting deep research agent");

    // Initialize telemetry if requested
    let telemetry_handle = if args.telemetry {
        let handle = telemetry_init()
            .with_store("deep-research-telemetry.db")
            .with_langfuse_from_env()
            .with_dashboard(args.telemetry_port)
            .install()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to initialize telemetry: {e}"))?;
        if let Some(url) = handle.dashboard_url() {
            tracing::info!(url = %url, "Telemetry dashboard started");
        }
        Some(handle)
    } else {
        None
    };

    // Build config from environment and CLI args
    let config = ResearchConfig::from_env(&args.model, args.max_iterations, args.require_approval)?;

    tracing::info!(
        query = %args.query,
        model = %config.model,
        max_iterations = args.max_iterations,
        require_approval = args.require_approval,
        thread_id = ?args.thread_id,
        "Configuration"
    );

    // Run the research orchestrator
    let result =
        orchestrator::run_research(&config, &args.query, args.thread_id.as_deref()).await?;

    // Display the research result to stdout
    std::io::stdout().write_all(result.as_bytes())?;
    std::io::stdout().write_all(b"\n")?;

    // Flush telemetry before exit
    if let Some(handle) = telemetry_handle {
        let _ = handle.shutdown().await;
    }

    Ok(())
}

// Rust guideline compliant 2026-05-27
