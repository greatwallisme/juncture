
```rust
// main.rs
use anyhow::Result;
use std::sync::Arc;
use axum::{Router, routing::{get, post, put, delete}};
use tower_http::trace::TraceLayer;

// Domain imports
use your_crate::domain::{
    services::AuthorServiceImpl,
    ports::{AuthorService, AuthorRepository, AuthorMetrics, AuthorNotifier, AuthorValidator},
};

// Adapter imports
use your_crate::outbound::sqlite::Sqlite;
use your_crate::outbound::prometheus_metrics::PrometheusMetrics;
use your_crate::outbound::email_notifier::EmailNotifier;
use your_crate::outbound::basic_validator::BasicAuthorValidator;

// Handler imports
use your_crate::inbound::http::handlers::{
    create_author, get_author, update_author, delete_author, list_authors,
};

#[derive(Clone)]
struct AppState {
    author_service: Arc<dyn AuthorService>,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::init();

    // Load configuration
    let config = Config::from_env()?;

    // Initialize adapters (concrete implementations)
    let repository = Sqlite::new(&config.database_url).await?;
    let metrics = PrometheusMetrics::new();
    let notifier = EmailNotifier::new(&config.smtp_config)?;
    let validator = BasicAuthorValidator::new();

    // Wire adapters into service
    let author_service = Arc::new(
        AuthorServiceImpl::new(repository, metrics, notifier, validator)
    );

    // Create application state
    let state = AppState { author_service };

    // Create HTTP router
    let app = Router::new()
        .route("/authors", post(create_author))
        .route("/authors", get(list_authors))
        .route("/authors/:id", get(get_author))
        .route("/authors/:id", put(update_author))
        .route("/authors/:id", delete(delete_author))
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    // Start server
    let listener = tokio::net::TcpListener::bind(&config.server_address).await?;
    tracing::info!("Server listening on {}", config.server_address);

    axum::serve(listener, app).await?;

    Ok(())
}

#[derive(Clone)]
struct Config {
    database_url: String,
    server_address: String,
    smtp_config: SmtpConfig,
}

impl Config {
    fn from_env() -> Result<Self> {
        Ok(Self {
            database_url: std::env::var("DATABASE_URL")
                .unwrap_or_else(|_| "sqlite:app.db".to_string()),
            server_address: std::env::var("SERVER_ADDRESS")
                .unwrap_or_else(|_| "0.0.0.0:3000".to_string()),
            smtp_config: SmtpConfig::from_env()?,
        })
    }
}

#[derive(Clone)]
struct SmtpConfig {
    host: String,
    port: u16,
    username: String,
    password: String,
}

impl SmtpConfig {
    fn from_env() -> Result<Self> {
        Ok(Self {
            host: std::env::var("SMTP_HOST")?,
            port: std::env::var("SMTP_PORT")
                .unwrap_or_else(|_| "587".to_string())
                .parse()?,
            username: std::env::var("SMTP_USERNAME")?,
            password: std::env::var("SMTP_PASSWORD")?,
        })
    }
}
```