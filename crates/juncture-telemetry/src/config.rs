//! Telemetry configuration builder and handle.
//!
//! Provides a `juncture-tracing`-style `init()` builder for one-line
//! telemetry setup with optional Langfuse cloud export and embedded dashboard.
//!
//! # Quick Start
//!
//! ```ignore
//! use juncture_telemetry::init;
//!
//! // Minimal -- SQLite in-memory, no export
//! let telemetry = init().await?;
//!
//! // File persistence + Langfuse cloud + dashboard
//! let telemetry = init()
//!     .with_store("telemetry.db")
//!     .with_langfuse_from_env()
//!     .with_dashboard(8123)
//!     .await?;
//!
//! let collector = telemetry.collector();
//! // ... run your agent ...
//! telemetry.shutdown().await?;
//! ```

use std::sync::Arc;

#[cfg(unix)]
use tokio::signal::unix::{SignalKind, signal};
use tracing::{info, warn};

use crate::batch_writer::BatchWriter;
use crate::collector::TelemetryCollector;
use crate::langfuse::{LangfuseConfig, LangfuseExporter};
use crate::models::CaptureConfig;
use crate::trace_store::{StoreError, TraceStore};

#[cfg(feature = "sqlite")]
use crate::sqlite_store::SqliteStore;

#[cfg(feature = "web")]
use crate::web::WebServer;

/// Builder for telemetry configuration.
///
/// Construct via [`init()`](crate::init).
#[derive(Debug)]
pub struct TelemetryConfig {
    store_path: Option<String>,
    langfuse: Option<LangfuseConfig>,
    dashboard_port: Option<u16>,
    bind_ip: [u8; 4],
    capture_config: CaptureConfig,
}

impl TelemetryConfig {
    /// Create a new configuration with sensible defaults.
    #[must_use]
    pub fn new() -> Self {
        Self {
            store_path: None,
            langfuse: None,
            dashboard_port: None,
            bind_ip: [127, 0, 0, 1],
            capture_config: CaptureConfig::default(),
        }
    }

    /// Set the `SQLite` database file path.
    ///
    /// If not called, an in-memory database is used.
    #[must_use]
    pub fn with_store(mut self, path: impl Into<String>) -> Self {
        self.store_path = Some(path.into());
        self
    }

    /// Enable Langfuse cloud export with explicit credentials.
    #[must_use]
    pub fn with_langfuse(mut self, config: LangfuseConfig) -> Self {
        self.langfuse = Some(config);
        self
    }

    /// Enable Langfuse cloud export by reading credentials from environment.
    ///
    /// Reads:
    /// - `LANGFUSE_PUBLIC_KEY`
    /// - `LANGFUSE_SECRET_KEY`
    /// - `LANGFUSE_BASE_URL` (defaults to `https://cloud.langfuse.com`)
    ///
    /// If any required variable is missing, Langfuse export is silently skipped.
    #[must_use]
    pub fn with_langfuse_from_env(self) -> Self {
        let public_key = std::env::var("LANGFUSE_PUBLIC_KEY").unwrap_or_default();
        let secret_key = std::env::var("LANGFUSE_SECRET_KEY").unwrap_or_default();

        if public_key.is_empty() || secret_key.is_empty() {
            info!("LANGFUSE_PUBLIC_KEY or LANGFUSE_SECRET_KEY not set, skipping Langfuse export");
            return self;
        }

        let base_url = std::env::var("LANGFUSE_BASE_URL")
            .unwrap_or_else(|_| "https://cloud.langfuse.com".to_string());

        self.with_langfuse(LangfuseConfig {
            public_key,
            secret_key,
            base_url,
        })
    }

    /// Enable the embedded web dashboard on the given port.
    #[must_use]
    pub const fn with_dashboard(mut self, port: u16) -> Self {
        self.dashboard_port = Some(port);
        self
    }

    /// Set the bind address for the dashboard server.
    ///
    /// Use `[0, 0, 0, 0]` for public access. Default is `[127, 0, 0, 1]`.
    #[must_use]
    pub const fn with_bind_addr(mut self, ip: [u8; 4]) -> Self {
        self.bind_ip = ip;
        self
    }

    /// Set custom capture configuration.
    #[must_use]
    pub fn with_capture_config(mut self, config: CaptureConfig) -> Self {
        self.capture_config = config;
        self
    }

