//! Debug dump system (see issue #88): snapshotting the Runtime's full
//! state to a file for offline diagnosis.
//!
//! [`build_debug_dump`] assembles a [`DebugDump`] from whatever the
//! caller already has in hand (a [`FlowGraph`], a [`MetricRegistry`],
//! a [`RuntimeConfig`], a start [`Instant`]) — same "recording
//! primitive, not automatic wiring" shape as [`crate::runtime_metrics`]
//! (#83), for the same reason: `RuntimeHandle` doesn't hold a live
//! graph/registry/start time yet, and building them automatically at
//! startup is #103/#112's job, not this issue's.
//!
//! [`sigusr1_signal`] is this issue's "Триггер: сигнал (SIGUSR1)" —
//! real, working, Unix-only (there's no Windows equivalent to
//! `SIGUSR1`; this project's own [`nyarix_module_api::Platform`]
//! includes non-Unix targets, so this is intentionally
//! `#[cfg(unix)]`-gated rather than faked cross-platform).
//!
//! **Scope note on the other two triggers:**
//! - "API-вызов" needs an HTTP/RPC endpoint — same gap #86/#123
//!   already tracks (no server framework exists in this workspace).
//! - "Crash" needs a global `std::panic::set_hook` installed at
//!   Runtime startup, capturing a dump before the process unwinds
//!   further — a real decision about whether the Runtime (a library,
//!   not an application) should install a *global* panic hook at all
//!   (as opposed to the already-existing per-module
//!   [`crate::sandbox::catch_module_panic`], #75, which is scoped to
//!   one module's call, not the whole process) that shouldn't be
//!   guessed at here. Tracked separately (#124).

use std::path::Path;
use std::time::Instant;

use nyarix_config::RuntimeConfig;
use nyarix_graph::{FlowGraph, GraphExport, export_graph};
use nyarix_module_api::MetricRegistry;
use serde::Serialize;

/// A full snapshot of the Runtime's state, ready to write to a file.
#[derive(Debug, Clone, Serialize)]
pub struct DebugDump {
    /// The flow graph's current shape (#86) — nodes double as this
    /// issue's "активные модули" (each carries its module name, type,
    /// and lifecycle state); a separate module-registry dump would
    /// just repeat the same list, since #113 hasn't decided whether
    /// loaded modules are tracked anywhere beyond the graph they're
    /// wired into.
    pub graph: GraphExport,
    /// Every currently registered metric (#80), as parsed JSON rather
    /// than a pre-encoded string, so it nests cleanly instead of
    /// double-escaping when this whole struct is serialized.
    pub metrics: serde_json::Value,
    /// The Runtime's configuration.
    pub config: RuntimeConfig,
    /// How long the Runtime had been up when this dump was taken.
    pub uptime_seconds: u64,
}

/// Assemble a [`DebugDump`] from `graph`, `metrics`, `config`, and
/// `started_at`.
#[must_use]
pub fn build_debug_dump(
    graph: &FlowGraph,
    metrics: &MetricRegistry,
    config: &RuntimeConfig,
    started_at: Instant,
) -> DebugDump {
    let metrics_json = serde_json::from_str(&metrics.export_json()).unwrap_or(serde_json::Value::Null);
    DebugDump {
        graph: export_graph(graph, Some(metrics)),
        metrics: metrics_json,
        config: config.clone(),
        uptime_seconds: started_at.elapsed().as_secs(),
    }
}

impl DebugDump {
    /// Serialize as pretty-printed JSON (this issue's "Формат: JSON
    /// или MessagePack" — JSON satisfies the either/or ask; adding
    /// MessagePack too wouldn't add anything this bullet requires).
    #[must_use]
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".to_string())
    }

    /// Write this dump as JSON to `path` (this issue's "Дамп полного
    /// состояния Runtime в файл").
    ///
    /// # Errors
    /// Returns the underlying [`std::io::Error`] if `path` can't be
    /// written.
    pub fn write_to_file(&self, path: &Path) -> std::io::Result<()> {
        std::fs::write(path, self.to_json())
    }
}

