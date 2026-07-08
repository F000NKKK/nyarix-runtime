//! Graph visualization export (see issue #86): DOT (Graphviz) and JSON
//! snapshots of a [`FlowGraph`]'s current shape.
//!
//! **Scope note:** "Эндпоинт в Runtime API для получения дампа графа"
//! isn't implemented — this workspace has no HTTP/RPC server framework
//! anywhere (no `axum`/`hyper`/`tonic`/etc. dependency), so there's no
//! existing API surface to attach an endpoint to; [`export_graph`] is
//! the data-producing half a future endpoint would call. Deciding on
//! and adding a whole API server is out of scope for a graph-export
//! issue.

use std::collections::HashSet;

use nyarix_core::NodeId;
use nyarix_module_api::{MetricRegistry, NodeType};
use serde::{Deserialize, Serialize};

use crate::edge::EdgeType;
use crate::graph::FlowGraph;
use crate::node::NodeState;

/// A snapshot of #82's per-node metrics for one node, if a
/// [`MetricRegistry`] was attached to [`export_graph`] and it has
/// recorded anything for that node yet.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeMetricsExport {
    /// See #82's `process_calls_total`.
    pub process_calls_total: u64,
    /// See #82's `errors_total`.
    pub errors_total: u64,
    /// See #82's `queue_depth`.
    pub queue_depth: i64,
}

/// One node in a [`GraphExport`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeExport {
    /// The node's [`NodeId`], as a string (DOT/JSON both want a plain
    /// identifier, not a typed one).
    pub id: String,
    /// The underlying module's name.
    pub name: String,
    /// The node's role in the graph.
    pub node_type: NodeType,
    /// The node's current lifecycle state ("status").
    pub state: NodeState,
    /// Whether this node is one of the graph's entry points.
    pub is_entry: bool,
    /// Whether this node is one of the graph's exit points.
    pub is_exit: bool,
    /// #82's per-node metrics, if a registry was attached and has
    /// recorded anything for this node yet.
    pub metrics: Option<NodeMetricsExport>,
}

/// One edge in a [`GraphExport`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EdgeExport {
    /// Source node id.
    pub from: String,
    /// Destination node id.
    pub to: String,
    /// How this edge routes packets.
    pub edge_type: EdgeType,
    /// The edge's current backpressure queue depth (#35).
    pub queue_depth: usize,
    /// Packets dropped on this edge so far (#35).
    pub dropped_count: u64,
}

/// A full snapshot of a [`FlowGraph`]'s nodes and edges, ready to
/// render as DOT or JSON.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GraphExport {
    /// Every node in the graph.
    pub nodes: Vec<NodeExport>,
    /// Every edge in the graph.
    pub edges: Vec<EdgeExport>,
}

/// Build a [`GraphExport`] of `graph`'s current shape.
///
/// If `metrics` is attached, each node's export includes whatever
/// #82's per-node metrics are currently recorded for it, looked up by
/// its module name (the same key [`crate::execution`]'s
/// `record_node_metrics` uses) via
/// [`MetricRegistry::get_counter`]/[`MetricRegistry::get_gauge`] — the
/// read-only lookups, not [`MetricRegistry::counter`]/
/// [`MetricRegistry::gauge`], so exporting a graph never has the side
/// effect of registering zero-value metrics for nodes that haven't
/// processed anything yet. A node with nothing recorded gets
/// `metrics: None`, not a zeroed-out [`NodeMetricsExport`].
#[must_use]
pub fn export_graph(graph: &FlowGraph, metrics: Option<&MetricRegistry>) -> GraphExport {
    let entry_points: HashSet<NodeId> = graph.entry_points().iter().copied().collect();
    let exit_points: HashSet<NodeId> = graph.exit_points().iter().copied().collect();

    let mut nodes = Vec::new();
    let mut edges = Vec::new();

    for id in graph.node_ids() {
        let Some(node) = graph.node(id) else {
            continue;
        };
        let name = node.module().metadata().name.clone();
        let node_metrics = metrics.and_then(|registry| node_metrics_export(registry, &name));

        nodes.push(NodeExport {
            id: id.to_string(),
            name,
            node_type: node.node_type(),
            state: node.state(),
            is_entry: entry_points.contains(&id),
            is_exit: exit_points.contains(&id),
            metrics: node_metrics,
        });

        for edge in graph.edges_from(id) {
            edges.push(EdgeExport {
                from: edge.from().to_string(),
                to: edge.to().to_string(),
                edge_type: edge.edge_type(),
                queue_depth: edge.queue_depth(),
                dropped_count: edge.dropped_count(),
            });
        }
    }

    GraphExport { nodes, edges }
}

/// `None` unless at least one of #82's per-node metrics was actually
/// recorded for `name` — a node nothing has ever processed through
/// gets no metrics entry rather than a `Some` full of zeros
/// indistinguishable from "recorded, and happened to be zero".
fn node_metrics_export(registry: &MetricRegistry, name: &str) -> Option<NodeMetricsExport> {
    let process_calls_total = registry.get_counter(name, "process_calls_total");
    let errors_total = registry.get_counter(name, "errors_total");
    let queue_depth = registry.get_gauge(name, "queue_depth");
    if process_calls_total.is_none() && errors_total.is_none() && queue_depth.is_none() {
        return None;
    }
    Some(NodeMetricsExport {
        process_calls_total: process_calls_total.map_or(0, |c| c.value()),
        errors_total: errors_total.map_or(0, |c| c.value()),
        queue_depth: queue_depth.map_or(0, |g| g.value()),
    })
}

