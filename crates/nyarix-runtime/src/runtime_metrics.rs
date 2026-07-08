//! Runtime-level metrics (see issue #83): uptime, module load results,
//! and the flow graph's shape — as opposed to #81's per-flow and #82's
//! per-node metrics, these describe the Runtime as a whole.
//!
//! **Scope note:** these are recording primitives operating on
//! whatever the caller already has in hand (a start [`Instant`], a
//! [`ModuleLoadReport`], a [`FlowGraph`]) — they don't wire themselves
//! into [`crate::init::RuntimeHandle`]'s actual startup sequence, since
//! `RuntimeHandle` doesn't hold a start time, a [`ModuleLoadReport`],
//! or a live graph yet (per its own doc comment) and calling
//! `load_modules`/building the graph automatically at startup is
//! already tracked separately (#112, #103) — this module is what that
//! wiring should call once it exists, not a reason to guess at it here.
//!
//! **Scope note on `active_flows`:** not implemented — this workspace
//! has no concept of an "active flow" to count (no session/flow
//! registry exists; [`nyarix_core::FlowId`] is just an id type, nothing
//! tracks which ones are currently live). Tracked separately.
//!
//! **Scope note on `memory_usage_bytes`:** not implemented — same
//! underlying decision #116 already defers for per-module memory
//! (which OS/jemalloc mechanism, and it's platform-specific in a way
//! that matters for this project's mobile targets), just applied
//! Runtime-wide instead of per-module. No second issue filed; once #116
//! picks a measurement mechanism, exposing a Runtime-total gauge here
//! is a trivial reuse of it.

use std::time::Instant;

use nyarix_graph::FlowGraph;
use nyarix_module_api::MetricRegistry;

use crate::module_loader::ModuleLoadReport;

const RUNTIME_METRICS_SCOPE: &str = "runtime";

/// Record how long the Runtime has been up, in whole seconds, as of
/// `started_at`.
pub fn record_uptime(metrics: &MetricRegistry, started_at: Instant) {
    let seconds = i64::try_from(started_at.elapsed().as_secs()).unwrap_or(i64::MAX);
    metrics
        .gauge(RUNTIME_METRICS_SCOPE, "uptime_seconds")
        .set(seconds);
}

/// Record `modules_loaded` (packages that passed validation) and
/// `modules_failed` (everything else in `report` — packages that
/// failed validation, plus files that couldn't even be read as a
/// `.nyp` archive) from a [`ModuleLoadReport`] (#41).
pub fn record_module_load_report(metrics: &MetricRegistry, report: &ModuleLoadReport) {
    let loaded = i64::try_from(report.valid.len()).unwrap_or(i64::MAX);
    let failed = i64::try_from(report.invalid.len() + report.errors.len()).unwrap_or(i64::MAX);
    metrics
        .gauge(RUNTIME_METRICS_SCOPE, "modules_loaded")
        .set(loaded);
    metrics
        .gauge(RUNTIME_METRICS_SCOPE, "modules_failed")
        .set(failed);
}

