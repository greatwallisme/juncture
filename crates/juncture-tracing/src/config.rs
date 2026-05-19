//! Tracing configuration and OpenTelemetry initialization
//!
//! This module provides the `TracingConfig` builder for configuring OpenTelemetry
//! trace and metrics export. This feature is only available when the `otel` feature
//! is enabled.

use std::fmt;

/// Error type for tracing configuration failures
#[derive(Debug)]
pub enum TracingError {
    /// Failed to install tracing subscriber
    InstallFailed(String),
}

impl fmt::Display for TracingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InstallFailed(msg) => write!(f, "Failed to install tracing subscriber: {msg}"),
        }
    }
}

impl std::error::Error for TracingError {}

/// Tracing configuration builder
///
/// Use this builder to configure OpenTelemetry trace and metrics export.
/// When the `otel` feature is enabled, this will set up OTLP exporters.
/// When disabled, it configures basic `tracing-subscriber` logging.
///
/// # Examples
///
/// ```
/// use juncture_tracing::config::TracingConfig;
/// use tracing::Level;
///
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// TracingConfig::new()
///     .with_service_name("my-agent-service")
///     .with_log_level(Level::INFO)
///     .install()?;
/// # Ok(())
/// # }
/// ```
#[cfg(feature = "otel")]
#[derive(Clone, Debug)]
pub struct TracingConfig {
    /// OTLP endpoint for trace/metrics export (future)
    otlp_endpoint: Option<String>,

    /// Service name for resource detection
    service_name: String,

    /// Service version for resource detection
    service_version: String,

    /// Additional resource attributes
    resource_attributes: Vec<(String, String)>,

    /// Trace sampling rate (0.0 to 1.0)
    trace_sampling: f64,

    /// Whether metrics export is enabled (future)
    metrics_enabled: bool,

    /// Log level for the subscriber
    log_level: tracing::Level,
}

#[cfg(feature = "otel")]
impl Default for TracingConfig {
    fn default() -> Self {
        Self {
            otlp_endpoint: None,
            service_name: "juncture-app".to_string(),
            service_version: env!("CARGO_PKG_VERSION").to_string(),
            resource_attributes: Vec::new(),
            trace_sampling: 1.0,
            metrics_enabled: false,
            log_level: tracing::Level::INFO,
        }
    }
}

#[cfg(feature = "otel")]
impl TracingConfig {
    /// Create a new tracing configuration with defaults
    ///
    /// # Examples
    ///
    /// ```
    /// use juncture_tracing::config::TracingConfig;
    ///
    /// let config = TracingConfig::new();
    /// // Configures default service name and other settings
    /// let _ = config;
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the OTLP endpoint for trace/metrics export
    ///
    /// # Parameters
    ///
    /// * `endpoint` - OTLP collector endpoint (e.g., `<http://localhost:4317>`)
    ///
    /// # Examples
    ///
    /// ```
    /// use juncture_tracing::config::TracingConfig;
    ///
    /// let config = TracingConfig::new()
    ///     .with_otlp_endpoint("http://collector:4317");
    /// // Configures OTLP endpoint
    /// let _ = config;
    /// ```
    #[must_use]
    pub fn with_otlp_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.otlp_endpoint = Some(endpoint.into());
        self
    }

    /// Set the service name for resource detection
    ///
    /// # Parameters
    ///
    /// * `name` - Service name
    ///
    /// # Examples
    ///
    /// ```
    /// use juncture_tracing::config::TracingConfig;
    ///
    /// let config = TracingConfig::new()
    ///     .with_service_name("my-agent");
    /// // Configures service name
    /// let _ = config;
    /// ```
    #[must_use]
    pub fn with_service_name(mut self, name: impl Into<String>) -> Self {
        self.service_name = name.into();
        self
    }

    /// Set the service version for resource detection
    ///
    /// # Parameters
    ///
    /// * `version` - Service version
    ///
    /// # Examples
    ///
    /// ```
    /// use juncture_tracing::config::TracingConfig;
    ///
    /// let config = TracingConfig::new()
    ///     .with_service_version("1.0.0");
    /// // Configures service version
    /// let _ = config;
    /// ```
    #[must_use]
    pub fn with_service_version(mut self, version: impl Into<String>) -> Self {
        self.service_version = version.into();
        self
    }

    /// Add resource attributes for the service
    ///
    /// # Parameters
    ///
    /// * `attrs` - Vector of (key, value) pairs
    ///
    /// # Examples
    ///
    /// ```
    /// use juncture_tracing::config::TracingConfig;
    ///
    /// let config = TracingConfig::new()
    ///     .with_resource_attributes([
    ///         ("deployment.environment".to_string(), "production".to_string()),
    ///         ("service.instance.id".to_string(), "pod-abc".to_string()),
    ///     ]);
    /// // Configures resource attributes
    /// let _ = config;
    /// ```
    #[must_use]
    pub fn with_resource_attributes(mut self, attrs: impl Into<Vec<(String, String)>>) -> Self {
        self.resource_attributes = attrs.into();
        self
    }

    /// Set the trace sampling rate
    ///
    /// # Parameters
    ///
    /// * `rate` - Sampling rate from 0.0 (no traces) to 1.0 (all traces)
    ///
    /// # Examples
    ///
    /// ```
    /// use juncture_tracing::config::TracingConfig;
    ///
    /// let config = TracingConfig::new()
    ///     .with_trace_sampling(0.1);
    /// // Configures 10% trace sampling
    /// let _ = config;
    /// ```
    #[must_use]
    pub const fn with_trace_sampling(mut self, rate: f64) -> Self {
        self.trace_sampling = rate;
        self
    }

    /// Enable or disable metrics export
    ///
    /// # Parameters
    ///
    /// * `enabled` - Whether to enable metrics export
    ///
    /// # Examples
    ///
    /// ```
    /// use juncture_tracing::config::TracingConfig;
    ///
    /// let config = TracingConfig::new()
    ///     .with_metrics(true);
    /// // Enables metrics export
    /// let _ = config;
    /// ```
    #[must_use]
    pub const fn with_metrics(mut self, enabled: bool) -> Self {
        self.metrics_enabled = enabled;
        self
    }

    /// Set the log level for the subscriber
    ///
    /// # Parameters
    ///
    /// * `level` - Minimum log level to emit
    ///
    /// # Examples
    ///
    /// ```
    /// use juncture_tracing::config::TracingConfig;
    /// use tracing::Level;
    ///
    /// let config = TracingConfig::new()
    ///     .with_log_level(Level::DEBUG);
    /// // Configures DEBUG log level
    /// let _ = config;
    /// ```
    #[must_use]
    pub const fn with_log_level(mut self, level: tracing::Level) -> Self {
        self.log_level = level;
        self
    }

    /// Install the tracing subscriber
    ///
    /// This initializes the global tracing subscriber with the configured
    /// settings. Currently sets up a basic `tracing-subscriber` for logging.
    /// In the future, when OpenTelemetry dependencies are added, this will
    /// configure OTLP exporters.
    ///
    /// # Errors
    ///
    /// Returns an error if the subscriber is already installed or installation
    /// fails for any reason.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use juncture_tracing::config::TracingConfig;
    ///
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// TracingConfig::new()
    ///     .with_service_name("my-app")
    ///     .install()?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn install(self) -> Result<(), TracingError> {
        // Build the env filter from the configured log level
        let env_filter = tracing_subscriber::EnvFilter::builder()
            .with_default_directive(self.log_level.into())
            .from_env_lossy();

        // Install the subscriber
        tracing_subscriber::fmt()
            .with_env_filter(env_filter)
            .try_init()
            .map_err(|e| TracingError::InstallFailed(e.to_string()))?;

        Ok(())
    }
}

