//! In-memory Flow Graph storage (see issue #29).

use std::collections::{HashMap, HashSet, VecDeque};

use nyarix_core::NodeId;
use nyarix_error::GraphError;
use nyarix_module_api::NodeType;

use crate::edge::Edge;
use crate::node::GraphNode;

/// In-memory storage for a Flow Graph: nodes, edges, and the adjacency
/// index used to traverse it.
///
/// This is storage and topology only — building a graph from
/// configuration (validating it, resolving conflicts) is the Graph
/// Builder's job (later M3 issues), and actually pushing packets through
/// it is the execution engine (#32+), not this struct.
#[derive(Default)]
pub struct FlowGraph {
    nodes: HashMap<NodeId, GraphNode>,
    edges: Vec<Edge>,
    /// `from -> [to, ...]`, kept in sync with `edges` for O(1) neighbor
    /// lookups during traversal.
    adjacency: HashMap<NodeId, Vec<NodeId>>,
    entry_points: Vec<NodeId>,
    exit_points: Vec<NodeId>,
}

impl FlowGraph {
    /// Create an empty graph.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a node to the graph.
    ///
    /// Nodes of type [`NodeType::Source`]/[`NodeType::Sink`] are
    /// automatically registered as entry/exit points; use
    /// [`Self::mark_entry_point`]/[`Self::mark_exit_point`] to also mark
    /// other node types (e.g. a diagnostic tap that both originates and
    /// terminates its own traffic).
    pub fn add_node(&mut self, node: GraphNode) {
        let id = node.id();
        match node.node_type() {
            NodeType::Source => self.mark_entry_point(id),
            NodeType::Sink => self.mark_exit_point(id),
            _ => {}
        }
        self.nodes.insert(id, node);
        self.adjacency.entry(id).or_default();
    }

    /// Remove a node and every edge touching it (incoming or outgoing).
    pub fn remove_node(&mut self, id: NodeId) -> Option<GraphNode> {
        let removed = self.nodes.remove(&id)?;
        self.edges
            .retain(|edge| edge.from() != id && edge.to() != id);
        self.adjacency.remove(&id);
        for targets in self.adjacency.values_mut() {
            targets.retain(|&target| target != id);
        }
        self.entry_points.retain(|&node| node != id);
        self.exit_points.retain(|&node| node != id);
        Some(removed)
    }

    /// Connect two existing nodes with an edge.
    ///
    /// # Errors
    /// Returns [`GraphError::MissingNode`] if either endpoint isn't
    /// currently in the graph.
    pub fn connect(&mut self, edge: Edge) -> Result<(), GraphError> {
        if !self.nodes.contains_key(&edge.from()) {
            return Err(GraphError::MissingNode {
                node_id: edge.from().to_string(),
            });
        }
        if !self.nodes.contains_key(&edge.to()) {
            return Err(GraphError::MissingNode {
                node_id: edge.to().to_string(),
            });
        }
        self.adjacency
            .entry(edge.from())
            .or_default()
            .push(edge.to());
        self.edges.push(edge);
        Ok(())
    }

    /// Remove and return the first edge from `from` to `to`, if any.
    ///
    /// If multiple parallel edges exist between the same pair of nodes
    /// (e.g. a `Fallback` alongside a `Sequential` edge), only the first
    /// one found is removed.
    pub fn disconnect(&mut self, from: NodeId, to: NodeId) -> Option<Edge> {
        let index = self
            .edges
            .iter()
            .position(|edge| edge.from() == from && edge.to() == to)?;
        let edge = self.edges.remove(index);
        if let Some(targets) = self.adjacency.get_mut(&from) {
            if let Some(pos) = targets.iter().position(|&target| target == to) {
                targets.remove(pos);
            }
        }
        Some(edge)
    }

    /// Find a path from `from` to `to`, following the adjacency structure
    /// only — this does **not** evaluate any edge [`crate::Condition`]s,
    /// it just answers "is `to` structurally reachable from `from`, and
    /// via which nodes". A breadth-first search, so the returned path (if
    /// any) has the fewest hops.
    #[must_use]
    pub fn find_path(&self, from: NodeId, to: NodeId) -> Option<Vec<NodeId>> {
        if !self.nodes.contains_key(&from) || !self.nodes.contains_key(&to) {
            return None;
        }
        if from == to {
            return Some(vec![from]);
        }

        let mut visited = HashSet::new();
        let mut queue = VecDeque::new();
        let mut predecessor: HashMap<NodeId, NodeId> = HashMap::new();

        visited.insert(from);
        queue.push_back(from);

        while let Some(current) = queue.pop_front() {
            let Some(neighbors) = self.adjacency.get(&current) else {
                continue;
            };
            for &next in neighbors {
                if visited.insert(next) {
                    predecessor.insert(next, current);
                    if next == to {
                        return Some(Self::reconstruct_path(&predecessor, from, to));
                    }
                    queue.push_back(next);
                }
            }
        }
        None
    }

    fn reconstruct_path(
        predecessor: &HashMap<NodeId, NodeId>,
        from: NodeId,
        to: NodeId,
    ) -> Vec<NodeId> {
        let mut path = vec![to];
        let mut current = to;
        while current != from {
            current = predecessor[&current];
            path.push(current);
        }
        path.reverse();
        path
    }

    /// Explicitly mark a node as a graph entry point.
    pub fn mark_entry_point(&mut self, id: NodeId) {
        if !self.entry_points.contains(&id) {
            self.entry_points.push(id);
        }
    }

