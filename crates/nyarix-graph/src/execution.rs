//! Sequential graph execution (see issue #32).
//!
//! Linear traversal only: a packet enters at one node, follows exactly
//! one outgoing edge per hop (the first whose [`Condition`](crate::Condition)
//! accepts it), and either reaches an exit point or is absorbed along the
//! way. Parallel fan-out (#33), async decoupling via the edge queues
//! (#34), and backpressure (#35) are separate, later pieces of the
//! execution engine — this is deliberately the simplest possible runner.

use nyarix_core::NodeId;
use nyarix_error::GraphError;
use nyarix_packet::Packet;
use thiserror::Error;

use crate::graph::FlowGraph;

/// Error produced while executing a packet through the graph.
#[derive(Debug, Error)]
pub enum ExecutionError {
    /// A node's module failed to process the packet.
    #[error("module processing failed: {0}")]
    Module(#[from] nyarix_error::ModuleError),
    /// A graph-structural problem (missing node, dead end, ...).
    #[error("graph error: {0}")]
    Graph(#[from] GraphError),
}

/// Run a packet through the graph starting at `entry`, following
/// [`FlowGraph`] adjacency one hop at a time.
///
/// Returns `Ok(Some(packet))` once the packet reaches an exit point,
/// `Ok(None)` if some node along the way absorbed it (see #19), or `Err`
/// if a module failed or the path dead-ends before reaching an exit.
///
/// # Errors
/// Returns [`ExecutionError::Graph`] with [`GraphError::MissingNode`] if
/// `entry` isn't an entry point, or [`GraphError::BuildFailed`] if
/// execution reaches a node with no outgoing edge accepting the packet
/// and that node isn't an exit point — this indicates the graph wasn't
/// validated (see [`FlowGraph::validate`]) before running it.
/// Returns [`ExecutionError::Module`] if a node's `process` call fails.
pub fn execute_sequential(
    graph: &mut FlowGraph,
    entry: NodeId,
    mut packet: Packet,
) -> Result<Option<Packet>, ExecutionError> {
    if !graph.entry_points().contains(&entry) {
        return Err(GraphError::MissingNode {
            node_id: entry.to_string(),
        }
        .into());
    }

    let mut current = entry;
    loop {
        let node = graph
            .node_mut(current)
            .ok_or_else(|| GraphError::MissingNode {
                node_id: current.to_string(),
            })?;

        packet = match node.process(packet)? {
            Some(packet) => packet,
            None => return Ok(None),
        };

        if graph.exit_points().contains(&current) {
            return Ok(Some(packet));
        }

        current = graph
            .edges_from(current)
            .find(|edge| edge.accepts(&packet))
            .map(crate::edge::Edge::to)
            .ok_or_else(|| GraphError::BuildFailed {
                reason: format!(
                    "node {current} has no outgoing edge accepting this packet \
                     (and isn't an exit point) — run FlowGraph::validate() first"
                ),
            })?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edge::EdgeType;
    use crate::node::{GraphNode, NodeConfig};
    use nyarix_module_api::{
        Health, Module, ModuleMetadata, ModuleType, Node, NodeType, RuntimeContext,
    };
    use std::sync::Arc;

    struct StubNode {
        metadata: ModuleMetadata,
        node_type: NodeType,
        absorb: bool,
    }

    impl StubNode {
        fn new(name: &str, node_type: NodeType) -> Self {
            Self {
                metadata: ModuleMetadata::new(
                    name,
                    semver::Version::new(0, 1, 0),
                    ModuleType::Flow,
                ),
                node_type,
                absorb: false,
            }
        }

        fn absorbing(name: &str, node_type: NodeType) -> Self {
            Self {
                absorb: true,
                ..Self::new(name, node_type)
            }
        }
    }

    impl Module for StubNode {
        fn metadata(&self) -> &ModuleMetadata {
            &self.metadata
        }

        fn initialize(&mut self, _ctx: &RuntimeContext) -> Result<(), nyarix_error::ModuleError> {
            Ok(())
        }

        fn process(&mut self, packet: Packet) -> Result<Option<Packet>, nyarix_error::ModuleError> {
            if self.absorb {
                Ok(None)
            } else {
                Ok(Some(packet))
            }
        }

        fn shutdown(&mut self, _ctx: &RuntimeContext) -> Result<(), nyarix_error::ModuleError> {
            Ok(())
        }

        fn health(&self) -> Health {
            Health::Healthy
        }
    }

    impl Node for StubNode {
        fn node_type(&self) -> NodeType {
            self.node_type
        }

        fn input_queue_depth(&self) -> usize {
            0
        }

        fn output_connections(&self) -> &[NodeId] {
            &[]
        }
    }

    fn node(module: StubNode) -> GraphNode {
        let module: Arc<dyn Node> = Arc::new(module);
        GraphNode::new(NodeId::new(), module, NodeConfig::default())
    }

    #[test]
    fn runs_a_linear_graph_to_the_exit() {
        let mut graph = FlowGraph::new();
        let a = node(StubNode::new("source", NodeType::Source));
        let b = node(StubNode::new("transformer", NodeType::Transformer));
        let c = node(StubNode::new("sink", NodeType::Sink));
        let (a_id, b_id, c_id) = (a.id(), b.id(), c.id());
        graph.add_node(a);
        graph.add_node(b);
        graph.add_node(c);

        let (ab, _rx1) = crate::edge::Edge::new(a_id, b_id, EdgeType::Sequential, None, 8);
        let (bc, _rx2) = crate::edge::Edge::new(b_id, c_id, EdgeType::Sequential, None, 8);
        graph.connect(ab).unwrap();
        graph.connect(bc).unwrap();

        let pkt = Packet::new(b"hello".as_slice());
        let id = pkt.id();

        let result = execute_sequential(&mut graph, a_id, pkt).unwrap();
        assert_eq!(result.unwrap().id(), id);
    }

    #[test]
    fn absorbed_packet_returns_none() {
        let mut graph = FlowGraph::new();
        let a = node(StubNode::new("source", NodeType::Source));
        let b = node(StubNode::absorbing("filter", NodeType::Filter));
        let c = node(StubNode::new("sink", NodeType::Sink));
        let (a_id, b_id, c_id) = (a.id(), b.id(), c.id());
        graph.add_node(a);
        graph.add_node(b);
        graph.add_node(c);

        let (ab, _rx1) = crate::edge::Edge::new(a_id, b_id, EdgeType::Sequential, None, 8);
        let (bc, _rx2) = crate::edge::Edge::new(b_id, c_id, EdgeType::Sequential, None, 8);
        graph.connect(ab).unwrap();
        graph.connect(bc).unwrap();

        let result = execute_sequential(&mut graph, a_id, Packet::new(b"data".as_slice())).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn dead_end_before_exit_is_an_error() {
        let mut graph = FlowGraph::new();
        let a = node(StubNode::new("source", NodeType::Source));
        let b = node(StubNode::new("dangling", NodeType::Transformer));
        let (a_id, b_id) = (a.id(), b.id());
        graph.add_node(a);
        graph.add_node(b);

        let (ab, _rx) = crate::edge::Edge::new(a_id, b_id, EdgeType::Sequential, None, 8);
        graph.connect(ab).unwrap();
        // `b` has no outgoing edge and isn't an exit point.

        let result = execute_sequential(&mut graph, a_id, Packet::new(b"data".as_slice()));
        assert!(matches!(
            result,
            Err(ExecutionError::Graph(GraphError::BuildFailed { .. }))
        ));
    }

    #[test]
    fn rejects_non_entry_point() {
        let mut graph = FlowGraph::new();
        let a = node(StubNode::new("transformer", NodeType::Transformer));
        let a_id = a.id();
        graph.add_node(a);

        let result = execute_sequential(&mut graph, a_id, Packet::new(b"data".as_slice()));
        assert!(matches!(
            result,
            Err(ExecutionError::Graph(GraphError::MissingNode { .. }))
        ));
    }
}
