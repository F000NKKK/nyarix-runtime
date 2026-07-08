//! Metric registry (see issue #80): registering and recording
//! counter/gauge/histogram metrics, namespaced per module.
//!
//! [`MetricRegistry`] is the storage — thread-safe via [`DashMap`] for
//! the registry itself and atomics (or a small [`Mutex`] for
//! [`Histogram`], which needs more than one word of state) for each
//! metric's value, so concurrent modules recording metrics never block
//! each other on unrelated metrics. [`MetricsHandle`] (#18) is what a
//! module actually holds — a thin, optional reference to a registry
//! (mirroring [`crate::event::EventBus`]'s "attached or not" shape on
//! [`crate::context::RuntimeContext`]).
//!
//! **Scope note:** Prometheus text-exposition export isn't
//! implemented — the issue lists "Prometheus/JSON" as an *either*,
//! optional bullet, and [`MetricRegistry::export_json`] already
//! satisfies it once.

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use dashmap::DashMap;
use serde::Serialize;

/// Build a metric's fully-qualified name: `nyarix.module.<module>.<name>`.
#[must_use]
pub fn metric_name(module: &str, name: &str) -> String {
    format!("nyarix.module.{module}.{name}")
}

/// A monotonically increasing counter.
#[derive(Debug, Default)]
pub struct Counter(AtomicU64);

impl Counter {
    /// Increment by `delta`.
    pub fn increment(&self, delta: u64) {
        self.0.fetch_add(delta, Ordering::Relaxed);
    }

    /// Current value.
    #[must_use]
    pub fn value(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }
}

/// A value that can go up or down.
#[derive(Debug, Default)]
pub struct Gauge(AtomicI64);

impl Gauge {
    /// Set the gauge to an absolute value.
    pub fn set(&self, value: i64) {
        self.0.store(value, Ordering::Relaxed);
    }

    /// Add `delta` (negative to decrease) to the current value.
    pub fn add(&self, delta: i64) {
        self.0.fetch_add(delta, Ordering::Relaxed);
    }

    /// Current value.
    #[must_use]
    pub fn value(&self) -> i64 {
        self.0.load(Ordering::Relaxed)
    }
}

#[derive(Debug)]
struct HistogramState {
    /// Upper bound of each bucket, ascending; the last bucket is
    /// implicitly `+Infinity`.
    bounds: Vec<f64>,
    /// Count of observations `<=` each corresponding bound in
    /// `bounds`, plus one trailing "everything else" bucket.
    counts: Vec<u64>,
    sum: f64,
    count: u64,
}

/// A distribution of observed values, bucketed by fixed boundaries
/// (Prometheus-style cumulative histogram).
#[derive(Debug)]
pub struct Histogram {
    state: Mutex<HistogramState>,
}

impl Histogram {
    /// Build a histogram with `bounds` as each bucket's upper edge
    /// (ascending) — one implicit `+Infinity` bucket catches anything
    /// above the last bound.
    #[must_use]
    pub fn new(bounds: Vec<f64>) -> Self {
        let bucket_count = bounds.len() + 1;
        Self {
            state: Mutex::new(HistogramState {
                bounds,
                counts: vec![0; bucket_count],
                sum: 0.0,
                count: 0,
            }),
        }
    }

    /// Record one observation.
    pub fn observe(&self, value: f64) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let bucket = state
            .bounds
            .iter()
            .position(|&bound| value <= bound)
            .unwrap_or(state.counts.len() - 1);
        state.counts[bucket] += 1;
        state.sum += value;
        state.count += 1;
    }

    /// A snapshot of this histogram's current state.
    #[must_use]
    pub fn snapshot(&self) -> HistogramSnapshot {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        HistogramSnapshot {
            bounds: state.bounds.clone(),
            counts: state.counts.clone(),
            sum: state.sum,
            count: state.count,
        }
    }
}

