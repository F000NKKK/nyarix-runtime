//! Nyarix flow graph engine.
//!
//! This crate holds the graph model that represents packet processing as
//! a directed graph of nodes (transports, crypto, obfuscation, policy,
//! etc.). [`GraphNode`] (#27), [`Edge`] (#28), and [`FlowGraph`] (#29) are
//! the storage layer; validation (#30/#31) checks it; [`execute_sequential`]
//! (#32) is the first (linear-only) execution engine.

pub mod condition;
pub mod edge;
pub mod execution;
pub mod graph;
pub mod metrics;
pub mod node;

pub use condition::Condition;
pub use edge::{Edge, EdgeType};
pub use execution::{execute_sequential, ExecutionError};
pub use graph::FlowGraph;
pub use metrics::NodeMetrics;
pub use node::{GraphNode, NodeConfig, NodeState};