impl GraphExport {
    /// Serialize as pretty-printed JSON (this issue's "JSON-формат
    /// (для web UI)").
    #[must_use]
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".to_string())
    }

    /// Serialize as a Graphviz DOT digraph (this issue's "DOT-формат
    /// (Graphviz)") — entry points are drawn as double circles, exit
    /// points as squares, everything else as plain ellipses (DOT's
    /// default), labeled with the node's name, type, and state.
    #[must_use]
    pub fn to_dot(&self) -> String {
        let mut dot = String::from("digraph FlowGraph {\n");
        for node in &self.nodes {
            let shape = if node.is_entry {
                "doublecircle"
            } else if node.is_exit {
                "square"
            } else {
                "ellipse"
            };
            dot.push_str(&format!(
                "  \"{}\" [label=\"{}\\n{:?}\\n{:?}\", shape={}];\n",
                node.id, node.name, node.node_type, node.state, shape
            ));
        }
        for edge in &self.edges {
            dot.push_str(&format!(
                "  \"{}\" -> \"{}\" [label=\"{:?}\"];\n",
                edge.from, edge.to, edge.edge_type
            ));
        }
        dot.push_str("}\n");
        dot
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edge::Edge;
    use crate::node::{GraphNode, NodeConfig};
    use nyarix_error::ModuleError;
    use nyarix_module_api::{Health, Module, ModuleMetadata, ModuleType, Node, RuntimeContext};
    use nyarix_packet::Packet;
    use std::sync::Arc;

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
        fn output_connections(&self) -> &[NodeId] {
            &[]
        }
    }

    fn stub_node(name: &str) -> GraphNode {
        let module: Arc<dyn Node> = Arc::new(StubNode {
            metadata: ModuleMetadata::new(name, semver::Version::new(0, 1, 0), ModuleType::Flow),
        });
        GraphNode::new(NodeId::new(), module, NodeConfig::default())
    }

    fn linear_graph() -> (FlowGraph, NodeId, NodeId) {
        let mut graph = FlowGraph::new();
        let a = stub_node("a");
        let b = stub_node("b");
        let (a_id, b_id) = (a.id(), b.id());
        graph.add_node(a);
        graph.add_node(b);
        graph.mark_entry_point(a_id);
        graph.mark_exit_point(b_id);
        let (edge, _rx) = Edge::new(a_id, b_id, EdgeType::Sequential, None, 8);
        graph.connect(edge).unwrap();
        (graph, a_id, b_id)
    }

    #[test]
    fn export_includes_every_node_and_edge() {
        let (graph, a_id, b_id) = linear_graph();
        let export = export_graph(&graph, None);

        assert_eq!(export.nodes.len(), 2);
        assert_eq!(export.edges.len(), 1);
        assert_eq!(export.edges[0].from, a_id.to_string());
        assert_eq!(export.edges[0].to, b_id.to_string());
    }

    #[test]
    fn export_marks_entry_and_exit_points() {
        let (graph, a_id, b_id) = linear_graph();
        let export = export_graph(&graph, None);

        let a = export
            .nodes
            .iter()
            .find(|n| n.id == a_id.to_string())
            .unwrap();
        let b = export
            .nodes
            .iter()
            .find(|n| n.id == b_id.to_string())
            .unwrap();
        assert!(a.is_entry);
        assert!(!a.is_exit);
        assert!(b.is_exit);
        assert!(!b.is_entry);
    }

    #[test]
    fn export_has_no_metrics_without_a_registry() {
        let (graph, _a_id, _b_id) = linear_graph();
        let export = export_graph(&graph, None);
        assert!(export.nodes.iter().all(|n| n.metrics.is_none()));
    }

    #[test]
    fn export_includes_metrics_when_recorded() {
        let (graph, _a_id, _b_id) = linear_graph();
        let metrics = MetricRegistry::new();
        metrics.counter("a", "process_calls_total").increment(5);
        metrics.gauge("a", "queue_depth").set(2);

        let export = export_graph(&graph, Some(&metrics));

        let a = export.nodes.iter().find(|n| n.name == "a").unwrap();
        let b = export.nodes.iter().find(|n| n.name == "b").unwrap();
        let a_metrics = a.metrics.as_ref().unwrap();
        assert_eq!(a_metrics.process_calls_total, 5);
        assert_eq!(a_metrics.queue_depth, 2);
        assert!(b.metrics.is_none());
    }

    #[test]
    fn to_json_round_trips_through_serde_json() {
        let (graph, _a_id, _b_id) = linear_graph();
        let export = export_graph(&graph, None);
        let json = export.to_json();
        let parsed: GraphExport = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, export);
    }

    #[test]
    fn to_dot_includes_every_node_and_edge() {
        let (graph, _a_id, _b_id) = linear_graph();
        let export = export_graph(&graph, None);
        let dot = export.to_dot();

        assert!(dot.starts_with("digraph FlowGraph {"));
        assert!(dot.contains('a'));
        assert!(dot.contains('b'));
        assert!(dot.contains("->"));
        assert!(dot.contains("doublecircle"));
        assert!(dot.contains("square"));
    }
}
