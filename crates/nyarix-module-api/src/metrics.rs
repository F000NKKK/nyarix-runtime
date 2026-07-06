//! Metrics handle (see issue #18; the real system is M8).

/// Handle a module uses to record metrics.
///
/// Currently a marker with no methods — recording/registration API design
/// is M8 (Metrics & Observability, see #80 Metric registry). Handed out by
/// `RuntimeContext::metrics()` so module code can already take `&MetricsHandle`
/// in its signatures without churn once M8 lands.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MetricsHandle {
    _private: (),
}
