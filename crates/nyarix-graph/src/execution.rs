//! Sequential graph execution (see issue #32).
//!
//! Linear traversal only: a packet enters at one node, follows exactly
//! one outgoing edge per hop (the first whose [`Condition`](crate::Condition)
//! accepts it), and either reaches an exit point or is absorbed along the
//! way. Parallel fan-out (#33), async decoupling via the edge queues
//! (#34), and backpressure (#35) are separate, later pieces of the
//! execution engine — this is deliberately the simplest possible runner.
//!
//! **Scope note (#77 CPU limits):** `record_and_check_processing_time`
//! only *detects* a node overrunning its declared
//! [`nyarix_module_api::ResourceLimits::max_processing_time`] — after
//! `Module::process` already returned. #77 also asks for actually
//! *interrupting* a long-running call and a cooperative
//! `tokio::task::yield_now` model, neither of which fits
//! `Module::process`'s current signature: it's a plain synchronous
//! `fn`, not `async`, so it never yields and can't be cancelled
//! mid-call without running it on its own thread/process and abandoning
//! (not killing) that thread on timeout — the same per-module isolation
//! gap #76/#116 hit for memory. Tracked separately (#117) rather than
//! guessed at here.

use std::sync::Arc;
use std::time::{Duration, Instant};

use nyarix_core::NodeId;
use nyarix_error::{GraphError, ModuleError};
use nyarix_module_api::MetricRegistry;
use nyarix_packet::Packet;
use thiserror::Error;

use crate::graph::FlowGraph;
use crate::node::GraphNode;
use crate::throughput::ThroughputTracker;

/// Histogram bucket bounds for `process_duration_us` (#82), in
/// microseconds — a spread wide enough for both packet-processing
/// (microseconds to low milliseconds) and the occasional slow node
/// hitting its CPU budget (#77, up to whole seconds).
const PROCESS_DURATION_BUCKETS_US: [f64; 8] = [
    50.0,
    100.0,
    500.0,
    1_000.0,
    5_000.0,
    10_000.0,
    100_000.0,
    1_000_000.0,
];

/// Histogram bucket bounds for `packet_latency_us` (#81) — same range
/// as [`PROCESS_DURATION_BUCKETS_US`], but this measures the whole
/// entry-to-exit traversal, not one node's `process` call.
const PACKET_LATENCY_BUCKETS_US: [f64; 8] = PROCESS_DURATION_BUCKETS_US;

/// The scope #81's flow-level metrics (`packets_total`, `bytes_total`,
/// `packet_latency_us`, `packets_dropped`) are recorded under — these
/// describe the graph as a whole, not any one node/module, so there's
/// no module name to key them by the way #82's per-node metrics are.
const FLOW_METRICS_SCOPE: &str = "flow";

/// Record #81's "Сбор на входе графа": one packet entering, `len`
/// bytes.
fn record_flow_entry(metrics: Option<&MetricRegistry>, len: usize) {
    let Some(metrics) = metrics else {
        return;
    };
    metrics
        .counter(FLOW_METRICS_SCOPE, "packets_total")
        .increment(1);
    metrics
        .counter(FLOW_METRICS_SCOPE, "bytes_total")
        .increment(u64::try_from(len).unwrap_or(u64::MAX));
}

/// Record #81's "Сбор на выходе графа": either the packet reached an
/// exit point after `started` (recorded in `packet_latency_us`), or it
/// didn't — absorbed along the way (#19), or the traversal errored —
/// counted as `packets_dropped` either way, since from the flow's
/// perspective neither one produced an output packet.
fn record_flow_exit(metrics: Option<&MetricRegistry>, started: Instant, reached_exit: bool) {
    let Some(metrics) = metrics else {
        return;
    };
    if reached_exit {
        #[allow(clippy::cast_precision_loss)]
        let elapsed_us = started.elapsed().as_micros() as f64;
        metrics
            .histogram(
                FLOW_METRICS_SCOPE,
                "packet_latency_us",
                PACKET_LATENCY_BUCKETS_US.to_vec(),
            )
            .observe(elapsed_us);
    } else {
        metrics
            .counter(FLOW_METRICS_SCOPE, "packets_dropped")
            .increment(1);
    }
}

