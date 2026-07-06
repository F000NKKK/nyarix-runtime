//! Nyarix flow graph engine.
//!
//! This crate holds the graph model that represents packet processing as
//! a directed graph of nodes (transports, crypto, obfuscation, policy,
//! etc.). [`GraphNode`] (#27), [`Edge`] (#28), and [`FlowGraph`] (#29) are
//! the first pieces; validation (#30/#31) and execution (#32+) follow.

pub mod condition;
pub mod edge;
pub mod graph;
pub mod metrics;
pub mod node;

pub use condition::Condition;
pub use edge::{Edge, EdgeType};
pub use graph::FlowGraph;
pub use metrics::NodeMetrics;
pub use node::{GraphNode, NodeConfig, NodeState};
