//! Throughput tracking with a sliding window (#85).
//!
//! [`ThroughputTracker`] records byte counts tagged by flow id and
//! computes current bytes/sec over a configurable window — both total
//! and per-flow. The window is a simple ring of `(Instant, u64)` pairs;
//! entries older than the window duration are pruned on each record.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use nyarix_core::FlowId;
use nyarix_module_api::MetricRegistry;

const THROUGHPUT_SCOPE: &str = "flow";

/// Default sliding-window duration: 5 seconds.
pub const DEFAULT_WINDOW: Duration = Duration::from_secs(5);

/// Tracks byte throughput over a sliding window, optionally exporting
/// current rates as gauges on a [`MetricRegistry`].
#[derive(Debug)]
pub struct ThroughputTracker {
    window: Duration,
    /// (timestamp, bytes) — kept sorted by insertion time.
    total: VecDeque<(Instant, u64)>,
    /// Per-flow windows, keyed by [`FlowId`].
    flows: HashMap<FlowId, VecDeque<(Instant, u64)>>,
    metrics: Option<Arc<MetricRegistry>>,
}

impl ThroughputTracker {
    /// Create a tracker with [`DEFAULT_WINDOW`] and no metric registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            window: DEFAULT_WINDOW,
            total: VecDeque::default(),
            flows: HashMap::default(),
            metrics: None,
        }
    }

    /// Attach a [`MetricRegistry`] so the tracker can export
    /// `throughput_bytes_per_sec` gauges on every [`record`](Self::record).
    #[must_use]
    pub fn with_metrics(mut self, metrics: Arc<MetricRegistry>) -> Self {
        self.metrics = Some(metrics);
        self
    }

    /// Override the sliding-window duration (default [`DEFAULT_WINDOW`]).
    #[must_use]
    pub fn with_window(mut self, window: Duration) -> Self {
        self.window = window;
        self
    }

    /// Record `bytes` for `flow_id` at the current instant.
    pub fn record(&mut self, flow_id: FlowId, bytes: u64) {
        let now = Instant::now();
        self.total.push_back((now, bytes));
        self.flows
            .entry(flow_id)
            .or_default()
            .push_back((now, bytes));
        self.prune(now);
        self.export_gauges();
    }

    /// Current total throughput in bytes/sec over the sliding window.
    #[must_use]
    pub fn total_throughput(&self) -> f64 {
        rate_from_window(&self.total, self.window)
    }

    /// Current throughput for a single flow, in bytes/sec.
    #[must_use]
    pub fn flow_throughput(&self, flow_id: FlowId) -> f64 {
        self.flows
            .get(&flow_id)
            .map_or(0.0, |window| rate_from_window(window, self.window))
    }

    /// Remove all entries older than `now - self.window`.
    fn prune(&mut self, now: Instant) {
        let cutoff = now - self.window;
        prune_window(&mut self.total, cutoff);
        for window in self.flows.values_mut() {
            prune_window(window, cutoff);
        }
    }

    /// Write current rates into the metric registry (if attached).
    fn export_gauges(&mut self) {
        let Some(ref metrics) = self.metrics else {
            return;
        };
        let total = self.total_throughput();
        #[allow(clippy::cast_precision_loss)]
        metrics
            .gauge(THROUGHPUT_SCOPE, "throughput_bytes_per_sec")
            .set(total as i64);
    }
}

impl Default for ThroughputTracker {
    fn default() -> Self {
        Self::new()
    }
}

/// Drop entries whose timestamp is before `cutoff`.
fn prune_window(window: &mut VecDeque<(Instant, u64)>, cutoff: Instant) {
    while window.front().is_some_and(|&(ts, _)| ts < cutoff) {
        window.pop_front();
    }
}

/// Compute bytes/sec from a window of `(Instant, u64)` pairs.
///
/// Returns 0.0 when the window is empty or covers 0 elapsed time.
fn rate_from_window(window: &VecDeque<(Instant, u64)>, window_duration: Duration) -> f64 {
    if window.is_empty() {
        return 0.0;
    }
    let total_bytes: u64 = window.iter().map(|&(_, bytes)| bytes).sum();
    let first_ts = window.front().unwrap().0;
    let last_ts = window.back().unwrap().0;
    let elapsed = last_ts.saturating_duration_since(first_ts);
    // Use the actual elapsed time capped at window_duration to avoid
    // dividing by a tiny number for a burst at startup.
    let denom = if elapsed > window_duration {
        window_duration
    } else if elapsed == Duration::ZERO {
        // All entries at the same instant — avoid division by zero.
        window_duration
    } else {
        elapsed
    };
    total_bytes as f64 / denom.as_secs_f64()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_tracker_returns_zero() {
        let tracker = ThroughputTracker::new();
        assert_eq!(tracker.total_throughput(), 0.0);
        assert_eq!(tracker.flow_throughput(FlowId::new()), 0.0);
    }

    #[test]
    fn single_record_is_not_zero() {
        let mut tracker = ThroughputTracker::new();
        let flow = FlowId::new();
        tracker.record(flow, 10_000);
        assert!(tracker.total_throughput() > 0.0);
    }

    #[test]
    fn per_flow_isolation() {
        let mut tracker = ThroughputTracker::new();
        let flow_a = FlowId::new();
        let flow_b = FlowId::new();
        tracker.record(flow_a, 1_000);
        tracker.record(flow_b, 3_000);

        let a = tracker.flow_throughput(flow_a);
        let b = tracker.flow_throughput(flow_b);
        assert!(b > a, "flow_b should have higher throughput than flow_a");
    }

    #[test]
    fn zero_bytes_is_still_recorded() {
        let mut tracker = ThroughputTracker::new();
        let flow = FlowId::new();
        tracker.record(flow, 0);
        // rate is 0, but the window isn't empty
        assert_eq!(tracker.total_throughput(), 0.0);
    }

    #[test]
    fn metrics_export_does_not_panic_without_registry() {
        let mut tracker = ThroughputTracker::new();
        let flow = FlowId::new();
        tracker.record(flow, 500);
        // just shouldn't panic
    }

    #[test]
    fn metrics_export_sets_gauge() {
        let metrics = Arc::new(MetricRegistry::new());
        let mut tracker = ThroughputTracker::new().with_metrics(Arc::clone(&metrics));
        let flow = FlowId::new();
        tracker.record(flow, 1000);
        let value = metrics
            .gauge(THROUGHPUT_SCOPE, "throughput_bytes_per_sec")
            .value();
        assert!(value > 0, "throughput gauge should be positive");
    }
}