/// Error produced while executing a packet through the graph.
#[derive(Debug, Error)]
pub enum ExecutionError {
    /// A node's module failed to process the packet.
    #[error("module processing failed: {0}")]
    Module(#[from] ModuleError),
    /// A graph-structural problem (missing node, dead end, ...).
    #[error("graph error: {0}")]
    Graph(#[from] GraphError),
}

/// Check a packet's payload against `node`'s declared
/// [`nyarix_module_api::ResourceLimits::max_payload_bytes`] (#76's
/// "Лимит на размер payload в обработке") *before* handing it to
/// `Module::process` — an oversized payload never reaches the module
/// at all, rather than trusting the module to reject it itself.
///
/// No declared limit (`None`) means unbounded, same convention as every
/// other [`nyarix_module_api::ResourceLimits`] field.
fn check_payload_limit(node: &GraphNode, packet: &Packet) -> Result<(), ExecutionError> {
    let metadata = node.module().metadata();
    let Some(max) = metadata.resource_limits.max_payload_bytes else {
        return Ok(());
    };
    let size = u64::try_from(packet.len()).unwrap_or(u64::MAX);
    if size > max {
        return Err(ExecutionError::Module(ModuleError::QuotaExceeded {
            name: metadata.name.clone(),
            resource: "payload_size".to_string(),
        }));
    }
    Ok(())
}

/// Record how long a `Module::process` call actually took (#77's
/// "Статистика: сколько CPU потребил модуль" — logged via `tracing`
/// regardless of whether a [`MetricRegistry`] is attached, so this is
/// visible even without one), and reject the node's result if it
/// exceeded [`nyarix_module_api::ResourceLimits::max_processing_time`]
/// (#77's "CPU time budget на модуль").
///
/// This can only detect an overrun *after* `process` already returned —
/// see this module's own scope note on why actually cutting off a
/// still-running synchronous call isn't implemented.
fn record_and_check_processing_time(
    node: &GraphNode,
    elapsed: Duration,
) -> Result<(), ExecutionError> {
    let metadata = node.module().metadata();
    tracing::debug!(
        node = %metadata.name,
        elapsed_ms = elapsed.as_millis(),
        "node process() duration"
    );
    let Some(max) = metadata.resource_limits.max_processing_time else {
        return Ok(());
    };
    if elapsed > max {
        tracing::warn!(
            node = %metadata.name,
            elapsed_ms = elapsed.as_millis(),
            budget_ms = max.as_millis(),
            "node exceeded its CPU time budget"
        );
        return Err(ExecutionError::Module(ModuleError::QuotaExceeded {
            name: metadata.name.clone(),
            resource: "processing_time".to_string(),
        }));
    }
    Ok(())
}

/// Record #82's per-node metrics for one `Module::process` call — a
/// no-op if `metrics` is `None` (same "attached or not" convention as
/// [`nyarix_module_api::context::RuntimeContext`]'s own optional
/// [`MetricRegistry`]).
///
/// - `process_calls_total` (counter) — incremented once per call.
/// - `process_duration_us` (histogram) — `elapsed` in microseconds.
/// - `queue_depth` (gauge) — `node`'s current
///   [`nyarix_module_api::Node::input_queue_depth`]. **Caveat**: this
///   reports whatever the module itself returns from that method —
///   until `NodeQueue` (#36) is actually wired into execution (#97),
///   there's no live queue behind it to measure, so a module that
///   hasn't implemented it meaningfully just reports whatever constant
///   it chose (`0` in every stub/test module in this codebase so far).
/// - `errors_total` (counter) — incremented when `succeeded` is `false`.
///
/// "Retries" (`retries_total`, #82's remaining bullet) isn't recorded
/// here — nothing in this execution engine retries a failed
/// `process()` call; a failure is logged and the packet is dropped
/// (see [`execute_sequential`]/[`execute_parallel`]'s docs). There's no
/// retry count to observe until a retry mechanism exists.
fn record_node_metrics(
    metrics: Option<&MetricRegistry>,
    node: &GraphNode,
    elapsed: Duration,
    succeeded: bool,
) {
    let Some(metrics) = metrics else {
        return;
    };
    let name = &node.module().metadata().name;

    metrics.counter(name, "process_calls_total").increment(1);
    #[allow(clippy::cast_precision_loss)]
    metrics
        .histogram(
            name,
            "process_duration_us",
            PROCESS_DURATION_BUCKETS_US.to_vec(),
        )
        .observe(elapsed.as_micros() as f64);
    metrics
        .gauge(name, "queue_depth")
        .set(i64::try_from(node.module().input_queue_depth()).unwrap_or(i64::MAX));
    if !succeeded {
        metrics.counter(name, "errors_total").increment(1);
    }
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
///
/// `metrics`, if attached, records #82's per-node metrics for every
/// hop (see [`record_node_metrics`]) and #81's flow-level metrics once
/// for the whole traversal (see [`record_flow_entry`]/
/// [`record_flow_exit`]) — `packets_total`/`bytes_total` at entry,
/// `packet_latency_us`/`packets_dropped` at exit (a dropped packet is
/// one absorbed along the way, #19, or one whose traversal errored;
/// either way it never reached an exit point, which is the flow-level
/// distinction this metric cares about).
pub fn execute_sequential(
    graph: &mut FlowGraph,
    entry: NodeId,
    packet: Packet,
    metrics: Option<&MetricRegistry>,
    mut throughput: Option<&mut ThroughputTracker>,
) -> Result<Option<Packet>, ExecutionError> {
    if let Some(ref mut tracker) = throughput {
        let len = u64::try_from(packet.len()).unwrap_or(u64::MAX);
        tracker.record(packet.metadata().flow_id, len);
    }
    record_flow_entry(metrics, packet.len());
    let started = Instant::now();
    let result = execute_sequential_inner(graph, entry, packet, metrics);
    record_flow_exit(metrics, started, matches!(result, Ok(Some(_))));
    result
}

fn execute_sequential_inner(
    graph: &mut FlowGraph,
    entry: NodeId,
    mut packet: Packet,
    metrics: Option<&MetricRegistry>,
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

        check_payload_limit(node, &packet)?;
        let started = Instant::now();
        let outcome = node.process(packet);
        let elapsed = started.elapsed();
        record_node_metrics(metrics, node, elapsed, outcome.is_ok());
        record_and_check_processing_time(node, elapsed)?;
        packet = match outcome? {
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
    graph: &Arc<tokio::sync::Mutex<FlowGraph>>,
    current: NodeId,
    packet: Packet,
    metrics: Option<&MetricRegistry>,
) -> Result<WalkStep, ExecutionError> {
    let mut guard = graph.lock().await;
    let node = guard
        .node_mut(current)
        .ok_or_else(|| GraphError::MissingNode {
            node_id: current.to_string(),
        })?;

    check_payload_limit(node, &packet)?;
    let started = Instant::now();
    let outcome = node.process(packet);
    let elapsed = started.elapsed();
    record_node_metrics(metrics, node, elapsed, outcome.is_ok());
    record_and_check_processing_time(node, elapsed)?;
    let processed = match outcome? {
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
///
/// `metrics`, same as [`execute_sequential`], records #81's flow-level
/// metrics once for the whole call — `reached_exit` for
/// [`record_flow_exit`]'s purposes is "at least one branch produced a
/// packet"; fan-out means some branches can be absorbed while others
/// exit, but there's no per-branch "dropped" accounting here (that
/// would need [`nyarix_module_api::NodeType::Aggregator`] semantics
/// this function's own docs already say aren't decided yet).
pub async fn execute_parallel(
    graph: Arc<tokio::sync::Mutex<FlowGraph>>,
    entry: NodeId,
    packet: Packet,
    max_concurrent_branches: usize,
    metrics: Option<Arc<MetricRegistry>>,
) -> Result<Vec<Packet>, ExecutionError> {
    record_flow_entry(metrics.as_deref(), packet.len());
    let started = Instant::now();
    let result = execute_parallel_inner(
        graph,
        entry,
        packet,
        max_concurrent_branches,
        metrics.clone(),
    )
    .await;
    let reached_exit = matches!(&result, Ok(results) if !results.is_empty());
    record_flow_exit(metrics.as_deref(), started, reached_exit);
    result
}

async fn execute_parallel_inner(
    graph: Arc<tokio::sync::Mutex<FlowGraph>>,
    entry: NodeId,
    packet: Packet,
    max_concurrent_branches: usize,
    metrics: Option<Arc<MetricRegistry>>,
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

    let semaphore = Arc::new(tokio::sync::Semaphore::new(max_concurrent_branches.max(1)));
    run_branch(graph, entry, packet, semaphore, metrics).await
}

fn run_branch(
    graph: Arc<tokio::sync::Mutex<FlowGraph>>,
    mut current: NodeId,
    mut packet: Packet,
    semaphore: Arc<tokio::sync::Semaphore>,
    metrics: Option<Arc<MetricRegistry>>,
) -> std::pin::Pin<Box<dyn Future<Output = Result<Vec<Packet>, ExecutionError>> + Send>> {
    Box::pin(async move {
        loop {
            match walk_node(&graph, current, packet, metrics.as_deref()).await? {
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
                        let graph = Arc::clone(&graph);
                        let semaphore = Arc::clone(&semaphore);
                        let metrics = metrics.clone();
                        let branch_packet = next_packet.clone();
                        set.spawn(async move {
                            let _permit = Arc::clone(&semaphore)
                                .acquire_owned()
                                .await
                                .expect("semaphore is never closed");
                            run_branch(graph, target, branch_packet, semaphore, metrics).await
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
        sleep_millis: u64,
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
                sleep_millis: 0,
            }
        }

        fn absorbing(name: &str, node_type: NodeType) -> Self {
            Self {
                absorb: true,
                ..Self::new(name, node_type)
            }
        }

        fn with_max_payload_bytes(name: &str, node_type: NodeType, max: u64) -> Self {
            let metadata =
                ModuleMetadata::new(name, semver::Version::new(0, 1, 0), ModuleType::Flow)
                    .with_resource_limits(nyarix_module_api::ResourceLimits {
                        max_payload_bytes: Some(max),
                        ..nyarix_module_api::ResourceLimits::unbounded()
                    });
            Self {
                metadata,
                node_type,
                absorb: false,
                sleep_millis: 0,
            }
        }

        fn with_processing_budget(
            name: &str,
            node_type: NodeType,
            budget_ms: u64,
            sleep_millis: u64,
        ) -> Self {
            let metadata =
                ModuleMetadata::new(name, semver::Version::new(0, 1, 0), ModuleType::Flow)
                    .with_resource_limits(nyarix_module_api::ResourceLimits {
                        max_processing_time: Some(Duration::from_millis(budget_ms)),
                        ..nyarix_module_api::ResourceLimits::unbounded()
                    });
            Self {
                metadata,
                node_type,
                absorb: false,
                sleep_millis,
            }
        }
    }

    impl Module for StubNode {
        fn metadata(&self) -> &ModuleMetadata {
            &self.metadata
        }

        fn initialize(&mut self, _ctx: &RuntimeContext) -> Result<(), ModuleError> {
            Ok(())
        }

        fn process(&mut self, packet: Packet) -> Result<Option<Packet>, ModuleError> {
            if self.sleep_millis > 0 {
                std::thread::sleep(Duration::from_millis(self.sleep_millis));
            }
            if self.absorb {
                Ok(None)
            } else {
                Ok(Some(packet))
            }
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

        let result = execute_sequential(&mut graph, a_id, pkt, None, None).unwrap();
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

        let result = execute_sequential(
            &mut graph,
            a_id,
            Packet::new(b"data".as_slice()),
            None,
            None,
        )
        .unwrap();
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

        let result = execute_sequential(
            &mut graph,
            a_id,
            Packet::new(b"data".as_slice()),
            None,
            None,
        );
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

        let result = execute_sequential(
            &mut graph,
            a_id,
            Packet::new(b"data".as_slice()),
            None,
            None,
        );
        assert!(matches!(
            result,
            Err(ExecutionError::Graph(GraphError::MissingNode { .. }))
        ));
    }

    #[test]
    fn an_oversized_payload_is_rejected_before_reaching_the_module() {
        let mut graph = FlowGraph::new();
        let a = node(StubNode::with_max_payload_bytes(
            "limited",
            NodeType::Source,
            5,
        ));
        let a_id = a.id();
        graph.mark_exit_point(a_id);
        graph.add_node(a);

        let result = execute_sequential(
            &mut graph,
            a_id,
            Packet::new(b"way too big".as_slice()),
            None,
            None,
        );

        let Err(ExecutionError::Module(ModuleError::QuotaExceeded { name, resource })) = result
        else {
            panic!("expected QuotaExceeded");
        };
        assert_eq!(name, "limited");
        assert_eq!(resource, "payload_size");
    }

    #[test]
    fn a_payload_within_the_limit_is_processed_normally() {
        let mut graph = FlowGraph::new();
        let a = node(StubNode::with_max_payload_bytes(
            "limited",
            NodeType::Source,
            5,
        ));
        let a_id = a.id();
        graph.mark_exit_point(a_id);
        graph.add_node(a);

        let pkt = Packet::new(b"ok".as_slice());
        let id = pkt.id();
        let result = execute_sequential(&mut graph, a_id, pkt, None, None).unwrap();
        assert_eq!(result.unwrap().id(), id);
    }

    #[test]
    fn a_node_that_overruns_its_processing_budget_is_reported_as_quota_exceeded() {
        let mut graph = FlowGraph::new();
        let a = node(StubNode::with_processing_budget(
            "slow",
            NodeType::Source,
            1,
            50,
        ));
        let a_id = a.id();
        graph.mark_exit_point(a_id);
        graph.add_node(a);

        let result = execute_sequential(
            &mut graph,
            a_id,
            Packet::new(b"data".as_slice()),
            None,
            None,
        );

        let Err(ExecutionError::Module(ModuleError::QuotaExceeded { name, resource })) = result
        else {
            panic!("expected QuotaExceeded");
        };
        assert_eq!(name, "slow");
        assert_eq!(resource, "processing_time");
    }

    #[test]
    fn a_node_within_its_processing_budget_is_not_rejected() {
        let mut graph = FlowGraph::new();
        let a = node(StubNode::with_processing_budget(
            "fast",
            NodeType::Source,
            500,
            0,
        ));
        let a_id = a.id();
        graph.mark_exit_point(a_id);
        graph.add_node(a);

        let pkt = Packet::new(b"data".as_slice());
        let id = pkt.id();
        let result = execute_sequential(&mut graph, a_id, pkt, None, None).unwrap();
        assert_eq!(result.unwrap().id(), id);
    }

    #[test]
    fn per_node_metrics_are_recorded_when_a_registry_is_attached() {
        let mut graph = FlowGraph::new();
        let a = node(StubNode::new("source", NodeType::Source));
        let a_id = a.id();
        graph.mark_exit_point(a_id);
        graph.add_node(a);

        let metrics = MetricRegistry::new();
        let result = execute_sequential(
            &mut graph,
            a_id,
            Packet::new(b"data".as_slice()),
            Some(&metrics),
            None,
        )
        .unwrap();
        assert!(result.is_some());

        assert_eq!(metrics.counter("source", "process_calls_total").value(), 1);
        assert_eq!(metrics.counter("source", "errors_total").value(), 0);
        assert_eq!(metrics.gauge("source", "queue_depth").value(), 0);
        assert_eq!(
            metrics
                .histogram("source", "process_duration_us", vec![])
                .snapshot()
                .count,
            1
        );
    }

    #[test]
    fn per_node_metrics_count_errors_when_process_fails() {
        struct FailingNode {
            metadata: ModuleMetadata,
        }

        impl Module for FailingNode {
            fn metadata(&self) -> &ModuleMetadata {
                &self.metadata
            }

            fn initialize(&mut self, _ctx: &RuntimeContext) -> Result<(), ModuleError> {
                Ok(())
            }

            fn process(&mut self, _packet: Packet) -> Result<Option<Packet>, ModuleError> {
                Err(ModuleError::Crashed {
                    name: "failing".to_string(),
                    reason: "boom".to_string(),
                })
            }

            fn shutdown(&mut self, _ctx: &RuntimeContext) -> Result<(), ModuleError> {
                Ok(())
            }

            fn health(&self) -> Health {
                Health::Healthy
            }
        }

        impl Node for FailingNode {
            fn node_type(&self) -> NodeType {
                NodeType::Source
            }

            fn input_queue_depth(&self) -> usize {
                0
            }

            fn output_connections(&self) -> &[NodeId] {
                &[]
            }
        }

        let mut graph = FlowGraph::new();
        let module: Arc<dyn Node> = Arc::new(FailingNode {
            metadata: ModuleMetadata::new(
                "failing",
                semver::Version::new(0, 1, 0),
                ModuleType::Flow,
            ),
        });
        let a = GraphNode::new(NodeId::new(), module, NodeConfig::default());
        let a_id = a.id();
        graph.mark_exit_point(a_id);
        graph.add_node(a);

        let metrics = MetricRegistry::new();
        let result = execute_sequential(
            &mut graph,
            a_id,
            Packet::new(b"data".as_slice()),
            Some(&metrics),
            None,
        );
        assert!(result.is_err());

        assert_eq!(metrics.counter("failing", "process_calls_total").value(), 1);
        assert_eq!(metrics.counter("failing", "errors_total").value(), 1);
    }

    #[test]
    fn flow_metrics_count_a_successful_traversal() {
        let mut graph = FlowGraph::new();
        let a = node(StubNode::new("source", NodeType::Source));
        let a_id = a.id();
        graph.mark_exit_point(a_id);
        graph.add_node(a);

        let metrics = MetricRegistry::new();
        let pkt = Packet::new(b"hello".as_slice());
        let result = execute_sequential(&mut graph, a_id, pkt, Some(&metrics), None).unwrap();
        assert!(result.is_some());

        assert_eq!(metrics.counter("flow", "packets_total").value(), 1);
        assert_eq!(metrics.counter("flow", "bytes_total").value(), 5);
        assert_eq!(metrics.counter("flow", "packets_dropped").value(), 0);
        assert_eq!(
            metrics
                .histogram("flow", "packet_latency_us", vec![])
                .snapshot()
                .count,
            1
        );
    }

    #[test]
    fn flow_metrics_count_an_absorbed_packet_as_dropped() {
        let mut graph = FlowGraph::new();
        let a = node(StubNode::absorbing("filter", NodeType::Source));
        let a_id = a.id();
        graph.mark_exit_point(a_id);
        graph.add_node(a);

        let metrics = MetricRegistry::new();
        let result = execute_sequential(
            &mut graph,
            a_id,
            Packet::new(b"data".as_slice()),
            Some(&metrics),
            None,
        )
        .unwrap();
        assert!(result.is_none());

        assert_eq!(metrics.counter("flow", "packets_total").value(), 1);
        assert_eq!(metrics.counter("flow", "packets_dropped").value(), 1);
        assert_eq!(
            metrics
                .histogram("flow", "packet_latency_us", vec![])
                .snapshot()
                .count,
            0
        );
    }

    #[test]
    fn flow_metrics_count_an_errored_traversal_as_dropped() {
        let mut graph = FlowGraph::new();
        let a = node(StubNode::with_processing_budget(
            "slow",
            NodeType::Source,
            1,
            50,
        ));
        let a_id = a.id();
        graph.mark_exit_point(a_id);
        graph.add_node(a);

        let metrics = MetricRegistry::new();
        let result = execute_sequential(
            &mut graph,
            a_id,
            Packet::new(b"data".as_slice()),
            Some(&metrics),
            None,
        );
        assert!(result.is_err());

        assert_eq!(metrics.counter("flow", "packets_total").value(), 1);
        assert_eq!(metrics.counter("flow", "packets_dropped").value(), 1);
    }

    #[test]
    fn throughput_tracker_is_called_on_each_execution() {
        let mut graph = FlowGraph::new();
        let a = node(StubNode::new("source", NodeType::Source));
        let a_id = a.id();
        graph.mark_exit_point(a_id);
        graph.add_node(a);

        let mut tracker = ThroughputTracker::new();
        let pkt = Packet::new(b"hello".as_slice());
        let flow_id = pkt.metadata().flow_id;

        let result = execute_sequential(&mut graph, a_id, pkt, None, Some(&mut tracker)).unwrap();
        assert!(result.is_some());

        // The tracker should have recorded 5 bytes for this flow.
        assert_eq!(tracker.flow_throughput(flow_id) > 0.0, true);
    }

    fn shared(graph: FlowGraph) -> Arc<tokio::sync::Mutex<FlowGraph>> {
        Arc::new(tokio::sync::Mutex::new(graph))
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

        let results =
            execute_parallel(shared(graph), src, Packet::new(b"data".as_slice()), 4, None)
                .await
                .unwrap();
        assert_eq!(results.len(), 2);
    }

    #[tokio::test]
    async fn parallel_flow_metrics_count_one_entry_and_one_exit() {
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

        let metrics = Arc::new(MetricRegistry::new());
        let results = execute_parallel(
            shared(graph),
            src,
            Packet::new(b"data".as_slice()),
            4,
            Some(Arc::clone(&metrics)),
        )
        .await
        .unwrap();
        assert_eq!(results.len(), 2);

        assert_eq!(metrics.counter("flow", "packets_total").value(), 1);
        assert_eq!(metrics.counter("flow", "packets_dropped").value(), 0);
        assert_eq!(
            metrics
                .histogram("flow", "packet_latency_us", vec![])
                .snapshot()
                .count,
            1
        );
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

        let results =
            execute_parallel(shared(graph), src, Packet::new(b"data".as_slice()), 4, None)
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
        let results =
            execute_parallel(shared(graph), src, Packet::new(b"data".as_slice()), 0, None)
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

        let result = execute_parallel(
            shared(graph),
            a_id,
            Packet::new(b"data".as_slice()),
            4,
            None,
        )
        .await;
        assert!(matches!(
            result,
            Err(ExecutionError::Graph(GraphError::MissingNode { .. }))
        ));
    }
}
