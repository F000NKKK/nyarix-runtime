//! Graph node structure (see issue #27).

use std::fmt;
use std::sync::Arc;

use nyarix_core::NodeId;
use nyarix_module_api::{ModuleConfig, Node, NodeType};

use crate::metrics::NodeMetrics;

/// The lifecycle state of a graph node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum NodeState {
    /// Constructed, but `initialize` hasn't run yet.
    #[default]
    Uninitialized,
    /// `initialize` is in progress.
    Initializing,
    /// Initialized and processing packets normally.
    Running,
    /// Running, but [`nyarix_module_api::Health`] reported degraded.
    Degraded,
    /// `shutdown` is in progress.
    Stopping,
    /// `shutdown` completed; the node will no longer process packets.
    Stopped,
}

/// Node-level configuration.
#[derive(Debug, Clone, PartialEq)]
pub struct NodeConfig {
    /// Configuration handed to the underlying module.
    pub module_config: ModuleConfig,
    /// Capacity of this node's input queue before backpressure kicks in
    /// (see #35 Backpressure handling, #36 Queue system per node).
    pub queue_capacity: usize,
}

impl NodeConfig {
    /// Default queue capacity, per #36's explicit spec.
    ///
    /// Note: this used to be 256 (matching
    /// `nyarix_config::GlobalDefaults::queue_depth`) until #36 specified
    /// 64 for per-node queues specifically — the two defaults serve
    /// different things (global scheduler-wide default vs. this node-local
    /// one) and don't need to match.
    pub const DEFAULT_QUEUE_CAPACITY: usize = 64;
}

impl Default for NodeConfig {
    fn default() -> Self {
        Self {
            module_config: ModuleConfig::empty(),
            queue_capacity: Self::DEFAULT_QUEUE_CAPACITY,
        }
    }
}

/// A node in the Flow Graph: a [`Node`]-implementing module plus the
/// graph-level bookkeeping around it (lifecycle state, config, metrics).
pub struct GraphNode {
    id: NodeId,
    node_type: NodeType,
    module: Arc<dyn Node>,
    config: NodeConfig,
    state: NodeState,
    metrics: NodeMetrics,
}

impl GraphNode {
    /// Wrap a module as a graph node.
    ///
    /// `node_type` is read from `module.node_type()` rather than taken as a
    /// separate parameter, so it can never drift from what the module
    /// itself reports.
    #[must_use]
    pub fn new(id: NodeId, module: Arc<dyn Node>, config: NodeConfig) -> Self {
        let node_type = module.node_type();
        Self {
            id,
            node_type,
            module,
            config,
            state: NodeState::Uninitialized,
            metrics: NodeMetrics::default(),
        }
    }

    /// This node's identifier.
    #[must_use]
    pub const fn id(&self) -> NodeId {
        self.id
    }

    /// This node's role in the Flow Graph.
    #[must_use]
    pub const fn node_type(&self) -> NodeType {
        self.node_type
    }

    /// The underlying module.
    #[must_use]
    pub fn module(&self) -> &Arc<dyn Node> {
        &self.module
    }

    /// This node's configuration.
    #[must_use]
    pub const fn config(&self) -> &NodeConfig {
        &self.config
    }

    /// This node's current lifecycle state.
    #[must_use]
    pub const fn state(&self) -> NodeState {
        self.state
    }

    /// Transition to a new lifecycle state.
    ///
    /// This does not validate that the transition is legal (e.g.
    /// `Stopped` -> `Running`) — enforcing the state machine is the
    /// execution engine's job (#32+), not this data structure's.
    pub fn set_state(&mut self, state: NodeState) {
        tracing::trace!(node_id = %self.id, ?state, "node state changed");
        self.state = state;
    }

    /// This node's metrics.
    #[must_use]
    pub const fn metrics(&self) -> NodeMetrics {
        self.metrics
    }

    /// Run this node's module on a packet (see #32).
    ///
    /// # Panics
    /// Panics if the module `Arc` has more than one owner — e.g. it was
    /// also resolved as another node's dependency via
    /// `RuntimeContext::resolve_dependency` (#18). `Module::process`
    /// needs exclusive access, and there isn't yet an established
    /// strategy for sharing mutable access to a *running* module; the
    /// Scheduler (M4) will need to settle that properly (locking?
    /// message-passing to an owning task?). Until then, a node's module
    /// must not be shared elsewhere while the graph is executing.
    pub fn process(
        &mut self,
        packet: nyarix_packet::Packet,
    ) -> nyarix_module_api::Result<Option<nyarix_packet::Packet>> {
        Arc::get_mut(&mut self.module)
            .expect("node module Arc must be uniquely owned to process a packet")
            .process(packet)
    }

    /// Call this node's module's `migrate` (see #38 hot swap).
    ///
    /// # Panics
    /// Same condition as [`Self::process`] — the module `Arc` must be
    /// uniquely owned.
    pub fn migrate(
        &mut self,
        ctx: &nyarix_module_api::RuntimeContext,
    ) -> nyarix_module_api::Result<()> {
        Arc::get_mut(&mut self.module)
            .expect("node module Arc must be uniquely owned to migrate")
            .migrate(ctx)
    }

