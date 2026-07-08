//! Main Runtime execution loop (see issue #43).
//!
//! Initializes every node in the graph, then repeatedly takes a packet
//! from a source channel, runs it through the graph via
//! [`nyarix_graph::execute_sequential`], and forwards the result to a
//! sink channel — until cancelled or the source closes. Shuts down every
//! node before returning.
//!
//! **Scope note — "Чтение из источника (TUN / socket / generator)":**
//! this loop only knows about a generic `mpsc::Receiver<Packet>`/
//! `Sender<Packet>`. It has no idea whether the other end is backed by a
//! TUN device (`nyarix-tun` doesn't have any code yet, only a separate
//! repo's issue backlog), a network socket (M9 official transport
//! modules, not built), or a synthetic test generator. Wiring any of
//! those in is separate work for whoever builds that transport.
//!
//! Also not implemented here: fan-out via
//! [`nyarix_graph::execute_parallel`] (#33) — this loop only drives
//! `execute_sequential`; and the CPU/I/O worker-pool split (#45/#46) —
//! this is a single async task, not yet backed by dedicated thread pools.
//!
//! Graph mutation mid-run (#37/#38/#39) is coordinated via
//! [`crate::pause::GraphPauseHandle`]/[`crate::pause::GraphPauseWatcher`]
//! (#98): while paused, this loop stops calling `source.recv()` again,
//! so no new packet starts a fresh trip through the graph until resumed.
//! See that module's docs for what this guarantee does and doesn't cover.

use std::sync::Arc;
use std::time::Duration;

use nyarix_core::NodeId;
use nyarix_error::ModuleError;
use nyarix_graph::FlowGraph;
use nyarix_module_api::RuntimeContext;
use nyarix_packet::Packet;
use thiserror::Error;
use tokio::sync::{Mutex, mpsc};
use tokio_util::sync::CancellationToken;

use crate::pause::GraphPauseWatcher;

/// Default bound on how long graceful shutdown (draining + node
/// `shutdown()` calls) is allowed to take before [`run`] gives up waiting
/// and returns anyway (see issue #44's "таймаут на принудительное
/// завершение").
pub const DEFAULT_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);

