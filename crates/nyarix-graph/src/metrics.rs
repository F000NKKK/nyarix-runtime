//! Per-node metrics (see issue #27; the full metrics system is M8).

/// Metrics tracked for a single graph node.
///
/// Currently a marker with no fields — the metric registry design (M8,
/// see #82 Node-level metrics: per-node stats) hasn't landed yet. Held on
/// [`crate::node::GraphNode`] so the field exists and the shape of
/// `GraphNode` doesn't change again once M8 does land.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NodeMetrics {
    _private: (),
}