    /// Build and start all telemetry components.
    ///
    /// Creates the store, collector, optional Langfuse exporter,
    /// and optional dashboard server. Returns a handle that
    /// provides access to the collector and manages shutdown.
    ///
    /// # Errors
    ///
    /// Returns `StoreError` if the database cannot be opened or
    /// the dashboard server cannot start.
    #[cfg(feature = "sqlite")]
    pub async fn install(self) -> Result<TelemetryHandle, StoreError> {
        let store: Arc<dyn TraceStore> = if let Some(ref path) = self.store_path {
            let s = SqliteStore::new(path).await?;
            info!(path = %path, "telemetry SQLite store created");
            Arc::new(s)
        } else {
            let s = SqliteStore::new_memory().await?;
            info!("telemetry in-memory store created");
            Arc::new(s)
        };

        let exporter = self.langfuse.map(|config| {
            let url = config.base_url.clone();
            info!(url = %url, "Langfuse cloud export enabled");
            LangfuseExporter::new(config)
        });

        let writer = if exporter.is_some() {
            BatchWriter::with_config_and_langfuse(Arc::clone(&store), exporter, 50, 5_000)
        } else {
            BatchWriter::new(Arc::clone(&store))
        };

        let collector = TelemetryCollector::from_parts(writer, self.capture_config);
        let server_handle = Self::start_dashboard(self.dashboard_port, self.bind_ip, &store).await;

        Self::spawn_signal_handler(&collector);

        Ok(TelemetryHandle {
            collector,
            server: server_handle,
        })
    }

    #[cfg(feature = "web")]
    async fn start_dashboard(
        port: Option<u16>,
        bind_ip: [u8; 4],
        store: &Arc<dyn TraceStore>,
    ) -> Option<crate::web::WebServerHandle> {
        let port = port?;
        let server = WebServer::new(Arc::clone(store), port).with_bind_addr(bind_ip);
        match server.start().await {
            Ok(h) => {
                info!(url = %h.base_url(), "telemetry dashboard started");
                Some(h)
            }
            Err(e) => {
                warn!("failed to start telemetry dashboard: {e}");
                None
            }
        }
    }

    #[cfg(not(feature = "web"))]
    async fn start_dashboard(
        port: Option<u16>,
        _bind_ip: [u8; 4],
        _store: &Arc<dyn TraceStore>,
    ) -> Option<()> {
        if port.is_some() {
            warn!("dashboard requested but 'web' feature not enabled");
        }
        None
    }

    fn spawn_signal_handler(collector: &TelemetryCollector) {
        let collector_clone = collector.clone();
        tokio::spawn(async move {
            #[cfg(unix)]
            {
                let mut sigterm =
                    signal(SignalKind::terminate()).expect("failed to register SIGTERM handler");
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {},
                    _ = sigterm.recv() => {},
                }
            }
            #[cfg(not(unix))]
            {
                let _ = tokio::signal::ctrl_c().await;
            }
            info!("signal received, flushing telemetry...");
            if let Err(e) = collector_clone.flush().await {
                warn!("telemetry flush on shutdown failed: {e}");
            }
        });
    }
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        Self::new()
    }
}

/// Handle to running telemetry components.
///
/// Provides access to the collector and manages graceful shutdown.
/// Dropping the handle flushes any buffered telemetry data.
pub struct TelemetryHandle {
    collector: TelemetryCollector,
    #[cfg(feature = "web")]
    server: Option<crate::web::WebServerHandle>,
    #[cfg(not(feature = "web"))]
    server: Option<()>,
}

impl std::fmt::Debug for TelemetryHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TelemetryHandle")
            .field("has_dashboard", &self.server.is_some())
            .finish_non_exhaustive()
    }
}

impl TelemetryHandle {
    /// Get a reference to the telemetry collector.
    #[must_use]
    pub const fn collector(&self) -> &TelemetryCollector {
        &self.collector
    }

    /// Get the dashboard base URL, if the dashboard is running.
    #[must_use]
    #[cfg(feature = "web")]
    pub fn dashboard_url(&self) -> Option<String> {
        self.server
            .as_ref()
            .map(crate::web::WebServerHandle::base_url)
    }

    /// Flush all buffered telemetry and stop the dashboard server.
    ///
    /// # Errors
    ///
    /// Returns `StoreError` if the flush fails.
    #[allow(unused_mut, reason = "mut required when web feature is enabled")]
    pub async fn shutdown(mut self) -> Result<(), StoreError> {
        self.collector.flush().await?;
        #[cfg(feature = "web")]
        if let Some(ref mut server) = self.server {
            server.stop();
        }
        Ok(())
    }
}

impl Drop for TelemetryHandle {
    fn drop(&mut self) {
        // Best-effort flush on drop. We cannot await here, so we
        // spawn a task. The signal handler also flushes on ctrl-c.
        let collector = self.collector.clone();
        tokio::spawn(async move {
            let _ = collector.flush().await;
        });
        #[cfg(feature = "web")]
        if let Some(ref mut server) = self.server {
            server.stop();
        }
    }
}