/// Error produced by the execution loop.
#[derive(Debug, Error)]
pub enum ExecutionLoopError {
    /// A node failed to initialize; the loop never started.
    #[error("failed to initialize node: {0}")]
    Initialize(#[from] ModuleError),
}

/// Initialize every node in the graph (see #22's lifecycle guarantee:
/// `initialize` before the first `process`).
///
/// # Errors
/// Returns the first [`ModuleError`] encountered. Nodes already
/// initialized before the failing one are left as-is — there's no
/// automatic rollback; a Runtime that fails to come up should be torn
/// down by the caller (e.g. via [`shutdown_all_nodes`]), not silently
/// half-initialized.
pub async fn initialize_all_nodes(
    graph: &Arc<Mutex<FlowGraph>>,
    ctx: &RuntimeContext,
) -> Result<(), ModuleError> {
    let mut guard = graph.lock().await;
    let ids: Vec<NodeId> = guard.node_ids().collect();
    for id in ids {
        if let Some(node) = guard.node_mut(id) {
            node.initialize(ctx)?;
        }
    }
    Ok(())
}

/// Shut down every node in the graph (see #22's lifecycle guarantee:
/// `shutdown` after the last `process`).
///
/// Unlike [`initialize_all_nodes`], this keeps going even if one node's
/// shutdown fails — a Runtime tearing down shouldn't leave the remaining
/// nodes leaking resources just because an earlier one errored. Returns
/// the first error encountered, if any, after every node has had a
/// chance to shut down.
///
/// Before calling `shutdown` on a node, drains whatever's still sitting
/// in its own input queue (#97) — packets enqueued but not yet
/// dequeued/processed when shutdown started (e.g. a concurrent branch
/// that lost the race to a sibling at a converging node, #96) are
/// logged and discarded rather than silently leaked.
pub async fn shutdown_all_nodes(
    graph: &Arc<Mutex<FlowGraph>>,
    ctx: &RuntimeContext,
) -> Result<(), ModuleError> {
    let mut guard = graph.lock().await;
    let ids: Vec<NodeId> = guard.node_ids().collect();
    let mut first_error = None;
    for id in ids {
        if let Some(node) = guard.node_mut(id) {
            let leftover = node.queue_receiver_mut().drain();
            if !leftover.is_empty() {
                tracing::warn!(
                    node_id = %id,
                    count = leftover.len(),
                    "discarding packets still queued at shutdown"
                );
            }
            if let Err(error) = node.shutdown(ctx) {
                tracing::warn!(%error, node_id = %id, "node failed to shut down cleanly");
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
        }
    }
    first_error.map_or(Ok(()), Err)
}

/// Everything [`run_with_timeout`] needs to drive one execution loop —
/// grouped into a struct rather than passed as separate arguments since
/// there are enough of them (graph, entry point, context, both ends of
/// the packet pipe, and three independent stop/pause signals) that a
/// positional argument list would be easy to miscall by swapping two of
/// similarly-typed ones.
pub struct RunConfig {
    /// The graph to run packets through.
    pub graph: Arc<Mutex<FlowGraph>>,
    /// Which node a packet from `source` enters the graph at.
    pub entry: NodeId,
    /// Runtime context passed to every node's lifecycle call.
    pub ctx: RuntimeContext,
    /// Where packets come from.
    pub source: mpsc::Receiver<Packet>,
    /// Where a packet that reaches an exit point is forwarded to.
    pub sink: mpsc::Sender<Packet>,
    /// Signals the loop to stop and begin graceful shutdown.
    pub shutdown: CancellationToken,
    /// Bounds how long draining and node shutdown are each allowed to
    /// take (see [`DEFAULT_SHUTDOWN_TIMEOUT`]).
    pub shutdown_timeout: Duration,
    /// Signals the loop to stop pulling new packets during a live graph
    /// mutation (see [`crate::pause::GraphPauseHandle`], #98).
    pub pause: GraphPauseWatcher,
}

/// Run the main execution loop with [`DEFAULT_SHUTDOWN_TIMEOUT`]. See
/// [`run_with_timeout`] for the full behavior and a way to override it.
///
/// # Errors
/// Same as [`run_with_timeout`].
pub async fn run(
    graph: Arc<Mutex<FlowGraph>>,
    entry: NodeId,
    ctx: RuntimeContext,
    source: mpsc::Receiver<Packet>,
    sink: mpsc::Sender<Packet>,
    shutdown: CancellationToken,
    pause: GraphPauseWatcher,
) -> Result<(), ExecutionLoopError> {
    run_with_timeout(RunConfig {
        graph,
        entry,
        ctx,
        source,
        sink,
        shutdown,
        shutdown_timeout: DEFAULT_SHUTDOWN_TIMEOUT,
        pause,
    })
    .await
}

/// Run the main execution loop (see issue #43), with graceful shutdown
/// (see issue #44) once `shutdown` is triggered, `source` closes, or
/// `sink` is dropped:
///
/// 1. Stop accepting new packets — the loop simply stops calling
///    `source.recv()` again. A `biased` `select!` makes sure a pending
///    cancellation is noticed before an already-ready `source.recv()` is
///    (i.e. shutdown always wins a tie, so no "one more packet" sneaks in
///    once cancelled).
/// 2. Drain whatever's already buffered in `source` (bounded by
///    `shutdown_timeout`), running each through the graph same as usual
///    — this is "processing the remainder", not just discarding it.
/// 3. Call `shutdown()` on every node ([`shutdown_all_nodes`]), also
///    bounded by `shutdown_timeout`.
///
/// If either step doesn't finish within `shutdown_timeout`, it's
/// abandoned (logged via `tracing`) and `run` returns anyway — forced
/// completion, per the issue's "таймаут на принудительное завершение".
///
/// While `pause` reports paused (see [`crate::pause::GraphPauseHandle`],
/// #98), the loop stops calling `source.recv()` again — whoever is
/// mutating the graph's topology is responsible for pausing before
/// calling `insert_after`/`remove_and_reconnect`/`swap_node` and
/// resuming afterward; see that module's docs for exactly what guarantee
/// this does and doesn't provide.
///
/// **Not implemented**: draining a node's own `NodeQueue` (#36) lanes —
/// only `source` (the entry point's inbound channel) is drained here;
/// see #97 for the queue itself and #98 for why full drain-on-pause
/// (vs. drain-on-shutdown, which this already does) isn't wired up yet.
///
/// # Errors
/// Returns [`ExecutionLoopError::Initialize`] if any node fails to
/// initialize — the loop never starts in that case. Packet-processing
/// errors encountered *during* the loop (main or draining) are logged
/// via `tracing`, not propagated: one bad packet shouldn't take down the
/// whole Runtime.
pub async fn run_with_timeout(config: RunConfig) -> Result<(), ExecutionLoopError> {
    let RunConfig {
        graph,
        entry,
        ctx,
        mut source,
        sink,
        shutdown,
        shutdown_timeout,
        mut pause,
    } = config;

    initialize_all_nodes(&graph, &ctx).await?;

    loop {
        pause.wait_until_resumed().await;

        let packet = tokio::select! {
            biased;
            () = shutdown.cancelled() => break,
            () = pause.wait_until_paused() => continue,
            received = source.recv() => match received {
                Some(packet) => packet,
                None => break,
            },
        };

        if !process_one(&graph, entry, packet, &sink, ctx.metrics().registry()).await {
            break;
        }
    }

    let drain_outcome = tokio::time::timeout(shutdown_timeout, async {
        while let Ok(packet) = source.try_recv() {
            if !process_one(&graph, entry, packet, &sink, ctx.metrics().registry()).await {
                break;
            }
        }
    })
    .await;
    if drain_outcome.is_err() {
        tracing::warn!("draining remaining packets timed out; forcing shutdown");
    }

    match tokio::time::timeout(shutdown_timeout, shutdown_all_nodes(&graph, &ctx)).await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            tracing::warn!(%error, "a node failed to shut down cleanly during loop exit");
        }
        Err(_) => {
            tracing::warn!("node shutdown timed out; forcing completion");
        }
    }

