//! Test utilities for metrics and tracing
//!
//! This module provides test helpers for collecting and asserting on metrics
//! in integration tests.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Test metrics collector for use in integration tests
///
/// A simple in-memory metrics collector that records counter increments,
/// histogram values, and gauge settings for test assertions.
///
/// # Examples
///
/// ```
/// use juncture_tracing::test_utils::TestMetricsCollector;
///
/// let metrics = TestMetricsCollector::new();
/// metrics.increment_counter("test.counter", 1);
/// metrics.record_histogram("test.histogram", 42.0);
/// metrics.set_gauge("test.gauge", 100.0);
///
/// assert_eq!(metrics.get_counter("test.counter"), 1);
/// assert_eq!(metrics.get_histogram_values("test.histogram"), vec![42.0]);
/// assert_eq!(metrics.get_gauge("test.gauge"), Some(100.0));
/// ```
#[derive(Clone, Debug)]
pub struct TestMetricsCollector {
    counters: Arc<Mutex<HashMap<String, u64>>>,
    histogram_values: Arc<Mutex<HashMap<String, Vec<f64>>>>,
    gauge_values: Arc<Mutex<HashMap<String, f64>>>,
}

impl Default for TestMetricsCollector {
    fn default() -> Self {
        Self::new()
    }
}

impl juncture_core::observability::MetricsCollector for TestMetricsCollector {
    fn inc_counter(&self, name: &str, value: u64) {
        self.increment_counter(name, value);
    }

    fn record_histogram(&self, name: &str, value: f64) {
        self.record_histogram(name, value);
    }

    fn set_gauge(&self, name: &str, value: u64) {
        #[allow(
            clippy::cast_precision_loss,
            reason = "gauge values from OTel are u64, stored as f64 in test utility"
        )]
        let fval = value as f64;
        self.set_gauge(name, fval);
    }
}