/// A point-in-time copy of a [`Histogram`]'s state, safe to hold
/// without the lock.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HistogramSnapshot {
    /// Each bucket's upper bound (ascending), not including the
    /// implicit trailing `+Infinity` bucket.
    pub bounds: Vec<f64>,
    /// Per-bucket observation counts — one more entry than `bounds`.
    pub counts: Vec<u64>,
    /// Sum of every observed value.
    pub sum: f64,
    /// Total number of observations.
    pub count: u64,
}

/// A thread-safe registry of counters, gauges, and histograms,
/// namespaced per module (see [`metric_name`]).
#[derive(Debug, Default)]
pub struct MetricRegistry {
    counters: DashMap<String, Arc<Counter>>,
    gauges: DashMap<String, Arc<Gauge>>,
    histograms: DashMap<String, Arc<Histogram>>,
}

impl MetricRegistry {
    /// An empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Get (or, on first call for this name, register) the counter
    /// named `name` under `module`.
    #[must_use]
    pub fn counter(&self, module: &str, name: &str) -> Arc<Counter> {
        Arc::clone(&self.counters.entry(metric_name(module, name)).or_default())
    }

    /// Get (or, on first call for this name, register) the gauge named
    /// `name` under `module`.
    #[must_use]
    pub fn gauge(&self, module: &str, name: &str) -> Arc<Gauge> {
        Arc::clone(&self.gauges.entry(metric_name(module, name)).or_default())
    }

    /// Get (or, on first call for this name, register with `bounds`)
    /// the histogram named `name` under `module`. `bounds` is only
    /// used the first time this name is registered — later calls
    /// return the already-registered histogram regardless of what
    /// `bounds` they pass.
    #[must_use]
    pub fn histogram(&self, module: &str, name: &str, bounds: Vec<f64>) -> Arc<Histogram> {
        Arc::clone(
            &self
                .histograms
                .entry(metric_name(module, name))
                .or_insert_with(|| Arc::new(Histogram::new(bounds))),
        )
    }

    /// Export every currently registered metric as a JSON object
    /// (this issue's optional "Экспорт в Prometheus/JSON формат").
    #[must_use]
    pub fn export_json(&self) -> String {
        let mut counters = HashMap::new();
        for entry in &self.counters {
            counters.insert(entry.key().clone(), entry.value().value());
        }
        let mut gauges = HashMap::new();
        for entry in &self.gauges {
            gauges.insert(entry.key().clone(), entry.value().value());
        }
        let mut histograms = HashMap::new();
        for entry in &self.histograms {
            histograms.insert(entry.key().clone(), entry.value().snapshot());
        }

        let export = Export {
            counters,
            gauges,
            histograms,
        };
        serde_json::to_string(&export).unwrap_or_else(|_| "{}".to_string())
    }
}

#[derive(Serialize)]
struct Export {
    counters: HashMap<String, u64>,
    gauges: HashMap<String, i64>,
    histograms: HashMap<String, HistogramSnapshot>,
}

/// Handle a module uses to record metrics (see issue #18; the real
/// system is #80).
///
/// Optionally attached to a shared [`MetricRegistry`] — the same
/// "attached or not" shape as [`crate::event::EventBus`] on
/// [`crate::context::RuntimeContext`]: a context built without one
/// (tests, or before the Runtime wires one up) simply can't record
/// anything, without needing a separate code path.
#[derive(Debug, Clone, Default)]
pub struct MetricsHandle {
    registry: Option<Arc<MetricRegistry>>,
}

impl MetricsHandle {
    /// Attach a live registry.
    #[must_use]
    pub fn with_registry(registry: Arc<MetricRegistry>) -> Self {
        Self {
            registry: Some(registry),
        }
    }

    /// The counter named `name` under `module`, or `None` if this
    /// handle has no registry attached.
    #[must_use]
    pub fn counter(&self, module: &str, name: &str) -> Option<Arc<Counter>> {
        self.registry
            .as_ref()
            .map(|registry| registry.counter(module, name))
    }

    /// The gauge named `name` under `module`, or `None` if this handle
    /// has no registry attached.
    #[must_use]
    pub fn gauge(&self, module: &str, name: &str) -> Option<Arc<Gauge>> {
        self.registry
            .as_ref()
            .map(|registry| registry.gauge(module, name))
    }

