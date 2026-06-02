//! Embedded web server with Langfuse-compatible REST API.
//!
//! Provides an axum-based HTTP server that exposes telemetry data
//! through a Langfuse-compatible API. This allows using the Langfuse
//! frontend UI directly against Juncture's embedded telemetry store.
//!
//! # Endpoints
//!
//! | Method | Path | Description |
//! |--------|------|-------------|
//! | `POST` | `/api/public/ingestion` | Batch ingest traces/observations |
//! | `GET` | `/api/public/traces` | Query traces with filters |
//! | `GET` | `/api/public/traces/:id` | Get single trace with observations |
//! | `GET` | `/api/public/sessions` | Query sessions |
//! | `GET` | `/api/public/sessions/:id` | Get single session with traces |
//! | `GET` | `/api/public/stats/daily` | Daily aggregated statistics |
//! | `GET` | `/` | Dashboard UI |

pub mod api;
pub mod dashboard;

use std::net::SocketAddr;
use std::sync::Arc;

use tokio::net::TcpListener;
use tracing::info;

use crate::trace_store::TraceStore;
use crate::web::api::AuthConfig;

/// Embedded web server for the telemetry dashboard and API.
///
/// Starts an HTTP server that serves the Langfuse-compatible REST API
/// and an embedded dashboard UI. The server runs in a background task
/// and can be stopped by dropping the handle.
///
/// # Examples
///
/// ```ignore
/// use juncture_telemetry::{SqliteStore, web::WebServer};
/// use std::sync::Arc;
///
/// let store = Arc::new(SqliteStore::new("telemetry.db").await?);
/// let server = WebServer::new(store, 8123).start().await?;
/// // Server is running in the background
/// server.stop();
/// ```
pub struct WebServer {
    store: Arc<dyn TraceStore>,
    port: u16,
    bind_ip: [u8; 4],
    auth: AuthConfig,
}

impl std::fmt::Debug for WebServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebServer")
            .field("port", &self.port)
            .finish_non_exhaustive()
    }
}

impl WebServer {
    /// Create a new web server configuration bound to `127.0.0.1` (localhost only).
    #[must_use]
    pub fn new(store: Arc<dyn TraceStore>, port: u16) -> Self {
        Self {
            store,
            port,
            bind_ip: [127, 0, 0, 1],
            auth: AuthConfig::default(),
        }
    }

    /// Set the bind address. Use `[0, 0, 0, 0]` to accept connections from
    /// any network interface (public access).
    ///
    /// # Examples
    ///
    /// ```ignore
    /// // Bind to all interfaces (public access)
    /// let server = WebServer::new(store, 8123)
    ///     .with_bind_addr([0, 0, 0, 0])
    ///     .start()
    ///     .await?;
    /// ```
    #[must_use]
    pub const fn with_bind_addr(mut self, ip: [u8; 4]) -> Self {
        self.bind_ip = ip;
        self
    }

    /// Set Langfuse-compatible authentication credentials.
    ///
    /// When set, the server validates Basic Auth headers on API endpoints.
    /// This allows pointing Langfuse SDK directly at the server:
    ///
    /// ```env
    /// LANGFUSE_SECRET_KEY=sk-lf-...
    /// LANGFUSE_PUBLIC_KEY=pk-lf-...
    /// LANGFUSE_HOST=http://127.0.0.1:8123
    /// ```
    #[must_use]
    pub fn with_auth(mut self, public_key: String, secret_key: String) -> Self {
        self.auth = AuthConfig {
            public_key: Some(public_key),
            secret_key: Some(secret_key),
        };
        self
    }

    /// Start the web server in a background task.
    ///
    /// # Errors
    ///
    /// Returns an error if the TCP listener cannot be bound.
    pub async fn start(self) -> Result<WebServerHandle, std::io::Error> {
        let router = api::create_router_with_auth(self.store, self.auth);
        let addr = SocketAddr::from((self.bind_ip, self.port));
        let listener = TcpListener::bind(addr).await?;
        let actual_addr = listener.local_addr()?;

        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();

        tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async {
                    let _ = shutdown_rx.await;
                })
                .await
                .unwrap_or_else(|e| {
                    tracing::error!("web server error: {e}");
                });
        });

        // Poll until the server is accepting connections.
        // This avoids a race between `axum::serve` reaching `accept()`
        // and the caller issuing the first request.
        for _ in 0..50 {
            if tokio::net::TcpStream::connect(actual_addr).await.is_ok() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        info!(addr = %actual_addr, "telemetry web server started");

        Ok(WebServerHandle {
            addr: actual_addr,
            shutdown_tx: Some(shutdown_tx),
        })
    }
}

/// Handle to a running web server.
///
/// Dropping this handle without calling `stop()` will leave the
/// server running until the process exits.
#[derive(Debug)]
pub struct WebServerHandle {
    /// The address the server is listening on.
    pub addr: SocketAddr,
    shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
}

impl WebServerHandle {
    /// Stop the web server gracefully.
    pub fn stop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
            info!(addr = %self.addr, "telemetry web server stopped");
        }
    }

    /// Get the base URL of the running server.
    #[must_use]
    pub fn base_url(&self) -> String {
        format!("http://{}", self.addr)
    }
}
