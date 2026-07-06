//! In-memory Flow Graph storage (see issue #29).

use std::collections::{HashMap, HashSet, VecDeque};

use nyarix_core::NodeId;
use nyarix_error::GraphError;
use nyarix_module_api::NodeType;

use crate::edge::{Edge, EdgeType};
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
    /// Edges explicitly allowed to close a cycle — intentional feedback
    /// loops (see #31's "explicit allowlist" option). Cycle detection
    /// skips traversing these edges, so a back-edge here can't be blamed
    /// for an otherwise-illegal cycle.
    allowed_feedback_edges: HashSet<(NodeId, NodeId)>,
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

    /// Look up a node by id, mutably.
    #[must_use]
    pub fn node_mut(&mut self, id: NodeId) -> Option<&mut GraphNode> {
        self.nodes.get_mut(&id)
    }

    /// Iterate the edges whose source is `id`.
    pub fn edges_from(&self, id: NodeId) -> impl Iterator<Item = &Edge> {
        self.edges.iter().filter(move |edge| edge.from() == id)
    }

    /// Number of nodes currently in the graph.
    #[must_use]
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Iterate the ids of every node currently in the graph, in no
    /// particular order (used by the execution loop, see #43, to
    /// initialize/shut down every node).
    pub fn node_ids(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.nodes.keys().copied()
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

    /// Mark an edge as an intentional feedback loop (see #31): cycle
    /// detection will not traverse it, so it can't be blamed for forming
    /// an illegal cycle. Does not affect routing — the edge still carries
    /// packets normally.
    pub fn allow_feedback_edge(&mut self, from: NodeId, to: NodeId) {
        self.allowed_feedback_edges.insert((from, to));
    }

    /// Detect a cycle in the graph via DFS, ignoring edges marked with
    /// [`Self::allow_feedback_edge`].
    ///
    /// Returns the node ids forming the cycle, in traversal order, with
    /// the starting node repeated at the end (e.g. `[a, b, c, a]`) — see
    /// [`Self::describe_cycle`] to render this as a message like
    /// `"Router → Fragmenter → Compressor → Router"`.
    #[must_use]
    pub fn detect_cycle(&self) -> Option<Vec<NodeId>> {
        let mut marks: HashMap<NodeId, DfsMark> = HashMap::new();
        let mut stack: Vec<NodeId> = Vec::new();

        for &start in self.nodes.keys() {
            if marks.contains_key(&start) {
                continue;
            }
            if let Some(cycle) = self.dfs_detect_cycle(start, &mut marks, &mut stack) {
                return Some(cycle);
            }
        }
        None
    }

    fn dfs_detect_cycle(
        &self,
        node: NodeId,
        marks: &mut HashMap<NodeId, DfsMark>,
        stack: &mut Vec<NodeId>,
    ) -> Option<Vec<NodeId>> {
        marks.insert(node, DfsMark::Visiting);
        stack.push(node);

        if let Some(neighbors) = self.adjacency.get(&node) {
            for &next in neighbors {
                if self.allowed_feedback_edges.contains(&(node, next)) {
                    continue;
                }
                match marks.get(&next) {
                    Some(DfsMark::Visiting) => {
                        let start_pos = stack.iter().position(|&n| n == next).unwrap_or(0);
                        let mut cycle = stack[start_pos..].to_vec();
                        cycle.push(next);
                        return Some(cycle);
                    }
                    Some(DfsMark::Visited) => {}
                    None => {
                        if let Some(cycle) = self.dfs_detect_cycle(next, marks, stack) {
                            return Some(cycle);
                        }
                    }
                }
            }
        }

        stack.pop();
        marks.insert(node, DfsMark::Visited);
        None
    }

    /// Render a cycle (as returned by [`Self::detect_cycle`]) as a
    /// human-readable chain of module names, e.g.
    /// `"Router → Fragmenter → Compressor → Router"`.
    #[must_use]
    pub fn describe_cycle(&self, cycle: &[NodeId]) -> String {
        cycle
            .iter()
            .map(|id| {
                self.node(*id).map_or_else(
                    || id.to_string(),
                    |node| node.module().metadata().name.clone(),
                )
            })
            .collect::<Vec<_>>()
            .join(" → ")
    }

    /// Validate that this graph is well-formed:
    /// - acyclic (modulo [`Self::allow_feedback_edge`]-marked edges)
    /// - every entry point can reach at least one exit point
    /// - no orphaned nodes (nodes with neither incoming nor outgoing
    ///   edges), except entry/exit points themselves
    ///
    /// **Not yet checked:** node-type compatibility at edge boundaries
    /// (e.g. does it make sense for a `Router` to feed an `Encryptor`?) —
    /// tracked separately (see the issue comment) since it needs a
    /// compatibility matrix that doesn't exist anywhere yet.
    ///
    /// # Errors
    /// Returns [`GraphError::Cycle`] if a cycle is found, or
    /// [`GraphError::BuildFailed`] describing the first reachability or
    /// orphan violation found.
    pub fn validate(&self) -> Result<(), GraphError> {
        if let Some(cycle) = self.detect_cycle() {
            return Err(GraphError::Cycle {
                cycle: self.describe_cycle(&cycle),
            });
        }

        for &entry in &self.entry_points {
            let reaches_an_exit = self
                .exit_points
                .iter()
                .any(|&exit| self.find_path(entry, exit).is_some());
            if !reaches_an_exit {
                return Err(GraphError::BuildFailed {
                    reason: format!("entry point {entry} cannot reach any exit point"),
                });
            }
        }

        for &id in self.nodes.keys() {
            if self.entry_points.contains(&id) || self.exit_points.contains(&id) {
                continue;
            }
            let has_incoming = self.edges.iter().any(|edge| edge.to() == id);
            let has_outgoing = self.adjacency.get(&id).is_some_and(|out| !out.is_empty());
            if !has_incoming && !has_outgoing {
                let name = self.node(id).map_or_else(
                    || id.to_string(),
                    |node| node.module().metadata().name.clone(),
                );
                return Err(GraphError::BuildFailed {
                    reason: format!(
                        "node '{name}' ({id}) is orphaned: no incoming or outgoing edges"
                    ),
                });
            }
        }

        Ok(())
    }

    /// Insert `new_node` immediately after `after_id`, splicing it into
    /// `after_id`'s single outgoing edge if it has exactly one (see issue
    /// #37): `after_id -[old edge_type/condition]-> new_node
    /// -[Sequential]-> old_target`. If `after_id` has no outgoing edge yet,
    /// just connects `after_id -[Sequential]-> new_node`.
    ///
    /// **Not implemented here** (needs a live execution engine, which
    /// doesn't exist yet — M4): pausing packet flow / draining in-flight
    /// packets during the splice. This only mutates the topology; running
    /// [`Self::validate`] afterward and coordinating with the Scheduler is
    /// the caller's job for now (see the issue comment for a tracked
    /// follow-up).
    ///
    /// # Errors
    /// Returns [`GraphError::MissingNode`] if `after_id` doesn't exist, or
    /// [`GraphError::BuildFailed`] if `after_id` has more than one
    /// outgoing edge (ambiguous which one to splice into).
    pub fn insert_after(
        &mut self,
        after_id: NodeId,
        new_node: GraphNode,
    ) -> Result<(), GraphError> {
        if !self.nodes.contains_key(&after_id) {
            return Err(GraphError::MissingNode {
                node_id: after_id.to_string(),
            });
        }

        let outgoing: Vec<usize> = self
            .edges
            .iter()
            .enumerate()
            .filter(|(_, edge)| edge.from() == after_id)
            .map(|(index, _)| index)
            .collect();

        if outgoing.len() > 1 {
            return Err(GraphError::BuildFailed {
                reason: format!(
                    "node {after_id} has {} outgoing edges; insert_after is ambiguous here",
                    outgoing.len()
                ),
            });
        }

        let new_id = new_node.id();
        let capacity = new_node.config().queue_capacity;
        self.add_node(new_node);

        if let Some(&index) = outgoing.first() {
            let old_edge = self.edges.remove(index);
            if let Some(targets) = self.adjacency.get_mut(&after_id) {
                if let Some(pos) = targets.iter().position(|&target| target == old_edge.to()) {
                    targets.remove(pos);
                }
            }
            let (splice_in, _rx1) = Edge::new(
                after_id,
                new_id,
                old_edge.edge_type(),
                old_edge.condition().cloned(),
                capacity,
            );
            let (splice_out, _rx2) =
                Edge::new(new_id, old_edge.to(), EdgeType::Sequential, None, capacity);
            self.connect(splice_in)?;
            self.connect(splice_out)?;
        } else {
            let (edge, _rx) = Edge::new(after_id, new_id, EdgeType::Sequential, None, capacity);
            self.connect(edge)?;
        }

        Ok(())
    }

    /// Remove a node, bridging its predecessors directly to its
    /// successors (see issue #37) — unlike [`Self::remove_node`] (#29),
    /// which just drops the dangling edges.
    ///
    /// Only supports the 1-to-many or many-to-1 case (typical for a
    /// linear pipeline splice); if the node has more than one predecessor
    /// *and* more than one successor, which pairs should connect is
    /// ambiguous, so this errors instead of guessing a full cross-product.
    /// New edges are plain `Sequential`, unconditioned — the removed
    /// node's own incident edges' types/conditions aren't carried over
    /// (there could be several, possibly conflicting).
    ///
    /// # Errors
    /// Returns [`GraphError::MissingNode`] if `id` doesn't exist, or
    /// [`GraphError::BuildFailed`] for the many-to-many case described
    /// above.
    pub fn remove_and_reconnect(&mut self, id: NodeId) -> Result<GraphNode, GraphError> {
        if !self.nodes.contains_key(&id) {
            return Err(GraphError::MissingNode {
                node_id: id.to_string(),
            });
        }

        let predecessors: Vec<NodeId> = self
            .edges
            .iter()
            .filter(|edge| edge.to() == id)
            .map(Edge::from)
            .collect();
        let successors: Vec<NodeId> = self
            .edges
            .iter()
            .filter(|edge| edge.from() == id)
            .map(Edge::to)
            .collect();

        if predecessors.len() > 1 && successors.len() > 1 {
            return Err(GraphError::BuildFailed {
                reason: format!(
                    "node {id} has {} predecessors and {} successors; \
                     remove_and_reconnect only supports 1-to-many or many-to-1",
                    predecessors.len(),
                    successors.len()
                ),
            });
        }

        let capacity = self
            .node(id)
            .map_or(crate::node::NodeConfig::DEFAULT_QUEUE_CAPACITY, |node| {
                node.config().queue_capacity
            });
        let removed = self
            .remove_node(id)
            .ok_or_else(|| GraphError::MissingNode {
                node_id: id.to_string(),
            })?;

        for &pred in &predecessors {
            for &succ in &successors {
                let (edge, _rx) = Edge::new(pred, succ, EdgeType::Sequential, None, capacity);
                self.connect(edge)?;
            }
        }

        Ok(removed)
    }

    /// Replace a node's module in place (see issue #38): same `NodeId`,
    /// same edges (nothing to rewire, since edges reference the `NodeId`
    /// not the module), new `Arc<dyn Node>`.
    ///
    /// The outgoing module's [`Module::migrate`](nyarix_module_api::Module::migrate)
    /// is called first, giving it a chance to flush/prepare state — but
    /// **its result doesn't change what happens next**: `migrate` (#16)
    /// takes no information about what it's migrating *to*, so there's no
    /// established way for it to actually hand internal state off to
    /// `new_module`. This always proceeds to install `new_module`
    /// regardless of whether `migrate` succeeded; the outcome is reported
    /// so the caller can log/react to it.
    ///
    /// **Not implemented here** (same M4 dependency as [`Self::insert_after`]):
    /// draining in-flight packets / atomicity with respect to a *running*
    /// graph. This only swaps the stored module.
    ///
    /// # Errors
    /// Returns [`GraphError::MissingNode`] if `id` doesn't exist.
    pub fn swap_node(
        &mut self,
        id: NodeId,
        new_module: std::sync::Arc<dyn nyarix_module_api::Node>,
        ctx: &nyarix_module_api::RuntimeContext,
    ) -> Result<SwapOutcome, GraphError> {
        let old_node = self
            .nodes
            .get_mut(&id)
            .ok_or_else(|| GraphError::MissingNode {
                node_id: id.to_string(),
            })?;

        let migrate_result = old_node.migrate(ctx);
        let config = old_node.config().clone();
        let new_node = GraphNode::new(id, new_module, config);
        let replaced = self
            .nodes
            .insert(id, new_node)
            .expect("checked above that the node exists");

        Ok(SwapOutcome {
            replaced,
            migrate_result,
        })
    }

    /// Structural difference between this graph and `other` (see issue
    /// #39): which nodes/edges were added or removed, and which nodes
    /// kept their id but got a different module instance (e.g. via
    /// [`Self::swap_node`]) — detected via `Arc` pointer identity, since
    /// `dyn Node` has no general equality to compare by.
    ///
    /// **Not implemented here**: building the "new" graph from a
    /// profile/stack (needs the Package/Profile system, M6/M9), migrating
    /// per-session state between old and new graphs (no defined model for
    /// that yet), and atomically switching a *running* graph over (M4).
    /// This is purely a diff of two [`FlowGraph`] snapshots you already
    /// have.
    #[must_use]
    pub fn diff(&self, other: &Self) -> GraphDiff {
        let old_ids: HashSet<NodeId> = self.nodes.keys().copied().collect();
        let new_ids: HashSet<NodeId> = other.nodes.keys().copied().collect();

        let added_nodes = new_ids.difference(&old_ids).copied().collect();
        let removed_nodes = old_ids.difference(&new_ids).copied().collect();
        let changed_nodes = old_ids
            .intersection(&new_ids)
            .filter(|id| {
                let old_module = self.nodes[id].module();
                let new_module = &other.nodes[id].module();
                !std::sync::Arc::ptr_eq(old_module, new_module)
            })
            .copied()
            .collect();

        let old_edges: HashSet<(NodeId, NodeId, EdgeType)> = self
            .edges
            .iter()
            .map(|edge| (edge.from(), edge.to(), edge.edge_type()))
            .collect();
        let new_edges: HashSet<(NodeId, NodeId, EdgeType)> = other
            .edges
            .iter()
            .map(|edge| (edge.from(), edge.to(), edge.edge_type()))
            .collect();

        let added_edges = new_edges.difference(&old_edges).copied().collect();
        let removed_edges = old_edges.difference(&new_edges).copied().collect();

        GraphDiff {
            added_nodes,
            removed_nodes,
            changed_nodes,
            added_edges,
            removed_edges,
        }
    }
}