    /// The histogram named `name` under `module` (registering it with
    /// `bounds` if this is the first call for that name), or `None` if
    /// this handle has no registry attached.
    #[must_use]
    pub fn histogram(&self, module: &str, name: &str, bounds: Vec<f64>) -> Option<Arc<Histogram>> {
        self.registry
            .as_ref()
            .map(|registry| registry.histogram(module, name, bounds))
    }

    /// The underlying [`MetricRegistry`], if one is attached — for
    /// callers (e.g. `nyarix_graph::execute_sequential`/
    /// `execute_parallel`, #82) that record metrics keyed by an
    /// arbitrary module name rather than a single one this handle is
    /// scoped to.
    #[must_use]
    pub fn registry(&self) -> Option<&MetricRegistry> {
        self.registry.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metric_name_follows_the_namespace_convention() {
        assert_eq!(
            metric_name("quic-transport", "packets_sent"),
            "nyarix.module.quic-transport.packets_sent"
        );
    }

    #[test]
    fn counter_starts_at_zero_and_accumulates() {
        let counter = Counter::default();
        assert_eq!(counter.value(), 0);
        counter.increment(3);
        counter.increment(2);
        assert_eq!(counter.value(), 5);
    }

    #[test]
    fn gauge_can_be_set_and_adjusted() {
        let gauge = Gauge::default();
        gauge.set(10);
        assert_eq!(gauge.value(), 10);
        gauge.add(-3);
        assert_eq!(gauge.value(), 7);
    }

    #[test]
    fn histogram_buckets_observations_and_tracks_sum_and_count() {
        let histogram = Histogram::new(vec![1.0, 5.0, 10.0]);
        histogram.observe(0.5);
        histogram.observe(3.0);
        histogram.observe(7.0);
        histogram.observe(100.0);

        let snapshot = histogram.snapshot();
        assert_eq!(snapshot.counts, vec![1, 1, 1, 1]);
        assert_eq!(snapshot.count, 4);
        assert!((snapshot.sum - 110.5).abs() < f64::EPSILON);
    }

    #[test]
    fn registry_returns_the_same_counter_instance_for_repeated_lookups() {
        let registry = MetricRegistry::new();
        let a = registry.counter("quic", "packets_sent");
        let b = registry.counter("quic", "packets_sent");
        a.increment(5);
        assert_eq!(b.value(), 5);
    }

    #[test]
    fn different_modules_get_independent_counters() {
        let registry = MetricRegistry::new();
        registry.counter("quic", "packets_sent").increment(5);
        registry.counter("udp", "packets_sent").increment(2);

        assert_eq!(registry.counter("quic", "packets_sent").value(), 5);
        assert_eq!(registry.counter("udp", "packets_sent").value(), 2);
    }

    #[test]
    fn export_json_includes_every_registered_metric_kind() {
        let registry = MetricRegistry::new();
        registry.counter("quic", "packets_sent").increment(5);
        registry.gauge("quic", "active_connections").set(3);
        registry
            .histogram("quic", "latency_ms", vec![10.0, 50.0])
            .observe(25.0);

        let json = registry.export_json();
        assert!(json.contains("nyarix.module.quic.packets_sent"));
        assert!(json.contains("nyarix.module.quic.active_connections"));
        assert!(json.contains("nyarix.module.quic.latency_ms"));
    }

    #[test]
    fn metrics_handle_without_a_registry_returns_none() {
        let handle = MetricsHandle::default();
        assert!(handle.counter("quic", "packets_sent").is_none());
        assert!(handle.gauge("quic", "active_connections").is_none());
        assert!(handle.histogram("quic", "latency_ms", vec![]).is_none());
    }

    #[test]
    fn metrics_handle_with_a_registry_delegates_to_it() {
        let registry = Arc::new(MetricRegistry::new());
        let handle = MetricsHandle::with_registry(Arc::clone(&registry));

        handle.counter("quic", "packets_sent").unwrap().increment(1);

        assert_eq!(registry.counter("quic", "packets_sent").value(), 1);
    }
}