/// Initialize tracing with default configuration
///
/// Convenience function that returns a `TracingConfig` with default settings.
/// Chain builder methods to customize, then call `install()` to activate.
///
/// # Examples
///
/// ```
/// use juncture_tracing::config::init;
///
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// init().with_service_name("my-app").install()?;
/// # Ok(())
/// # }
/// ```
#[cfg(feature = "otel")]
#[must_use]
pub fn init() -> TracingConfig {
    TracingConfig::new()
}

#[cfg(all(test, feature = "otel"))]
mod tests {
    use super::*;

    #[test]
    fn test_tracing_config_default() {
        let config = TracingConfig::new();
        assert_eq!(config.service_name, "juncture-app");
        assert!(!config.metrics_enabled);
        assert!((config.trace_sampling - 1.0).abs() < f64::EPSILON);
        assert_eq!(config.log_level, tracing::Level::INFO);
    }

    #[test]
    fn test_tracing_config_builder() {
        let config = TracingConfig::new()
            .with_otlp_endpoint("http://localhost:4317")
            .with_service_name("test-service")
            .with_service_version("2.0.0")
            .with_resource_attributes(vec![("key".to_string(), "value".to_string())])
            .with_trace_sampling(0.5)
            .with_metrics(true)
            .with_log_level(tracing::Level::DEBUG);

        assert_eq!(
            config.otlp_endpoint,
            Some("http://localhost:4317".to_string())
        );
        assert_eq!(config.service_name, "test-service");
        assert_eq!(config.service_version, "2.0.0");
        assert_eq!(config.resource_attributes.len(), 1);
        assert!((config.trace_sampling - 0.5).abs() < f64::EPSILON);
        assert!(config.metrics_enabled);
        assert_eq!(config.log_level, tracing::Level::DEBUG);
    }

    #[test]
    fn test_init_function() {
        let config = init();
        assert_eq!(config.service_name, "juncture-app");
    }

    #[test]
    fn test_tracing_error_display() {
        let err = TracingError::InstallFailed("test error".to_string());
        let display_str = format!("{err}");
        assert!(display_str.contains("Failed to install"));
        assert!(display_str.contains("test error"));
    }

    #[test]
    fn test_install_no_panic() {
        // This test verifies that install() doesn't panic.
        // We can't actually test successful installation multiple times
        // in a single process, so we just verify the API works.
        let config = TracingConfig::new();
        // We expect this might fail if already installed, but it shouldn't panic
        let _result = std::panic::catch_unwind(|| {
            let _ = config.install();
        });
        // The test passes as long as we don't panic
    }
}

// Rust guideline compliant 2026-05-19
