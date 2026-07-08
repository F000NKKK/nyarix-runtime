//! The `Node` trait — the contract for a Flow Graph node (see issue #17).

use nyarix_core::NodeId;
use serde::{Deserialize, Serialize};

use crate::module::Module;

/// The functional category of a graph node.
///
/// Distinct from [`crate::metadata::ModuleType`]: `ModuleType` classifies a
/// module as a *package* (what it is), `NodeType` classifies it as a
/// *position in the Flow Graph* (what role it plays once wired in) — a
/// single module can in principle be instantiated as different node types
/// in different graphs (e.g. a metrics module as both an `Observer` and,
/// via a different profile, feeding an `Aggregator`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NodeType {
    /// Produces packets (entry point into the graph).
    Source,
    /// Transforms a packet into another packet.
    Transformer,
    /// Conditionally drops packets.
    Filter,
    /// Encrypts/decrypts payloads.
    Encryptor,
    /// Packs/unpacks payload framing.
    Packer,
    /// Masks traffic shape, timing, or structure.
    Obfuscator,
    /// Delivers packets to/from the network.
    Transport,
    /// Selects the outgoing path for a packet.
    Router,
    /// Combines multiple packets/streams into one (#96).
    ///
    /// **Merge semantics are entirely the module's own `Module::process`
    /// logic — the execution engine needs no special support for this
    /// node type.** Each of `N` converging branches (multiple upstream
    /// edges targeting the same `Aggregator` node — already structurally
    /// possible, since [`NodeId`] is just a graph-wide identifier and
    /// nothing stops two edges from sharing a `to`) calls `process` once,
    /// same as any other node. The module tracks how many it's seen in
    /// its own state and:
    /// - returns `Ok(None)` (absorbed, per #19) for every branch before
    ///   the last one it's waiting for;
    /// - returns `Ok(Some(merged))` on whichever call completes the
    ///   set it decided to wait for.
    ///
    /// This composes with `nyarix_graph::execute_parallel`'s existing
    /// contract with zero changes to the engine: a branch that gets
    /// `None` back contributes nothing to the result `Vec<Packet>`
    /// (already true for any absorbing node, #19); the branch that
    /// gets `Some` continues on normally. Whether "the set it decided
    /// to wait for" means all upstream branches, first-N, or a
    /// timeout-bounded subset is the module's own policy — the engine
    /// has no opinion and needs none.
    Aggregator,
    /// Observes packets without altering them.
    Observer,
    /// Makes policy decisions (fallback, padding, priority, ...).
    PolicyResolver,
    /// Terminal node — consumes packets, produces none.
    Sink,
}

/// The contract for a node in the Flow Graph.
///
/// Every `Node` is also a [`Module`]: the graph only cares about wiring
/// (`node_type`, queue depth, outgoing edges), while packet processing
/// itself is the `Module` contract it extends.
pub trait Node: Module {
    /// The role this node plays in the Flow Graph.
    fn node_type(&self) -> NodeType;

    /// Current depth of this node's pending input queue.
    ///
    /// Used by the scheduler (M4) and by policy/observability nodes to
    /// detect backpressure.
    fn input_queue_depth(&self) -> usize;

    /// The nodes this node forwards packets to.
    ///
    /// An empty slice means this node is a `Sink` (or currently
    /// disconnected). Populated by the Graph Builder (M3) when the graph is
    /// assembled.
    fn output_connections(&self) -> &[NodeId];
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::RuntimeContext;
    use crate::metadata::{ModuleMetadata, ModuleType};
    use crate::module::Result;
    use nyarix_packet::Packet;

    struct RouterNode {
        metadata: ModuleMetadata,
        outputs: Vec<NodeId>,
        queue_depth: usize,
    }

    impl Module for RouterNode {
        fn metadata(&self) -> &ModuleMetadata {
            &self.metadata
        }

        fn initialize(&mut self, _ctx: &RuntimeContext) -> Result<()> {
            Ok(())
        }

        fn process(&mut self, packet: Packet) -> Result<Option<Packet>> {
            Ok(Some(packet))
        }

        fn shutdown(&mut self, _ctx: &RuntimeContext) -> Result<()> {
            Ok(())
        }
    }

    impl Node for RouterNode {
        fn node_type(&self) -> NodeType {
            NodeType::Router
        }

        fn input_queue_depth(&self) -> usize {
            self.queue_depth
        }

        fn output_connections(&self) -> &[NodeId] {
            &self.outputs
        }
    }

    #[test]
    fn node_reports_type_and_topology() {
        let downstream = [NodeId::new(), NodeId::new()];
        let node = RouterNode {
            metadata: ModuleMetadata::new(
                "router",
                semver::Version::new(0, 1, 0),
                ModuleType::Flow,
            ),
            outputs: downstream.to_vec(),
            queue_depth: 3,
        };

        assert_eq!(node.node_type(), NodeType::Router);
        assert_eq!(node.input_queue_depth(), 3);
        assert_eq!(node.output_connections(), downstream.as_slice());
    }

    #[test]
    fn sink_has_no_output_connections() {
        let node = RouterNode {
            metadata: ModuleMetadata::new("sink", semver::Version::new(0, 1, 0), ModuleType::Flow),
            outputs: Vec::new(),
            queue_depth: 0,
        };

        assert!(node.output_connections().is_empty());
    }

    /// A `Node` is usable wherever a `Module` is expected (trait object
    /// upcasting via a plain fn boundary).
    #[test]
    fn node_is_usable_as_module() {
        fn accepts_module(m: &dyn Module) -> &ModuleMetadata {
            m.metadata()
        }

        let node = RouterNode {
            metadata: ModuleMetadata::new(
                "router",
                semver::Version::new(0, 1, 0),
                ModuleType::Flow,
            ),
            outputs: Vec::new(),
            queue_depth: 0,
        };
        assert_eq!(accepts_module(&node).name, "router");
    }
}