    /// Explicitly mark a node as a graph exit point.
    pub fn mark_exit_point(&mut self, id: NodeId) {
        if !self.exit_points.contains(&id) {
            self.exit_points.push(id);
        }
    }

    /// Look up a node by id.
    #[must_use]
    pub fn node(&self, id: NodeId) -> Option<&GraphNode> {
        self.nodes.get(&id)
    }

    /// Number of nodes currently in the graph.
    #[must_use]
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Number of edges currently in the graph.
    #[must_use]
    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }

    /// Nodes marked as entry points.
    #[must_use]
    pub fn entry_points(&self) -> &[NodeId] {
        &self.entry_points
    }

    /// Nodes marked as exit points.
    #[must_use]
    pub fn exit_points(&self) -> &[NodeId] {
        &self.exit_points
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edge::EdgeType;
    use crate::node::NodeConfig;
    use nyarix_module_api::{Health, Module, ModuleMetadata, ModuleType, Node, RuntimeContext};
    use nyarix_packet::Packet;
    use std::sync::Arc;

    struct StubNode {
        metadata: ModuleMetadata,
        node_type: NodeType,
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
            Ok(Some(packet))
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

    fn node(name: &str, node_type: NodeType) -> GraphNode {
        let module: Arc<dyn Node> = Arc::new(StubNode::new(name, node_type));
        GraphNode::new(NodeId::new(), module, NodeConfig::default())
    }

    #[test]
    fn add_node_registers_source_as_entry_point() {
        let mut graph = FlowGraph::new();
        let source = node("source", NodeType::Source);
        let id = source.id();

        graph.add_node(source);

        assert_eq!(graph.node_count(), 1);
        assert_eq!(graph.entry_points(), &[id]);
        assert!(graph.exit_points().is_empty());
    }

    #[test]
    fn add_node_registers_sink_as_exit_point() {
        let mut graph = FlowGraph::new();
        let sink = node("sink", NodeType::Sink);
        let id = sink.id();

        graph.add_node(sink);

        assert_eq!(graph.exit_points(), &[id]);
        assert!(graph.entry_points().is_empty());
    }

    #[test]
    fn connect_fails_for_missing_node() {
        let mut graph = FlowGraph::new();
        let a = node("a", NodeType::Source);
        let a_id = a.id();
        graph.add_node(a);

        let (edge, _rx) = Edge::new(a_id, NodeId::new(), EdgeType::Sequential, None, 8);
        assert!(graph.connect(edge).is_err());
    }

    #[test]
    fn connect_succeeds_between_existing_nodes() {
        let mut graph = FlowGraph::new();
        let a = node("a", NodeType::Source);
        let b = node("b", NodeType::Sink);
        let (a_id, b_id) = (a.id(), b.id());
        graph.add_node(a);
        graph.add_node(b);

        let (edge, _rx) = Edge::new(a_id, b_id, EdgeType::Sequential, None, 8);
        graph.connect(edge).unwrap();

        assert_eq!(graph.edge_count(), 1);
        assert_eq!(graph.find_path(a_id, b_id), Some(vec![a_id, b_id]));
    }

    #[test]
    fn find_path_across_multiple_hops() {
        let mut graph = FlowGraph::new();
        let a = node("a", NodeType::Source);
        let b = node("b", NodeType::Transformer);
        let c = node("c", NodeType::Sink);
        let (a_id, b_id, c_id) = (a.id(), b.id(), c.id());
        graph.add_node(a);
        graph.add_node(b);
        graph.add_node(c);

        let (edge_ab, _rx1) = Edge::new(a_id, b_id, EdgeType::Sequential, None, 8);
        let (edge_bc, _rx2) = Edge::new(b_id, c_id, EdgeType::Sequential, None, 8);
        graph.connect(edge_ab).unwrap();
        graph.connect(edge_bc).unwrap();

        assert_eq!(graph.find_path(a_id, c_id), Some(vec![a_id, b_id, c_id]));
    }

    #[test]
    fn find_path_returns_none_when_unreachable() {
        let mut graph = FlowGraph::new();
        let a = node("a", NodeType::Source);
        let b = node("b", NodeType::Sink);
        let (a_id, b_id) = (a.id(), b.id());
        graph.add_node(a);
        graph.add_node(b);

        assert_eq!(graph.find_path(a_id, b_id), None);
    }

    #[test]
    fn disconnect_removes_edge() {
        let mut graph = FlowGraph::new();
        let a = node("a", NodeType::Source);
        let b = node("b", NodeType::Sink);
        let (a_id, b_id) = (a.id(), b.id());
        graph.add_node(a);
        graph.add_node(b);

        let (edge, _rx) = Edge::new(a_id, b_id, EdgeType::Sequential, None, 8);
        graph.connect(edge).unwrap();
        assert!(graph.disconnect(a_id, b_id).is_some());
        assert_eq!(graph.edge_count(), 0);
        assert_eq!(graph.find_path(a_id, b_id), None);
    }

    #[test]
    fn remove_node_cleans_up_edges_and_entry_points() {
        let mut graph = FlowGraph::new();
        let a = node("a", NodeType::Source);
        let b = node("b", NodeType::Sink);
        let (a_id, b_id) = (a.id(), b.id());
        graph.add_node(a);
        graph.add_node(b);

        let (edge, _rx) = Edge::new(a_id, b_id, EdgeType::Sequential, None, 8);
        graph.connect(edge).unwrap();

        graph.remove_node(a_id);

        assert_eq!(graph.node_count(), 1);
        assert_eq!(graph.edge_count(), 0);
        assert!(graph.entry_points().is_empty());
        assert!(graph.node(a_id).is_none());
    }
}