/// Record `graph_depth`: the longest entry-to-exit path length (in
/// nodes) across every entry/exit pair in `graph`.
///
/// [`FlowGraph::find_path`] returns the *shortest* path between two
/// nodes (breadth-first search) — this only equals the graph's true
/// longest path when there's at most one path between any entry/exit
/// pair, which holds for every graph this workspace can currently
/// build ([`crate::graph_builder::build_from_profile`] only produces
/// linear chains); a graph with multiple differently-sized routes
/// between the same entry and exit would need a real longest-path
/// computation, not this reuse of `find_path`.
pub fn record_graph_depth(metrics: &MetricRegistry, graph: &FlowGraph) {
    let depth = graph
        .entry_points()
        .iter()
        .flat_map(|&entry| {
            graph
                .exit_points()
                .iter()
                .filter_map(move |&exit| graph.find_path(entry, exit))
        })
        .map(|path| path.len())
        .max()
        .unwrap_or(0);
    let depth = i64::try_from(depth).unwrap_or(i64::MAX);
    metrics
        .gauge(RUNTIME_METRICS_SCOPE, "graph_depth")
        .set(depth);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::module_loader::{LoadedPackage, RejectedPackage};
    use nyarix_error::ModuleError;
    use nyarix_graph::{Edge, EdgeType, GraphNode, NodeConfig};
    use nyarix_loader::ValidationReport;
    use nyarix_module_api::{
        Health, Module, ModuleMetadata, ModuleType, Node, NodeType, RuntimeContext,
    };
    use nyarix_package::{SignatureStatus, TrustLevel};
    use nyarix_packet::Packet;
    use std::sync::Arc;
    use std::time::Duration;

    #[test]
    fn uptime_reflects_elapsed_time_since_start() {
        let metrics = MetricRegistry::new();
        let started_at = Instant::now() - Duration::from_secs(5);

        record_uptime(&metrics, started_at);

        assert!(metrics.gauge("runtime", "uptime_seconds").value() >= 5);
    }

    fn validation_report(platform_supported: bool) -> ValidationReport {
        ValidationReport {
            signature_status: SignatureStatus::Unsigned,
            trust_level: TrustLevel::Unknown,
            conflicts: Vec::new(),
            platform_supported,
        }
    }

    #[test]
    fn module_load_report_counts_loaded_and_failed() {
        let metrics = MetricRegistry::new();
        let report = ModuleLoadReport {
            valid: vec![LoadedPackage {
                path: "a.nyp".into(),
                data: vec![],
                validation: validation_report(true),
            }],
            invalid: vec![RejectedPackage {
                path: "b.nyp".into(),
                validation: validation_report(false),
            }],
            errors: vec![],
        };

        record_module_load_report(&metrics, &report);

        assert_eq!(metrics.gauge("runtime", "modules_loaded").value(), 1);
        assert_eq!(metrics.gauge("runtime", "modules_failed").value(), 1);
    }

    struct StubNode {
        metadata: ModuleMetadata,
    }

    impl Module for StubNode {
        fn metadata(&self) -> &ModuleMetadata {
            &self.metadata
        }
        fn initialize(&mut self, _ctx: &RuntimeContext) -> Result<(), ModuleError> {
            Ok(())
        }
        fn process(&mut self, packet: Packet) -> Result<Option<Packet>, ModuleError> {
            Ok(Some(packet))
        }
        fn shutdown(&mut self, _ctx: &RuntimeContext) -> Result<(), ModuleError> {
            Ok(())
        }
        fn health(&self) -> Health {
            Health::Healthy
        }
    }

    impl Node for StubNode {
        fn node_type(&self) -> NodeType {
            NodeType::Transformer
        }
        fn input_queue_depth(&self) -> usize {
            0
        }
        fn output_connections(&self) -> &[nyarix_core::NodeId] {
            &[]
        }
    }

    fn stub_node(name: &str) -> GraphNode {
        let module: Arc<dyn Node> = Arc::new(StubNode {
            metadata: ModuleMetadata::new(name, semver::Version::new(0, 1, 0), ModuleType::Flow),
        });
        GraphNode::new(nyarix_core::NodeId::new(), module, NodeConfig::default())
    }

    #[test]
    fn graph_depth_counts_the_longest_entry_to_exit_path() {
        let metrics = MetricRegistry::new();
        let mut graph = FlowGraph::new();
        let a = stub_node("a");
        let b = stub_node("b");
        let c = stub_node("c");
        let (a_id, b_id, c_id) = (a.id(), b.id(), c.id());
        graph.add_node(a);
        graph.add_node(b);
        graph.add_node(c);
        graph.mark_entry_point(a_id);
        graph.mark_exit_point(c_id);
        let (ab, _rx1) = Edge::new(a_id, b_id, EdgeType::Sequential, None, 8);
        let (bc, _rx2) = Edge::new(b_id, c_id, EdgeType::Sequential, None, 8);
        graph.connect(ab).unwrap();
        graph.connect(bc).unwrap();

        record_graph_depth(&metrics, &graph);

        assert_eq!(metrics.gauge("runtime", "graph_depth").value(), 3);
    }

    #[test]
    fn graph_depth_is_zero_for_a_graph_with_no_entry_or_exit_points() {
        let metrics = MetricRegistry::new();
        let graph = FlowGraph::new();

        record_graph_depth(&metrics, &graph);

        assert_eq!(metrics.gauge("runtime", "graph_depth").value(), 0);
    }
}