/// A stream of `SIGUSR1` signals (this issue's "Триггер: сигнал
/// (SIGUSR1)") — the caller awaits `.recv()` on the returned
/// [`tokio::signal::unix::Signal`] and builds/writes a
/// [`DebugDump`] each time it fires, the same way
/// [`crate::shutdown::cancel_on_ctrl_c`] is a signal source a caller
/// reacts to rather than something that acts on its own.
///
/// # Errors
/// Returns the underlying [`std::io::Error`] if installing the signal
/// handler fails (e.g. a conflicting handler already installed).
#[cfg(unix)]
pub fn sigusr1_signal() -> std::io::Result<tokio::signal::unix::Signal> {
    tokio::signal::unix::signal(tokio::signal::unix::SignalKind::user_defined1())
}

#[cfg(test)]
mod tests {
    use super::*;
    use nyarix_error::ModuleError;
    use nyarix_graph::{GraphNode, NodeConfig};
    use nyarix_module_api::{
        Health, Module, ModuleMetadata, ModuleType, Node, NodeType, RuntimeContext,
    };
    use nyarix_packet::Packet;
    use std::sync::Arc;
    use std::time::Duration;

    struct StubNode {
        metadata: ModuleMetadata,
    }

    impl Module for StubNode {
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

    impl Node for StubNode {
        fn node_type(&self) -> NodeType {
            NodeType::Transformer
        }
        fn input_queue_depth(&self) -> usize {
            0
        }
        fn output_connections(&self) -> &[nyarix_core::NodeId] {
            &[]
        }
    }

    fn graph_with_one_node() -> FlowGraph {
        let mut graph = FlowGraph::new();
        let module: Arc<dyn Node> = Arc::new(StubNode {
            metadata: ModuleMetadata::new("a", semver::Version::new(0, 1, 0), ModuleType::Flow),
        });
        let node = GraphNode::new(nyarix_core::NodeId::new(), module, NodeConfig::default());
        let id = node.id();
        graph.add_node(node);
        graph.mark_entry_point(id);
        graph.mark_exit_point(id);
        graph
    }

    #[test]
    fn build_debug_dump_includes_graph_metrics_and_config() {
        let graph = graph_with_one_node();
        let metrics = MetricRegistry::new();
        metrics.counter("a", "process_calls_total").increment(1);
        let config = RuntimeConfig::from_toml(r#"mode = "client""#).unwrap();
        let started_at = Instant::now() - Duration::from_secs(10);

        let dump = build_debug_dump(&graph, &metrics, &config, started_at);

        assert_eq!(dump.graph.nodes.len(), 1);
        assert!(dump.uptime_seconds >= 10);
        assert_eq!(dump.config.mode, nyarix_config::RuntimeMode::Client);
        assert!(
            dump.metrics
                .to_string()
                .contains("nyarix.module.a.process_calls_total")
        );
    }

    #[test]
    fn to_json_produces_valid_json() {
        let graph = graph_with_one_node();
        let metrics = MetricRegistry::new();
        let config = RuntimeConfig::from_toml(r#"mode = "client""#).unwrap();
        let dump = build_debug_dump(&graph, &metrics, &config, Instant::now());

        let json = dump.to_json();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(parsed.get("graph").is_some());
        assert!(parsed.get("config").is_some());
    }

    #[test]
    fn write_to_file_writes_valid_json() {
        let graph = graph_with_one_node();
        let metrics = MetricRegistry::new();
        let config = RuntimeConfig::from_toml(r#"mode = "client""#).unwrap();
        let dump = build_debug_dump(&graph, &metrics, &config, Instant::now());

        let path = std::env::temp_dir().join(format!(
            "nyarix-debug-dump-test-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        dump.write_to_file(&path).unwrap();

        let contents = std::fs::read_to_string(&path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&contents).unwrap();
        assert!(parsed.get("graph").is_some());

        let _ = std::fs::remove_file(&path);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn sigusr1_signal_can_be_installed() {
        // Just confirms the handler installs without error; actually
        // sending SIGUSR1 to the test process and awaiting `.recv()`
        // would need a real OS-level signal, out of scope for a unit
        // test.
        assert!(sigusr1_signal().is_ok());
    }
}