impl TestMetricsCollector {
    /// Create a new test metrics collector
    ///
    /// # Examples
    ///
    /// ```
    /// use juncture_tracing::test_utils::TestMetricsCollector;
    ///
    /// let collector = TestMetricsCollector::new();
    /// assert_eq!(collector.get_counter("any"), 0);
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self {
            counters: Arc::new(Mutex::new(HashMap::new())),
            histogram_values: Arc::new(Mutex::new(HashMap::new())),
            gauge_values: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Increment a counter metric
    ///
    /// Adds the given value to the counter, creating it if it doesn't exist.
    ///
    /// # Parameters
    ///
    /// * `name` - Counter metric name
    /// * `value` - Value to add (default is 1)
    ///
    /// # Panics
    ///
    /// Panics if the internal mutex is poisoned (should not happen in normal usage).
    ///
    /// # Examples
    ///
    /// ```
    /// use juncture_tracing::test_utils::TestMetricsCollector;
    ///
    /// let metrics = TestMetricsCollector::new();
    /// metrics.increment_counter("my.counter", 1);
    /// metrics.increment_counter("my.counter", 2);
    /// assert_eq!(metrics.get_counter("my.counter"), 3);
    /// ```
    pub fn increment_counter(&self, name: &str, value: u64) {
        let mut counters = self.counters.lock().unwrap();
        *counters.entry(name.to_string()).or_insert(0) += value;
    }

    /// Record a value in a histogram metric
    ///
    /// Adds the value to the histogram's recorded values.
    ///
    /// # Parameters
    ///
    /// * `name` - Histogram metric name
    /// * `value` - Value to record
    ///
    /// # Panics
    ///
    /// Panics if the internal mutex is poisoned (should not happen in normal usage).
    ///
    /// # Examples
    ///
    /// ```
    /// use juncture_tracing::test_utils::TestMetricsCollector;
    ///
    /// let metrics = TestMetricsCollector::new();
    /// metrics.record_histogram("latency_ms", 100.0);
    /// metrics.record_histogram("latency_ms", 200.0);
    ///
    /// let values = metrics.get_histogram_values("latency_ms");
    /// assert_eq!(values.len(), 2);
    /// assert_eq!(values[0], 100.0);
    /// assert_eq!(values[1], 200.0);
    /// ```
    pub fn record_histogram(&self, name: &str, value: f64) {
        let mut histograms = self.histogram_values.lock().unwrap();
        histograms.entry(name.to_string()).or_default().push(value);
    }

    /// Set a gauge metric to a specific value
    ///
    /// # Parameters
    ///
    /// * `name` - Gauge metric name
    /// * `value` - Value to set
    ///
    /// # Panics
    ///
    /// Panics if the internal mutex is poisoned (should not happen in normal usage).
    ///
    /// # Examples
    ///
    /// ```
    /// use juncture_tracing::test_utils::TestMetricsCollector;
    ///
    /// let metrics = TestMetricsCollector::new();
    /// metrics.set_gauge("temperature", 98.6);
    /// metrics.set_gauge("temperature", 99.1);
    ///
    /// assert_eq!(metrics.get_gauge("temperature"), Some(99.1));
    /// ```
    pub fn set_gauge(&self, name: &str, value: f64) {
        let mut gauges = self.gauge_values.lock().unwrap();
        gauges.insert(name.to_string(), value);
    }

    /// Get the current value of a counter metric
    ///
    /// Returns 0 if the counter has never been incremented.
    ///
    /// # Parameters
    ///
    /// * `name` - Counter metric name
    ///
    /// # Panics
    ///
    /// Panics if the internal mutex is poisoned (should not happen in normal usage).
    ///
    /// # Examples
    ///
    /// ```
    /// use juncture_tracing::test_utils::TestMetricsCollector;
    ///
    /// let metrics = TestMetricsCollector::new();
    /// assert_eq!(metrics.get_counter("test"), 0);
    ///
    /// metrics.increment_counter("test", 5);
    /// assert_eq!(metrics.get_counter("test"), 5);
    /// ```
    #[must_use]
    pub fn get_counter(&self, name: &str) -> u64 {
        let counters = self.counters.lock().unwrap();
        counters.get(name).copied().unwrap_or(0)
    }

    /// Get all recorded values for a histogram metric
    ///
    /// Returns an empty vector if the histogram has no values.
    ///
    /// # Parameters
    ///
    /// * `name` - Histogram metric name
    ///
    /// # Panics
    ///
    /// Panics if the internal mutex is poisoned (should not happen in normal usage).
    ///
    /// # Examples
    ///
    /// ```
    /// use juncture_tracing::test_utils::TestMetricsCollector;
    ///
    /// let metrics = TestMetricsCollector::new();
    /// assert!(metrics.get_histogram_values("test").is_empty());
    ///
    /// metrics.record_histogram("test", 1.0);
    /// assert_eq!(metrics.get_histogram_values("test"), vec![1.0]);
    /// ```
    #[must_use]
    pub fn get_histogram_values(&self, name: &str) -> Vec<f64> {
        let histograms = self.histogram_values.lock().unwrap();
        histograms.get(name).cloned().unwrap_or_default()
    }

    /// Get the current value of a gauge metric
    ///
    /// Returns `None` if the gauge has never been set.
    ///
    /// # Parameters
    ///
    /// * `name` - Gauge metric name
    ///
    /// # Panics
    ///
    /// Panics if the internal mutex is poisoned (should not happen in normal usage).
    ///
    /// # Examples
    ///
    /// ```
    /// use juncture_tracing::test_utils::TestMetricsCollector;
    ///
    /// let metrics = TestMetricsCollector::new();
    /// assert_eq!(metrics.get_gauge("test"), None);
    ///
    /// metrics.set_gauge("test", 42.0);
    /// assert_eq!(metrics.get_gauge("test"), Some(42.0));
    /// ```
    #[must_use]
    pub fn get_gauge(&self, name: &str) -> Option<f64> {
        let gauges = self.gauge_values.lock().unwrap();
        gauges.get(name).copied()
    }

    /// Clear all recorded metrics
    ///
    /// Useful for resetting state between test cases.
    ///
    /// # Panics
    ///
    /// Panics if any internal mutex is poisoned (should not happen in normal usage).
    ///
    /// # Examples
    ///
    /// ```
    /// use juncture_tracing::test_utils::TestMetricsCollector;
    ///
    /// let metrics = TestMetricsCollector::new();
    /// metrics.increment_counter("test", 5);
    /// metrics.clear();
    /// assert_eq!(metrics.get_counter("test"), 0);
    /// ```
    #[expect(
        clippy::significant_drop_tightening,
        reason = "Locks are held only briefly for clearing"
    )]
    pub fn clear(&self) {
        let mut counters = self.counters.lock().unwrap();
        let mut histograms = self.histogram_values.lock().unwrap();
        let mut gauges = self.gauge_values.lock().unwrap();

        counters.clear();
        histograms.clear();
        gauges.clear();
    }

    /// Get all counter names that have been recorded
    ///
    /// # Panics
    ///
    /// Panics if the internal mutex is poisoned (should not happen in normal usage).
    ///
    /// # Examples
    ///
    /// ```
    /// use juncture_tracing::test_utils::TestMetricsCollector;
    ///
    /// let metrics = TestMetricsCollector::new();
    /// metrics.increment_counter("counter1", 1);
    /// metrics.increment_counter("counter2", 1);
    ///
    /// let names = metrics.counter_names();
    /// assert_eq!(names.len(), 2);
    /// assert!(names.contains(&"counter1".to_string()));
    /// ```
    #[must_use]
    pub fn counter_names(&self) -> Vec<String> {
        let counters = self.counters.lock().unwrap();
        counters.keys().cloned().collect()
    }

    /// Get all histogram names that have been recorded
    ///
    /// # Panics
    ///
    /// Panics if the internal mutex is poisoned (should not happen in normal usage).
    ///
    /// # Examples
    ///
    /// ```
    /// use juncture_tracing::test_utils::TestMetricsCollector;
    ///
    /// let metrics = TestMetricsCollector::new();
    /// metrics.record_histogram("hist1", 1.0);
    /// metrics.record_histogram("hist2", 2.0);
    ///
    /// let names = metrics.histogram_names();
    /// assert_eq!(names.len(), 2);
    /// assert!(names.contains(&"hist1".to_string()));
    /// ```
    #[must_use]
    pub fn histogram_names(&self) -> Vec<String> {
        let histograms = self.histogram_values.lock().unwrap();
        histograms.keys().cloned().collect()
    }

    /// Get all gauge names that have been recorded
    ///
    /// # Panics
    ///
    /// Panics if the internal mutex is poisoned (should not happen in normal usage).
    ///
    /// # Examples
    ///
    /// ```
    /// use juncture_tracing::test_utils::TestMetricsCollector;
    ///
    /// let metrics = TestMetricsCollector::new();
    /// metrics.set_gauge("gauge1", 1.0);
    /// metrics.set_gauge("gauge2", 2.0);
    ///
    /// let names = metrics.gauge_names();
    /// assert_eq!(names.len(), 2);
    /// assert!(names.contains(&"gauge1".to_string()));
    /// ```
    #[must_use]
    pub fn gauge_names(&self) -> Vec<String> {
        let gauges = self.gauge_values.lock().unwrap();
        gauges.keys().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default() {
        let collector = TestMetricsCollector::default();
        assert_eq!(collector.get_counter("test"), 0);
        assert!(collector.get_histogram_values("test").is_empty());
        assert_eq!(collector.get_gauge("test"), None);
    }

    #[test]
    fn test_increment_counter() {
        let metrics = TestMetricsCollector::new();

        metrics.increment_counter("test.counter", 1);
        assert_eq!(metrics.get_counter("test.counter"), 1);

        metrics.increment_counter("test.counter", 2);
        assert_eq!(metrics.get_counter("test.counter"), 3);

        // Different counter
        metrics.increment_counter("other.counter", 10);
        assert_eq!(metrics.get_counter("other.counter"), 10);
        assert_eq!(metrics.get_counter("test.counter"), 3);
    }

    #[test]
    fn test_record_histogram() {
        let metrics = TestMetricsCollector::new();

        metrics.record_histogram("test.histogram", 1.0);
        assert_eq!(metrics.get_histogram_values("test.histogram"), vec![1.0]);

        metrics.record_histogram("test.histogram", 2.0);
        metrics.record_histogram("test.histogram", 3.0);

        let values = metrics.get_histogram_values("test.histogram");
        assert_eq!(values.len(), 3);
        assert_eq!(values, vec![1.0, 2.0, 3.0]);

        // Different histogram
        metrics.record_histogram("other.histogram", 100.0);
        assert_eq!(metrics.get_histogram_values("other.histogram"), vec![100.0]);
    }

    #[test]
    fn test_set_gauge() {
        let metrics = TestMetricsCollector::new();

        metrics.set_gauge("test.gauge", 50.0);
        assert_eq!(metrics.get_gauge("test.gauge"), Some(50.0));

        metrics.set_gauge("test.gauge", 75.0);
        assert_eq!(metrics.get_gauge("test.gauge"), Some(75.0));

        // Different gauge
        metrics.set_gauge("other.gauge", 100.0);
        assert_eq!(metrics.get_gauge("other.gauge"), Some(100.0));
        assert_eq!(metrics.get_gauge("test.gauge"), Some(75.0));
    }

    #[test]
    fn test_clear() {
        let metrics = TestMetricsCollector::new();

        metrics.increment_counter("counter", 5);
        metrics.record_histogram("histogram", 1.0);
        metrics.set_gauge("gauge", 10.0);

        metrics.clear();

        assert_eq!(metrics.get_counter("counter"), 0);
        assert!(metrics.get_histogram_values("histogram").is_empty());
        assert_eq!(metrics.get_gauge("gauge"), None);
    }

    #[test]
    fn test_metric_names() {
        let metrics = TestMetricsCollector::new();

        metrics.increment_counter("counter1", 1);
        metrics.increment_counter("counter2", 1);

        let counter_names = metrics.counter_names();
        assert_eq!(counter_names.len(), 2);
        assert!(counter_names.contains(&"counter1".to_string()));
        assert!(counter_names.contains(&"counter2".to_string()));

        metrics.record_histogram("hist1", 1.0);
        metrics.record_histogram("hist2", 1.0);

        let histogram_names = metrics.histogram_names();
        assert_eq!(histogram_names.len(), 2);
        assert!(histogram_names.contains(&"hist1".to_string()));

        metrics.set_gauge("gauge1", 1.0);
        metrics.set_gauge("gauge2", 1.0);

        let gauge_names = metrics.gauge_names();
        assert_eq!(gauge_names.len(), 2);
        assert!(gauge_names.contains(&"gauge1".to_string()));
    }

    #[test]
    fn test_clone() {
        let metrics1 = TestMetricsCollector::new();
        metrics1.increment_counter("test", 5);

        let metrics2 = metrics1.clone();
        assert_eq!(metrics2.get_counter("test"), 5);

        // Changes to clone affect original (they share the same Arc)
        metrics2.increment_counter("test", 3);
        assert_eq!(metrics1.get_counter("test"), 8);
    }
}

// Rust guideline compliant 2026-05-19