    /// Call this node's module's `initialize` and, on success, transition
    /// to [`NodeState::Running`] (see #43's execution loop, which calls
    /// this on every node before the first `process`).
    ///
    /// # Panics
    /// Same condition as [`Self::process`] — the module `Arc` must be
    /// uniquely owned.
    pub fn initialize(
        &mut self,
        ctx: &nyarix_module_api::RuntimeContext,
    ) -> nyarix_module_api::Result<()> {
        self.set_state(NodeState::Initializing);
        let result = Arc::get_mut(&mut self.module)
            .expect("node module Arc must be uniquely owned to initialize")
            .initialize(ctx);
        if result.is_ok() {
            self.set_state(NodeState::Running);
        }
        result
    }

    /// Call this node's module's `shutdown` and, on success, transition
    /// to [`NodeState::Stopped`] (see #43's execution loop, which calls
    /// this on every node after the last `process`).
    ///
    /// Also cancels any event subscriptions the module registered on
    /// `ctx` via `RuntimeContext::on_event` (#49) — regardless of
    /// whether `shutdown` itself succeeded, so a failing module doesn't
    /// leak subscriber tasks.
    ///
    /// # Panics
    /// Same condition as [`Self::process`] — the module `Arc` must be
    /// uniquely owned.
    pub fn shutdown(
        &mut self,
        ctx: &nyarix_module_api::RuntimeContext,
    ) -> nyarix_module_api::Result<()> {
        self.set_state(NodeState::Stopping);
        let result = Arc::get_mut(&mut self.module)
            .expect("node module Arc must be uniquely owned to shut down")
            .shutdown(ctx);
        ctx.cancel_subscriptions();
        if result.is_ok() {
            self.set_state(NodeState::Stopped);
        }
        result
    }
}

impl fmt::Debug for GraphNode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GraphNode")
            .field("id", &self.id)
            .field("node_type", &self.node_type)
            .field("state", &self.state)
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nyarix_error::ModuleError;
    use nyarix_module_api::{Health, Module, ModuleMetadata, ModuleType, RuntimeContext};
    use nyarix_packet::Packet;

    struct StubRouter {
        metadata: ModuleMetadata,
    }

    impl StubRouter {
        fn new() -> Self {
            Self {
                metadata: ModuleMetadata::new(
                    "router",
                    semver::Version::new(0, 1, 0),
                    ModuleType::Flow,
                ),
            }
        }
    }

    impl Module for StubRouter {
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

    impl Node for StubRouter {
        fn node_type(&self) -> NodeType {
            NodeType::Router
        }

        fn input_queue_depth(&self) -> usize {
            0
        }

        fn output_connections(&self) -> &[NodeId] {
            &[]
        }
    }

    #[test]
    fn node_type_is_derived_from_module() {
        let module: Arc<dyn Node> = Arc::new(StubRouter::new());
        let node = GraphNode::new(NodeId::new(), module, NodeConfig::default());

        assert_eq!(node.node_type(), NodeType::Router);
    }

    #[test]
    fn starts_uninitialized() {
        let module: Arc<dyn Node> = Arc::new(StubRouter::new());
        let node = GraphNode::new(NodeId::new(), module, NodeConfig::default());

        assert_eq!(node.state(), NodeState::Uninitialized);
    }

    #[test]
    fn set_state_updates_state() {
        let module: Arc<dyn Node> = Arc::new(StubRouter::new());
        let mut node = GraphNode::new(NodeId::new(), module, NodeConfig::default());

        node.set_state(NodeState::Running);
        assert_eq!(node.state(), NodeState::Running);

        node.set_state(NodeState::Degraded);
        assert_eq!(node.state(), NodeState::Degraded);
    }

    #[test]
    fn default_queue_capacity_matches_global_default() {
        let config = NodeConfig::default();
        assert_eq!(config.queue_capacity, 64);
    }

    #[tokio::test]
    async fn shutdown_cancels_the_context_s_event_subscriptions() {
        use nyarix_module_api::{Event, EventBus, EventFilter};
        use std::collections::HashMap;
        use std::sync::{Arc as StdArc, Mutex};

        let module: Arc<dyn Node> = Arc::new(StubRouter::new());
        let mut node = GraphNode::new(NodeId::new(), module, NodeConfig::default());

        let bus = StdArc::new(EventBus::default());
        let ctx =
            RuntimeContext::with_event_bus(ModuleConfig::empty(), HashMap::new(), bus.clone());

        let received: StdArc<Mutex<Vec<Event>>> = StdArc::new(Mutex::new(Vec::new()));
        let received_clone = StdArc::clone(&received);
        ctx.on_event(EventFilter::All, move |event| {
            received_clone.lock().unwrap().push(event);
        });

        node.shutdown(&ctx).unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        bus.publish(Event::new("after_shutdown"));
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        assert!(received.lock().unwrap().is_empty());
    }
}
