//! Graph Builder integration (see issue #42): building a [`FlowGraph`]
//! from a profile.
//!
//! **Scope note:** "Поиск модулей по спецификации в профиле" takes a
//! `resolve` callback rather than reaching into
//! [`nyarix_loader::ModuleRegistry`] (#57) directly. That registry
//! stores `Arc<dyn Module>`, but a graph node needs `Arc<dyn Node>` — a
//! *different* trait object (`Node: Module` is a supertrait
//! relationship, but Rust can't turn a `dyn Module` back into a `dyn
//! Node`; the extra vtable entries `node_type`/`input_queue_depth`/
//! `output_connections` need aren't there once a value is only known as
//! `dyn Module`). Deciding how loaded modules end up as `dyn Node`
//! (store `Arc<dyn Node>` in the registry instead? a second registry?)
//! is a design question for whichever issue makes Module instantiation
//! (#57) real, which itself waits on #107. `resolve` sidesteps the
//! question entirely for now: the caller supplies however it gets an
//! `Arc<dyn Node>` by name — a test double today, a real registry once
//! that's decided.
//!
//! "Чтение профиля" itself isn't implemented here either —
//! [`nyarix_config::RuntimeConfig::from_toml`]/`from_file` (already
//! existing, pre-#42) already parses TOML profiles; this module starts
//! from an already-parsed [`ProfileConfig`]. YAML profiles aren't
//! supported — nothing in `nyarix-config` parses YAML today, and adding
//! a second format is a config-layer decision, not this issue's.

use std::sync::Arc;

use nyarix_config::ProfileConfig;
use nyarix_core::NodeId;
use nyarix_error::GraphError;
use nyarix_graph::{Edge, EdgeType, FlowGraph, GraphNode, NodeConfig};
use nyarix_module_api::Node;

/// Building a [`FlowGraph`] from a profile failed.
#[derive(Debug, thiserror::Error)]
pub enum BuildError {
    /// A name in the profile's `stack` has no module `resolve` could
    /// find.
    #[error("profile references unknown module: {0}")]
    UnknownModule(String),
    /// The resulting graph failed DAG validation (see
    /// [`FlowGraph::validate`]) — shouldn't happen for a linear chain,
    /// but checked rather than assumed.
    #[error(transparent)]
    Graph(#[from] GraphError),
}

/// Build a linear [`FlowGraph`] from `profile.stack`: one node per name,
/// in order, each looked up via `resolve`, connected by
/// [`EdgeType::Sequential`] edges — the first node marked as the graph's
/// entry point, the last as its exit point.
///
/// # Errors
/// Returns [`BuildError::UnknownModule`] naming the first `stack` entry
/// `resolve` returned `None` for, or [`BuildError::Graph`] if the
/// resulting graph fails [`FlowGraph::validate`].
pub fn build_from_profile(
    profile: &ProfileConfig,
    mut resolve: impl FnMut(&str) -> Option<Arc<dyn Node>>,
) -> Result<FlowGraph, BuildError> {
    let mut graph = FlowGraph::new();
    let mut node_ids: Vec<NodeId> = Vec::with_capacity(profile.stack.len());

    for name in &profile.stack {
        let module = resolve(name).ok_or_else(|| BuildError::UnknownModule(name.clone()))?;
        let node = GraphNode::new(NodeId::new(), module, NodeConfig::default());
        node_ids.push(node.id());
        graph.add_node(node);
    }

    for pair in node_ids.windows(2) {
        let (edge, _receiver) = Edge::new(
            pair[0],
            pair[1],
            EdgeType::Sequential,
            None,
            NodeConfig::DEFAULT_QUEUE_CAPACITY,
        );
        graph.connect(edge)?;
    }

    if let Some(&first) = node_ids.first() {
        graph.mark_entry_point(first);
    }
    if let Some(&last) = node_ids.last() {
        graph.mark_exit_point(last);
    }

    graph.validate()?;

    Ok(graph)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nyarix_error::ModuleError;
    use nyarix_module_api::{Health, ModuleMetadata, ModuleType, NodeType, RuntimeContext};
    use nyarix_packet::Packet;
    use std::collections::HashMap;

    struct StubModule {
        metadata: ModuleMetadata,
    }

    impl StubModule {
        fn new(name: &str) -> Arc<dyn Node> {
            Arc::new(Self {
                metadata: ModuleMetadata::new(name, semver::Version::new(0, 1, 0), ModuleType::Flow),
            })
        }
    }

    impl nyarix_module_api::Module for StubModule {
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

    impl Node for StubModule {
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

    fn profile(stack: &[&str]) -> ProfileConfig {
        ProfileConfig {
            name: "test".to_string(),
            description: String::new(),
            stack: stack.iter().map(|s| (*s).to_string()).collect(),
            policy: HashMap::new(),
        }
    }

    fn registry(names: &[&str]) -> HashMap<String, Arc<dyn Node>> {
        names
            .iter()
            .map(|name| ((*name).to_string(), StubModule::new(name)))
            .collect()
    }

    #[test]
    fn builds_a_linear_chain_from_the_stack() {
        let profile = profile(&["a", "b", "c"]);
        let modules = registry(&["a", "b", "c"]);

        let graph = build_from_profile(&profile, |name| modules.get(name).cloned()).unwrap();

        assert_eq!(graph.node_count(), 3);
        assert_eq!(graph.edge_count(), 2);
    }

    #[test]
    fn a_single_node_stack_is_both_entry_and_exit() {
        let profile = profile(&["a"]);
        let modules = registry(&["a"]);

        let graph = build_from_profile(&profile, |name| modules.get(name).cloned()).unwrap();

        assert_eq!(graph.node_count(), 1);
        assert_eq!(graph.entry_points().len(), 1);
        assert_eq!(graph.exit_points().len(), 1);
        assert_eq!(graph.entry_points(), graph.exit_points());
    }

    #[test]
    fn an_empty_stack_builds_an_empty_graph() {
        let profile = profile(&[]);

        let graph = build_from_profile(&profile, |_| None).unwrap();

        assert_eq!(graph.node_count(), 0);
    }

    #[test]
    fn an_unresolvable_module_name_is_a_clear_error() {
        let profile = profile(&["a", "missing"]);
        let modules = registry(&["a"]);

        let err = build_from_profile(&profile, |name| modules.get(name).cloned()).unwrap_err();

        assert!(matches!(err, BuildError::UnknownModule(name) if name == "missing"));
    }

    #[test]
    fn stack_order_determines_edge_direction() {
        let profile = profile(&["a", "b"]);
        let modules = registry(&["a", "b"]);

        let graph = build_from_profile(&profile, |name| modules.get(name).cloned()).unwrap();

        let entry = graph.entry_points()[0];
        let exit = graph.exit_points()[0];
        assert!(graph.find_path(entry, exit).is_some());
    }
}