/// Outcome of [`FlowGraph::swap_node`].
#[derive(Debug)]
pub struct SwapOutcome {
    /// The `GraphNode` that was replaced.
    pub replaced: GraphNode,
    /// Whether the outgoing module's `migrate()` succeeded — informational
    /// only; see [`FlowGraph::swap_node`]'s docs for why it doesn't change
    /// the outcome of the swap itself.
    pub migrate_result: nyarix_module_api::Result<()>,
}

/// Structural difference between two [`FlowGraph`]s, see
/// [`FlowGraph::diff`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphDiff {
    /// Nodes present in the new graph but not the old one.
    pub added_nodes: Vec<NodeId>,
    /// Nodes present in the old graph but not the new one.
    pub removed_nodes: Vec<NodeId>,
    /// Nodes present in both graphs but whose module instance differs.
    pub changed_nodes: Vec<NodeId>,
    /// Edges (as `(from, to, edge_type)`) present in the new graph but not
    /// the old one.
    pub added_edges: Vec<(NodeId, NodeId, EdgeType)>,
    /// Edges present in the old graph but not the new one.
    pub removed_edges: Vec<(NodeId, NodeId, EdgeType)>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DfsMark {
    Visiting,
    Visited,
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

    #[test]
    fn valid_linear_graph_passes_validation() {
        let mut graph = FlowGraph::new();
        let a = node("source", NodeType::Source);
        let b = node("sink", NodeType::Sink);
        let (a_id, b_id) = (a.id(), b.id());
        graph.add_node(a);
        graph.add_node(b);

        let (edge, _rx) = Edge::new(a_id, b_id, EdgeType::Sequential, None, 8);
        graph.connect(edge).unwrap();

        assert!(graph.detect_cycle().is_none());
        assert!(graph.validate().is_ok());
    }

    #[test]
    fn detects_a_cycle() {
        let mut graph = FlowGraph::new();
        let a = node("router", NodeType::Router);
        let b = node("fragmenter", NodeType::Transformer);
        let c = node("compressor", NodeType::Transformer);
        let (a_id, b_id, c_id) = (a.id(), b.id(), c.id());
        graph.add_node(a);
        graph.add_node(b);
        graph.add_node(c);

        let (ab, _rx1) = Edge::new(a_id, b_id, EdgeType::Sequential, None, 8);
        let (bc, _rx2) = Edge::new(b_id, c_id, EdgeType::Sequential, None, 8);
        let (ca, _rx3) = Edge::new(c_id, a_id, EdgeType::Sequential, None, 8);
        graph.connect(ab).unwrap();
        graph.connect(bc).unwrap();
        graph.connect(ca).unwrap();

        // DFS can start from any node (HashMap iteration order isn't
        // guaranteed), so the cycle may be reported starting at any of
        // its members — check by rotation instead of an exact string.
        let valid_descriptions = [
            "router → fragmenter → compressor → router",
            "fragmenter → compressor → router → fragmenter",
            "compressor → router → fragmenter → compressor",
        ];

        let cycle = graph.detect_cycle().expect("cycle should be detected");
        let description = graph.describe_cycle(&cycle);
        assert!(
            valid_descriptions.contains(&description.as_str()),
            "unexpected cycle description: {description}"
        );

        match graph.validate() {
            Err(GraphError::Cycle { cycle }) => assert_eq!(cycle, description),
            other => panic!("expected Cycle error, got {other:?}"),
        }
    }

    #[test]
    fn allowed_feedback_edge_is_not_a_cycle() {
        let mut graph = FlowGraph::new();
        let a = node("source", NodeType::Source);
        let b = node("loop-node", NodeType::Transformer);
        let c = node("sink", NodeType::Sink);
        let (a_id, b_id, c_id) = (a.id(), b.id(), c.id());
        graph.add_node(a);
        graph.add_node(b);
        graph.add_node(c);

        let (ab, _rx1) = Edge::new(a_id, b_id, EdgeType::Sequential, None, 8);
        let (bc, _rx2) = Edge::new(b_id, c_id, EdgeType::Sequential, None, 8);
        let (feedback, _rx3) = Edge::new(b_id, b_id, EdgeType::Fallback, None, 8);
        graph.connect(ab).unwrap();
        graph.connect(bc).unwrap();
        graph.connect(feedback).unwrap();

        // Without the allowlist, the self-loop is a cycle.
        assert!(graph.detect_cycle().is_some());

        graph.allow_feedback_edge(b_id, b_id);
        assert!(graph.detect_cycle().is_none());
        assert!(graph.validate().is_ok());
    }

    #[test]
    fn validate_rejects_unreachable_exit() {
        let mut graph = FlowGraph::new();
        let a = node("source", NodeType::Source);
        let b = node("sink", NodeType::Sink);
        graph.add_node(a);
        graph.add_node(b);
        // No edge connecting them: entry point can't reach the exit point.

        match graph.validate() {
            Err(GraphError::BuildFailed { .. }) => {}
            other => panic!("expected BuildFailed error, got {other:?}"),
        }
    }

    #[test]
    fn validate_rejects_orphan_node() {
        let mut graph = FlowGraph::new();
        let a = node("source", NodeType::Source);
        let b = node("sink", NodeType::Sink);
        let orphan = node("orphan", NodeType::Observer);
        let (a_id, b_id) = (a.id(), b.id());
        graph.add_node(a);
        graph.add_node(b);
        graph.add_node(orphan);

        let (edge, _rx) = Edge::new(a_id, b_id, EdgeType::Sequential, None, 8);
        graph.connect(edge).unwrap();

        match graph.validate() {
            Err(GraphError::BuildFailed { reason }) => assert!(reason.contains("orphan")),
            other => panic!("expected BuildFailed error, got {other:?}"),
        }
    }

    #[test]
    fn insert_after_splices_into_single_outgoing_edge() {
        let mut graph = FlowGraph::new();
        let a = node("source", NodeType::Source);
        let c = node("sink", NodeType::Sink);
        let (a_id, c_id) = (a.id(), c.id());
        graph.add_node(a);
        graph.add_node(c);
        let (edge, _rx) = Edge::new(a_id, c_id, EdgeType::Sequential, None, 8);
        graph.connect(edge).unwrap();

        let b = node("middle", NodeType::Transformer);
        let b_id = b.id();
        graph.insert_after(a_id, b).unwrap();

        assert_eq!(graph.edge_count(), 2);
        assert_eq!(graph.find_path(a_id, c_id), Some(vec![a_id, b_id, c_id]));
    }

    #[test]
    fn insert_after_with_no_outgoing_edge_just_connects() {
        let mut graph = FlowGraph::new();
        let a = node("source", NodeType::Source);
        let a_id = a.id();
        graph.add_node(a);

        let b = node("middle", NodeType::Transformer);
        let b_id = b.id();
        graph.insert_after(a_id, b).unwrap();

        assert_eq!(graph.find_path(a_id, b_id), Some(vec![a_id, b_id]));
    }

    #[test]
    fn insert_after_rejects_ambiguous_branch_point() {
        let mut graph = FlowGraph::new();
        let a = node("router", NodeType::Router);
        let b = node("branch-1", NodeType::Sink);
        let c = node("branch-2", NodeType::Sink);
        let (a_id, b_id, c_id) = (a.id(), b.id(), c.id());
        graph.add_node(a);
        graph.add_node(b);
        graph.add_node(c);
        let (e1, _rx1) = Edge::new(a_id, b_id, EdgeType::Sequential, None, 8);
        let (e2, _rx2) = Edge::new(a_id, c_id, EdgeType::Sequential, None, 8);
        graph.connect(e1).unwrap();
        graph.connect(e2).unwrap();

        let new_node = node("new", NodeType::Transformer);
        assert!(matches!(
            graph.insert_after(a_id, new_node),
            Err(GraphError::BuildFailed { .. })
        ));
    }

    #[test]
    fn remove_and_reconnect_bridges_predecessor_to_successor() {
        let mut graph = FlowGraph::new();
        let a = node("source", NodeType::Source);
        let b = node("middle", NodeType::Transformer);
        let c = node("sink", NodeType::Sink);
        let (a_id, b_id, c_id) = (a.id(), b.id(), c.id());
        graph.add_node(a);
        graph.add_node(b);
        graph.add_node(c);
        let (e1, _rx1) = Edge::new(a_id, b_id, EdgeType::Sequential, None, 8);
        let (e2, _rx2) = Edge::new(b_id, c_id, EdgeType::Sequential, None, 8);
        graph.connect(e1).unwrap();
        graph.connect(e2).unwrap();

        graph.remove_and_reconnect(b_id).unwrap();

        assert_eq!(graph.node_count(), 2);
        assert_eq!(graph.find_path(a_id, c_id), Some(vec![a_id, c_id]));
    }

    #[test]
    fn remove_and_reconnect_rejects_many_to_many() {
        let mut graph = FlowGraph::new();
        let a1 = node("source-1", NodeType::Source);
        let a2 = node("source-2", NodeType::Source);
        let hub = node("hub", NodeType::Aggregator);
        let b1 = node("sink-1", NodeType::Sink);
        let b2 = node("sink-2", NodeType::Sink);
        let (a1_id, a2_id, hub_id, b1_id, b2_id) = (a1.id(), a2.id(), hub.id(), b1.id(), b2.id());
        graph.add_node(a1);
        graph.add_node(a2);
        graph.add_node(hub);
        graph.add_node(b1);
        graph.add_node(b2);
        for (from, to) in [
            (a1_id, hub_id),
            (a2_id, hub_id),
            (hub_id, b1_id),
            (hub_id, b2_id),
        ] {
            let (edge, _rx) = Edge::new(from, to, EdgeType::Sequential, None, 8);
            graph.connect(edge).unwrap();
        }

        assert!(matches!(
            graph.remove_and_reconnect(hub_id),
            Err(GraphError::BuildFailed { .. })
        ));
    }

    #[test]
    fn swap_node_replaces_module_keeps_edges() {
        let mut graph = FlowGraph::new();
        let a = node("source", NodeType::Source);
        let b = node("old", NodeType::Transformer);
        let c = node("sink", NodeType::Sink);
        let (a_id, b_id, c_id) = (a.id(), b.id(), c.id());
        graph.add_node(a);
        graph.add_node(b);
        graph.add_node(c);
        let (e1, _rx1) = Edge::new(a_id, b_id, EdgeType::Sequential, None, 8);
        let (e2, _rx2) = Edge::new(b_id, c_id, EdgeType::Sequential, None, 8);
        graph.connect(e1).unwrap();
        graph.connect(e2).unwrap();

        let new_module: Arc<dyn Node> = Arc::new(StubNode::new("new", NodeType::Transformer));
        let ctx = RuntimeContext::empty();
        let outcome = graph.swap_node(b_id, new_module, &ctx).unwrap();

        assert!(outcome.migrate_result.is_ok());
        assert_eq!(outcome.replaced.module().metadata().name, "old");
        assert_eq!(graph.node(b_id).unwrap().module().metadata().name, "new");
        // Edges are untouched: the path still exists under the same id.
        assert_eq!(graph.find_path(a_id, c_id), Some(vec![a_id, b_id, c_id]));
    }

    #[test]
    fn swap_node_rejects_missing_node() {
        let mut graph = FlowGraph::new();
        let new_module: Arc<dyn Node> = Arc::new(StubNode::new("new", NodeType::Transformer));
        let ctx = RuntimeContext::empty();
        assert!(matches!(
            graph.swap_node(NodeId::new(), new_module, &ctx),
            Err(GraphError::MissingNode { .. })
        ));
    }

    #[test]
    fn diff_reports_added_removed_and_changed() {
        // Shared module instance for the node that must NOT be reported
        // as "changed" — `changed_nodes` is Arc pointer identity, so the
        // unchanged node must genuinely be the same Arc in both graphs,
        // not just an equivalent-looking new instance.
        let unchanged_source: Arc<dyn Node> = Arc::new(StubNode::new("source", NodeType::Source));
        let a_id = NodeId::new();

        let mut old_graph = FlowGraph::new();
        old_graph.add_node(GraphNode::new(
            a_id,
            Arc::clone(&unchanged_source),
            NodeConfig::default(),
        ));
        let b = node("stays", NodeType::Transformer);
        let removed_node = node("removed", NodeType::Sink);
        let (b_id, removed_id) = (b.id(), removed_node.id());
        old_graph.add_node(b);
        old_graph.add_node(removed_node);
        let (e1, _rx1) = Edge::new(a_id, b_id, EdgeType::Sequential, None, 8);
        let (e2, _rx2) = Edge::new(b_id, removed_id, EdgeType::Sequential, None, 8);
        old_graph.connect(e1).unwrap();
        old_graph.connect(e2).unwrap();

        let mut new_graph = FlowGraph::new();
        new_graph.add_node(GraphNode::new(
            a_id,
            Arc::clone(&unchanged_source),
            NodeConfig::default(),
        ));
        let b2 = node("stays-changed", NodeType::Transformer);
        let added_node = node("added", NodeType::Sink);
        new_graph.add_node(GraphNode::new(
            b_id,
            Arc::clone(b2.module()),
            b2.config().clone(),
        ));
        let added_id = added_node.id();
        new_graph.add_node(added_node);
        let (e3, _rx3) = Edge::new(a_id, b_id, EdgeType::Sequential, None, 8);
        let (e4, _rx4) = Edge::new(b_id, added_id, EdgeType::Sequential, None, 8);
        new_graph.connect(e3).unwrap();
        new_graph.connect(e4).unwrap();

        let diff = old_graph.diff(&new_graph);
        assert_eq!(diff.added_nodes, vec![added_id]);
        assert_eq!(diff.removed_nodes, vec![removed_id]);
        assert_eq!(diff.changed_nodes, vec![b_id]);
        assert!(
            diff.added_edges
                .contains(&(b_id, added_id, EdgeType::Sequential))
        );
        assert!(
            diff.removed_edges
                .contains(&(b_id, removed_id, EdgeType::Sequential))
        );
    }
}
