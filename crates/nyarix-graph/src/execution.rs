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

/// Outcome of processing one packet through one node, before deciding
/// where (or whether) it goes next.
enum WalkStep {
    /// The node absorbed the packet (see #19).
    Absorbed,
    /// The packet reached an exit point.
    Exited(Packet),
    /// The packet should continue to one of these `(EdgeType, NodeId)`
    /// targets, all of which accepted it.
    Continue(Packet, Vec<(crate::edge::EdgeType, NodeId)>),
}

async fn walk_node(
    graph: &std::sync::Arc<tokio::sync::Mutex<FlowGraph>>,
    current: NodeId,
    packet: Packet,
) -> Result<WalkStep, ExecutionError> {
    let mut guard = graph.lock().await;
    let node = guard
        .node_mut(current)
        .ok_or_else(|| GraphError::MissingNode {
            node_id: current.to_string(),
        })?;

    let processed = match node.process(packet)? {
        Some(packet) => packet,
        None => return Ok(WalkStep::Absorbed),
    };

    if guard.exit_points().contains(&current) {
        return Ok(WalkStep::Exited(processed));
    }

    let next: Vec<_> = guard
        .edges_from(current)
        .filter(|edge| edge.accepts(&processed))
        .map(|edge| (edge.edge_type(), edge.to()))
        .collect();
    Ok(WalkStep::Continue(processed, next))
}

/// Run a packet through the graph starting at `entry`, forking into
/// concurrent `tokio::spawn`ed branches wherever a node has one or more
/// accepting [`EdgeType::Parallel`](crate::edge::EdgeType::Parallel)
/// edges (see issue #33), and joining all branches before returning.
///
/// Returns one packet per branch that reached an exit point — branches
/// absorbed along the way (#19) contribute nothing. **This deliberately
/// does not merge branches into a single packet**: what "merging N
/// parallel results back into one" means is the job of an
/// [`nyarix_module_api::NodeType::Aggregator`] node, and that node type
/// has no defined arity/waiting policy yet (does it wait for all
/// predecessors? first N? with a timeout?) — tracked separately once
/// that's designed.
///
/// If a node has both `Parallel` and non-`Parallel` accepting edges at
/// the same time, the non-`Parallel` ones are ignored for that hop:
/// `Parallel` is treated as an explicit fan-out point, not one option
/// among several to pick from.
///
/// `max_concurrent_branches` bounds how many spawned branches run at
/// once (via a [`tokio::sync::Semaphore`]); it's clamped to at least 1.
///
/// # Errors
/// Same conditions as [`execute_sequential`], plus: if a spawned branch
/// task panics, that's reported as [`GraphError::BuildFailed`].
pub async fn execute_parallel(
    graph: std::sync::Arc<tokio::sync::Mutex<FlowGraph>>,
    entry: NodeId,
    packet: Packet,
    max_concurrent_branches: usize,
) -> Result<Vec<Packet>, ExecutionError> {
    {
        let guard = graph.lock().await;
        if !guard.entry_points().contains(&entry) {
            return Err(GraphError::MissingNode {
                node_id: entry.to_string(),
            }
            .into());
        }
    }

    let semaphore =
        std::sync::Arc::new(tokio::sync::Semaphore::new(max_concurrent_branches.max(1)));
    run_branch(graph, entry, packet, semaphore).await
}