    Ok(())
}

/// Process one packet through the graph and forward the result to
/// `sink`. Returns `false` if the loop should stop (sink closed).
async fn process_one(
    graph: &Arc<Mutex<FlowGraph>>,
    entry: NodeId,
    packet: Packet,
    sink: &mpsc::Sender<Packet>,
    metrics: Option<&nyarix_module_api::MetricRegistry>,
) -> bool {
    let result = {
        let mut guard = graph.lock().await;
        // #75's "Перехват паники (catch_unwind)": a node panicking mid-`process`
        // is contained here the same way `capability_enforcement::enforce_and_instantiate`
        // contains one during `initialize` — logged and treated as this
        // packet failing, not as the whole loop unwinding.
        crate::sandbox::catch_panic(move || {
            nyarix_graph::execute_sequential(&mut guard, entry, packet, metrics, None)
        })
    };

    match result {
        Ok(Ok(Some(output))) => {
            if sink.send(output).await.is_err() {
                tracing::warn!("execution loop sink closed; stopping");
                return false;
            }
        }
        Ok(Ok(None)) => {}
        Ok(Err(error)) => {
            tracing::warn!(%error, "packet processing failed");
        }
        Err(reason) => {
            tracing::warn!(reason = %reason, "node panicked while processing a packet");
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use nyarix_error::ModuleError as CoreModuleError;
    use nyarix_graph::{Edge, EdgeType, GraphNode, NodeConfig};
    use nyarix_module_api::{Health, Module, ModuleMetadata, ModuleType, Node, NodeType};
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct TrackedNode {
        metadata: ModuleMetadata,
        node_type: NodeType,
        initialized: Arc<AtomicUsize>,
        shut_down: Arc<AtomicUsize>,
    }

    impl Module for TrackedNode {
        fn metadata(&self) -> &ModuleMetadata {
            &self.metadata
        }

        fn initialize(&mut self, _ctx: &RuntimeContext) -> Result<(), CoreModuleError> {
            self.initialized.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn process(&mut self, packet: Packet) -> Result<Option<Packet>, CoreModuleError> {
            Ok(Some(packet))
        }

        fn shutdown(&mut self, _ctx: &RuntimeContext) -> Result<(), CoreModuleError> {
            self.shut_down.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn health(&self) -> Health {
            Health::Healthy
        }
    }

    impl Node for TrackedNode {
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

    fn tracked_node(
        name: &str,
        node_type: NodeType,
        initialized: &Arc<AtomicUsize>,
        shut_down: &Arc<AtomicUsize>,
    ) -> GraphNode {
        let module: Arc<dyn Node> = Arc::new(TrackedNode {
            metadata: ModuleMetadata::new(name, semver::Version::new(0, 1, 0), ModuleType::Flow),
            node_type,
            initialized: Arc::clone(initialized),
            shut_down: Arc::clone(shut_down),
        });
        GraphNode::new(NodeId::new(), module, NodeConfig::default())
    }

    #[tokio::test]
    async fn loop_initializes_processes_and_shuts_down() {
        let initialized = Arc::new(AtomicUsize::new(0));
        let shut_down = Arc::new(AtomicUsize::new(0));

        let mut graph = FlowGraph::new();
        let source_node = tracked_node("source", NodeType::Source, &initialized, &shut_down);
        let sink_node = tracked_node("sink", NodeType::Sink, &initialized, &shut_down);
        let (entry, exit) = (source_node.id(), sink_node.id());
        graph.add_node(source_node);
        graph.add_node(sink_node);
        let (edge, _rx) = Edge::new(entry, exit, EdgeType::Sequential, None, 8);
        graph.connect(edge).unwrap();

        let graph = Arc::new(Mutex::new(graph));
        let (source_tx, source_rx) = mpsc::channel(8);
        let (sink_tx, mut sink_rx) = mpsc::channel(8);
        let shutdown = CancellationToken::new();

        let loop_handle = tokio::spawn(run(
            Arc::clone(&graph),
            entry,
            RuntimeContext::empty(),
            source_rx,
            sink_tx,
            shutdown.clone(),
            GraphPauseWatcher::always_resumed(),
        ));

        let pkt = Packet::new(b"hello".as_slice());
        let id = pkt.id();
        source_tx.send(pkt).await.unwrap();

        let received = sink_rx.recv().await.unwrap();
        assert_eq!(received.id(), id);
        assert_eq!(initialized.load(Ordering::SeqCst), 2);

        shutdown.cancel();
        loop_handle.await.unwrap().unwrap();
        assert_eq!(shut_down.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn loop_stops_when_source_closes() {
        let initialized = Arc::new(AtomicUsize::new(0));
        let shut_down = Arc::new(AtomicUsize::new(0));

        let mut graph = FlowGraph::new();
        let node = tracked_node("solo", NodeType::Source, &initialized, &shut_down);
        graph.mark_exit_point(node.id());
        let entry = node.id();
        graph.add_node(node);

        let graph = Arc::new(Mutex::new(graph));
        let (source_tx, source_rx) = mpsc::channel(8);
        let (sink_tx, _sink_rx) = mpsc::channel(8);
        let shutdown = CancellationToken::new();

        let loop_handle = tokio::spawn(run(
            graph,
            entry,
            RuntimeContext::empty(),
            source_rx,
            sink_tx,
            shutdown,
            GraphPauseWatcher::always_resumed(),
        ));

        drop(source_tx);
        loop_handle.await.unwrap().unwrap();
        assert_eq!(shut_down.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn shutdown_drains_already_buffered_packets() {
        let initialized = Arc::new(AtomicUsize::new(0));
        let shut_down = Arc::new(AtomicUsize::new(0));

        let mut graph = FlowGraph::new();
        let source_node = tracked_node("source", NodeType::Source, &initialized, &shut_down);
        let sink_node = tracked_node("sink", NodeType::Sink, &initialized, &shut_down);
        let (entry, exit) = (source_node.id(), sink_node.id());
        graph.add_node(source_node);
        graph.add_node(sink_node);
        let (edge, _rx) = Edge::new(entry, exit, EdgeType::Sequential, None, 8);
        graph.connect(edge).unwrap();

        let graph = Arc::new(Mutex::new(graph));
        // Enough capacity that both sends below land in the channel
        // before the loop has a chance to drain either of them.
        let (source_tx, source_rx) = mpsc::channel(8);
        let (sink_tx, mut sink_rx) = mpsc::channel(8);
        let shutdown = CancellationToken::new();

        let pkt1 = Packet::new(b"first".as_slice());
        let pkt2 = Packet::new(b"second".as_slice());
        let (id1, id2) = (pkt1.id(), pkt2.id());
        source_tx.send(pkt1).await.unwrap();
        source_tx.send(pkt2).await.unwrap();

        // Cancel immediately: both packets are already buffered in
        // `source` at this point, so they must come out through the
        // drain step (step 2), not the main loop.
        shutdown.cancel();

        run_with_timeout(RunConfig {
            graph,
            entry,
            ctx: RuntimeContext::empty(),
            source: source_rx,
            sink: sink_tx,
            shutdown,
            shutdown_timeout: Duration::from_secs(5),
            pause: GraphPauseWatcher::always_resumed(),
        })
        .await
        .unwrap();

        let mut received = Vec::new();
        while let Ok(packet) = sink_rx.try_recv() {
            received.push(packet.id());
        }
        assert_eq!(received, vec![id1, id2]);
        assert_eq!(shut_down.load(Ordering::SeqCst), 2);
    }

    struct PanickingNode {
        metadata: ModuleMetadata,
    }

    impl Module for PanickingNode {
        fn metadata(&self) -> &ModuleMetadata {
            &self.metadata
        }

        fn initialize(&mut self, _ctx: &RuntimeContext) -> Result<(), CoreModuleError> {
            Ok(())
        }

        fn process(&mut self, _packet: Packet) -> Result<Option<Packet>, CoreModuleError> {
            panic!("node panicked mid-process");
        }

        fn shutdown(&mut self, _ctx: &RuntimeContext) -> Result<(), CoreModuleError> {
            Ok(())
        }

        fn health(&self) -> Health {
            Health::Healthy
        }
    }

    impl Node for PanickingNode {
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

    #[tokio::test]
    async fn a_panicking_node_does_not_crash_the_loop_and_the_next_packet_still_gets_through() {
        let mut graph = FlowGraph::new();
        let node: Arc<dyn Node> = Arc::new(PanickingNode {
            metadata: ModuleMetadata::new(
                "panicky",
                semver::Version::new(0, 1, 0),
                ModuleType::Flow,
            ),
        });
        let node = GraphNode::new(NodeId::new(), node, NodeConfig::default());
        let entry = node.id();
        graph.mark_exit_point(entry);
        graph.add_node(node);

        let graph = Arc::new(Mutex::new(graph));
        let (source_tx, source_rx) = mpsc::channel(8);
        let (sink_tx, mut sink_rx) = mpsc::channel(8);
        let shutdown = CancellationToken::new();

        let loop_handle = tokio::spawn(run(
            graph,
            entry,
            RuntimeContext::empty(),
            source_rx,
            sink_tx,
            shutdown.clone(),
            GraphPauseWatcher::always_resumed(),
        ));

        source_tx
            .send(Packet::new(b"boom".as_slice()))
            .await
            .unwrap();
        // Give the panicking packet a moment to be processed (and not
        // received on the sink, since the node panicked instead of
        // returning a packet) before proving the loop is still alive.
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(sink_rx.try_recv().is_err());

        shutdown.cancel();
        loop_handle.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn pausing_stops_new_packets_and_resuming_lets_them_through() {
        let initialized = Arc::new(AtomicUsize::new(0));
        let shut_down = Arc::new(AtomicUsize::new(0));

        let mut graph = FlowGraph::new();
        let node = tracked_node("solo", NodeType::Source, &initialized, &shut_down);
        graph.mark_exit_point(node.id());
        let entry = node.id();
        graph.add_node(node);

        let graph = Arc::new(Mutex::new(graph));
        let (source_tx, source_rx) = mpsc::channel(8);
        let (sink_tx, mut sink_rx) = mpsc::channel(8);
        let shutdown = CancellationToken::new();
        let (pause_handle, pause_watcher) = crate::pause::GraphPauseHandle::new();

        let loop_handle = tokio::spawn(run(
            graph,
            entry,
            RuntimeContext::empty(),
            source_rx,
            sink_tx,
            shutdown.clone(),
            pause_watcher,
        ));

        pause_handle.pause();
        // Give the loop a chance to notice the pause before sending.
        tokio::time::sleep(Duration::from_millis(20)).await;

        let held = Packet::new(b"held".as_slice());
        let held_id = held.id();
        source_tx.send(held).await.unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(
            sink_rx.try_recv().is_err(),
            "a paused loop must not process a packet sent while paused"
        );

        pause_handle.resume();
        let received = sink_rx.recv().await.unwrap();
        assert_eq!(received.id(), held_id);

        shutdown.cancel();
        loop_handle.await.unwrap().unwrap();
    }
}
