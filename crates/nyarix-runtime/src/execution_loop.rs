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
//! `execute_sequential`; graph mutation mid-run (#37/#38/#39, blocked on
//! #98); and the CPU/I/O worker-pool split (#45/#46) — this is a single
//! async task, not yet backed by dedicated thread pools.

use std::sync::Arc;

use nyarix_core::NodeId;
use nyarix_error::ModuleError;
use nyarix_graph::FlowGraph;
use nyarix_module_api::RuntimeContext;
use nyarix_packet::Packet;
use thiserror::Error;
use tokio::sync::{mpsc, Mutex};
use tokio_util::sync::CancellationToken;

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
pub async fn shutdown_all_nodes(
    graph: &Arc<Mutex<FlowGraph>>,
    ctx: &RuntimeContext,
) -> Result<(), ModuleError> {
    let mut guard = graph.lock().await;
    let ids: Vec<NodeId> = guard.node_ids().collect();
    let mut first_error = None;
    for id in ids {
        if let Some(node) = guard.node_mut(id) {
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

/// Run the main execution loop.
///
/// # Errors
/// Returns [`ExecutionLoopError::Initialize`] if any node fails to
/// initialize — the loop never starts in that case. Packet-processing
/// errors encountered *during* the loop are logged via `tracing`, not
/// propagated: one bad packet shouldn't take down the whole Runtime.
pub async fn run(
    graph: Arc<Mutex<FlowGraph>>,
    entry: NodeId,
    ctx: RuntimeContext,
    mut source: mpsc::Receiver<Packet>,
    sink: mpsc::Sender<Packet>,
    shutdown: CancellationToken,
) -> Result<(), ExecutionLoopError> {
    initialize_all_nodes(&graph, &ctx).await?;

    loop {
        let packet = tokio::select! {
            () = shutdown.cancelled() => break,
            received = source.recv() => match received {
                Some(packet) => packet,
                None => break,
            },
        };

        let result = {
            let mut guard = graph.lock().await;
            nyarix_graph::execute_sequential(&mut guard, entry, packet)
        };

        match result {
            Ok(Some(output)) => {
                if sink.send(output).await.is_err() {
                    tracing::warn!("execution loop sink closed; stopping");
                    break;
                }
            }
            Ok(None) => {}
            Err(error) => {
                tracing::warn!(%error, "packet processing failed");
            }
        }
    }

    if let Err(error) = shutdown_all_nodes(&graph, &ctx).await {
        tracing::warn!(%error, "a node failed to shut down cleanly during loop exit");
    }

    Ok(())
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
        ));

        drop(source_tx);
        loop_handle.await.unwrap().unwrap();
        assert_eq!(shut_down.load(Ordering::SeqCst), 1);
    }
}