fn run_branch(
    graph: std::sync::Arc<tokio::sync::Mutex<FlowGraph>>,
    mut current: NodeId,
    mut packet: Packet,
    semaphore: std::sync::Arc<tokio::sync::Semaphore>,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<Packet>, ExecutionError>> + Send>>
{
    Box::pin(async move {
        loop {
            match walk_node(&graph, current, packet).await? {
                WalkStep::Absorbed => return Ok(Vec::new()),
                WalkStep::Exited(packet) => return Ok(vec![packet]),
                WalkStep::Continue(next_packet, edges) => {
                    let parallel_targets: Vec<NodeId> = edges
                        .iter()
                        .filter(|(edge_type, _)| *edge_type == crate::edge::EdgeType::Parallel)
                        .map(|(_, to)| *to)
                        .collect();

                    if parallel_targets.is_empty() {
                        let Some((_, to)) = edges.first() else {
                            return Err(GraphError::BuildFailed {
                                reason: format!(
                                    "node {current} has no outgoing edge accepting this \
                                     packet (and isn't an exit point) — run \
                                     FlowGraph::validate() first"
                                ),
                            }
                            .into());
                        };
                        current = *to;
                        packet = next_packet;
                        continue;
                    }

                    let mut set = tokio::task::JoinSet::new();
                    for target in parallel_targets {
                        let graph = std::sync::Arc::clone(&graph);
                        let semaphore = std::sync::Arc::clone(&semaphore);
                        let branch_packet = next_packet.clone();
                        set.spawn(async move {
                            let _permit = std::sync::Arc::clone(&semaphore)
                                .acquire_owned()
                                .await
                                .expect("semaphore is never closed");
                            run_branch(graph, target, branch_packet, semaphore).await
                        });
                    }

                    let mut results = Vec::new();
                    while let Some(joined) = set.join_next().await {
                        let branch_result: Result<Vec<Packet>, ExecutionError> =
                            joined.map_err(|join_err| GraphError::BuildFailed {
                                reason: format!("parallel branch task panicked: {join_err}"),
                            })?;
                        results.extend(branch_result?);
                    }
                    return Ok(results);
                }
            }
        }
    })
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

    fn shared(graph: FlowGraph) -> std::sync::Arc<tokio::sync::Mutex<FlowGraph>> {
        std::sync::Arc::new(tokio::sync::Mutex::new(graph))
    }

    #[tokio::test]
    async fn parallel_fanout_joins_both_branches() {
        let mut graph = FlowGraph::new();
        let source = node(StubNode::new("source", NodeType::Source));
        let branch_a = node(StubNode::new("branch-a", NodeType::Transformer));
        let branch_b = node(StubNode::new("branch-b", NodeType::Transformer));
        let sink_a = node(StubNode::new("sink-a", NodeType::Sink));
        let sink_b = node(StubNode::new("sink-b", NodeType::Sink));
        let (src, ba, bb, sa, sb) = (
            source.id(),
            branch_a.id(),
            branch_b.id(),
            sink_a.id(),
            sink_b.id(),
        );
        graph.add_node(source);
        graph.add_node(branch_a);
        graph.add_node(branch_b);
        graph.add_node(sink_a);
        graph.add_node(sink_b);

        let (e1, _rx1) = crate::edge::Edge::new(src, ba, EdgeType::Parallel, None, 8);
        let (e2, _rx2) = crate::edge::Edge::new(src, bb, EdgeType::Parallel, None, 8);
        let (e3, _rx3) = crate::edge::Edge::new(ba, sa, EdgeType::Sequential, None, 8);
        let (e4, _rx4) = crate::edge::Edge::new(bb, sb, EdgeType::Sequential, None, 8);
        graph.connect(e1).unwrap();
        graph.connect(e2).unwrap();
        graph.connect(e3).unwrap();
        graph.connect(e4).unwrap();

        let results = execute_parallel(shared(graph), src, Packet::new(b"data".as_slice()), 4)
            .await
            .unwrap();
        assert_eq!(results.len(), 2);
    }

    #[tokio::test]
    async fn absorbed_branch_does_not_affect_sibling() {
        let mut graph = FlowGraph::new();
        let source = node(StubNode::new("source", NodeType::Source));
        let dropper = node(StubNode::absorbing("dropper", NodeType::Filter));
        let passer = node(StubNode::new("passer", NodeType::Transformer));
        let sink = node(StubNode::new("sink", NodeType::Sink));
        let (src, drop_id, pass_id, sink_id) = (source.id(), dropper.id(), passer.id(), sink.id());
        graph.add_node(source);
        graph.add_node(dropper);
        graph.add_node(passer);
        graph.add_node(sink);

        let (e1, _rx1) = crate::edge::Edge::new(src, drop_id, EdgeType::Parallel, None, 8);
        let (e2, _rx2) = crate::edge::Edge::new(src, pass_id, EdgeType::Parallel, None, 8);
        let (e3, _rx3) = crate::edge::Edge::new(pass_id, sink_id, EdgeType::Sequential, None, 8);
        graph.connect(e1).unwrap();
        graph.connect(e2).unwrap();
        graph.connect(e3).unwrap();

        let results = execute_parallel(shared(graph), src, Packet::new(b"data".as_slice()), 4)
            .await
            .unwrap();
        // Only the surviving branch contributes a result.
        assert_eq!(results.len(), 1);
    }

    #[tokio::test]
    async fn zero_max_concurrency_is_clamped_to_one() {
        let mut graph = FlowGraph::new();
        let source = node(StubNode::new("source", NodeType::Source));
        let branch_a = node(StubNode::new("branch-a", NodeType::Sink));
        let branch_b = node(StubNode::new("branch-b", NodeType::Sink));
        let (src, ba, bb) = (source.id(), branch_a.id(), branch_b.id());
        graph.add_node(source);
        graph.add_node(branch_a);
        graph.add_node(branch_b);

        let (e1, _rx1) = crate::edge::Edge::new(src, ba, EdgeType::Parallel, None, 8);
        let (e2, _rx2) = crate::edge::Edge::new(src, bb, EdgeType::Parallel, None, 8);
        graph.connect(e1).unwrap();
        graph.connect(e2).unwrap();

        // max_concurrent_branches = 0 must not deadlock; it's clamped to 1.
        let results = execute_parallel(shared(graph), src, Packet::new(b"data".as_slice()), 0)
            .await
            .unwrap();
        assert_eq!(results.len(), 2);
    }

    #[tokio::test]
    async fn parallel_rejects_non_entry_point() {
        let mut graph = FlowGraph::new();
        let a = node(StubNode::new("transformer", NodeType::Transformer));
        let a_id = a.id();
        graph.add_node(a);

        let result =
            execute_parallel(shared(graph), a_id, Packet::new(b"data".as_slice()), 4).await;
        assert!(matches!(
            result,
            Err(ExecutionError::Graph(GraphError::MissingNode { .. }))
        ));
    }
}
