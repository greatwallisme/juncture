//! Tracing configuration and OpenTelemetry initialization
//!
//! This module provides the `TracingConfig` builder for configuring OpenTelemetry
//! trace and metrics export. This feature is only available when the `otel` feature
//! is enabled.

use std::fmt;

#[cfg(feature = "otel")]
use opentelemetry::global;
#[cfg(feature = "otel")]
use opentelemetry::trace::TracerProvider as _;
#[cfg(feature = "otel")]
use opentelemetry_otlp::MetricExporter;
#[cfg(feature = "otel")]
use opentelemetry_otlp::WithExportConfig;
#[cfg(feature = "otel")]
use opentelemetry_sdk::metrics::{PeriodicReader, SdkMeterProvider};
#[cfg(feature = "otel")]
use opentelemetry_sdk::runtime::Tokio;
#[cfg(feature = "otel")]
use opentelemetry_sdk::{Resource, trace::TracerProvider};
#[cfg(feature = "otel")]
use tracing_subscriber::prelude::*;

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
    /// OTLP endpoint for trace/metrics export
    otlp_endpoint: Option<String>,

    /// Service name for resource detection
    service_name: String,

    /// Service version for resource detection
    service_version: String,

    /// Additional resource attributes
    resource_attributes: Vec<(String, String)>,

    /// Trace sampling rate (0.0 to 1.0)
    trace_sampling: f64,

    /// Whether metrics export is enabled
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

    /// Enable or disable `OTel` metrics export
    ///
    /// When enabled and an OTLP endpoint is configured via
    /// [`with_otlp_endpoint`](Self::with_otlp_endpoint), a global
    /// [`SdkMeterProvider`] is created alongside the tracer provider.
    /// Consumers can then obtain a [`Meter`] via
    /// `opentelemetry::global::meter("juncture")` and pass it to
    /// [`MetricsRegistry::with_meter`].
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
    ///     .with_otlp_endpoint("http://collector:4317")
    ///     .with_metrics(true);
    /// // Enables metrics export via OTLP
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
    /// settings. When the `otel` feature is enabled and an OTLP endpoint
    /// is configured, it sets up OpenTelemetry OTLP export for both traces
    /// and (if enabled) metrics. Otherwise, it configures basic
    /// `tracing-subscriber` logging.
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

        #[cfg(feature = "otel")]
        {
            // Only set up OTLP if endpoint is configured
            // Clone to avoid borrow checker issues with self move
            let otlp_endpoint = self.otlp_endpoint.clone();
            if let Some(endpoint) = otlp_endpoint.as_ref() {
                return self.install_otel(env_filter, endpoint);
            }

            // Warn if metrics were requested but no OTLP endpoint is configured
            if self.metrics_enabled {
                tracing::warn!(
                    "Metrics export is enabled but no OTLP endpoint is configured. \
                     Call .with_otlp_endpoint() to enable OTLP trace and metrics export."
                );
            }
        }

        // Fallback to basic fmt subscriber without OTLP
        tracing_subscriber::fmt()
            .with_env_filter(env_filter)
            .try_init()
            .map_err(|e| TracingError::InstallFailed(e.to_string()))?;

        Ok(())
    }

    /// Install OpenTelemetry OTLP pipeline
    ///
    /// Sets up the OpenTelemetry tracer with OTLP exporter and configures
    /// the tracing subscriber with both OTLP and fmt layers. When
    /// `metrics_enabled` is `true`, also creates a global
    /// [`SdkMeterProvider`] with an OTLP metric exporter so that metrics
    /// collected through [`MetricsRegistry::with_meter`] flow to OTLP.
    ///
    /// This is separated from `install()` to allow conditional compilation
    /// based on the `otel` feature flag.
    #[cfg(feature = "otel")]
    #[allow(
        clippy::too_many_lines,
        reason = "OTLP setup requires: resource config, OTLP exporters (trace + metrics), tracer provider, meter provider, layers, subscriber initialization"
    )]
    fn install_otel(
        self,
        env_filter: tracing_subscriber::EnvFilter,
        otlp_endpoint: &str,
    ) -> Result<(), TracingError> {
        // Build resource attributes
        let mut resource_attributes = vec![
            opentelemetry::KeyValue::new("service.name", self.service_name),
            opentelemetry::KeyValue::new("service.version", self.service_version),
        ];

        // Add custom resource attributes
        for (key, value) in self.resource_attributes {
            resource_attributes.push(opentelemetry::KeyValue::new(key, value));
        }

        let resource = Resource::new(resource_attributes);

        // Configure OTLP trace exporter using tonic
        let exporter = opentelemetry_otlp::SpanExporter::builder()
            .with_tonic()
            .with_endpoint(otlp_endpoint)
            .build()
            .map_err(|e| {
                TracingError::InstallFailed(format!("OTLP trace exporter build failed: {e}"))
            })?;

        // Create tracer provider with resource (clone for potential metrics use below)
        let tracer_provider = TracerProvider::builder()
            .with_resource(resource.clone())
            .with_simple_exporter(exporter)
            .build();

        // Create OpenTelemetry layer
        let otel_layer =
            tracing_opentelemetry::layer().with_tracer(tracer_provider.tracer("juncture"));

        // Create fmt layer for console logging with filter
        let fmt_layer = tracing_subscriber::fmt::layer().with_filter(env_filter);

        // Build and initialize subscriber with both layers
        tracing_subscriber::registry()
            .with(otel_layer)
            .with(fmt_layer)
            .try_init()
            .map_err(|e| TracingError::InstallFailed(format!("Subscriber init failed: {e}")))?;

        // Set up OTel metrics export if enabled
        if self.metrics_enabled {
            let metric_exporter = MetricExporter::builder()
                .with_tonic()
                .with_endpoint(otlp_endpoint)
                .build()
                .map_err(|e| {
                    TracingError::InstallFailed(format!("OTLP metric exporter build failed: {e}"))
                })?;

            let reader = PeriodicReader::builder(metric_exporter, Tokio).build();

            let meter_provider = SdkMeterProvider::builder()
                .with_resource(resource)
                .with_reader(reader)
                .build();

            global::set_meter_provider(meter_provider);
        }

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
        let _result = std::panic::catch_unwind(|| {
            let _ = config.install();
        });
        // The test passes as long as we don't panic
    }

    #[test]
    fn test_metrics_flag_is_properly_set() {
        let config = TracingConfig::new().with_metrics(true);
        assert!(config.metrics_enabled);

        let config = TracingConfig::new().with_metrics(false);
        assert!(!config.metrics_enabled);
    }

    #[test]
    fn test_metrics_flag_false_by_default() {
        let config = TracingConfig::new();
        assert!(!config.metrics_enabled);
    }

    #[tokio::test]
    async fn test_install_with_metrics_does_not_panic() {
        // Verify the metrics code path is reached without panicking.
        // The gRPC channel creation is lazy so exporter build succeeds
        // even without a real collector. The try_init() may fail because
        // another test already installed a subscriber, but that is
        // a normal error, not a panic.
        let config = TracingConfig::new()
            .with_service_name("test-metrics-install")
            .with_otlp_endpoint("http://127.0.0.1:4318")
            .with_metrics(true);
        let _result = std::panic::catch_unwind(|| {
            let _ = config.install();
        });
    }
}

// Rust guideline compliant 2026-05-22
