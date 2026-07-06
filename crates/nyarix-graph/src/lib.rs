//! Nyarix flow graph engine.
//!
//! This crate holds the graph model that represents packet processing as
//! a directed graph of nodes (transports, crypto, obfuscation, policy,
//! etc.). [`GraphNode`] (#27) is the first piece; edges (#28), storage
//! (#29), validation (#30/#31), and execution (#32+) follow.

pub mod metrics;
pub mod node;

pub use metrics::NodeMetrics;
pub use node::{GraphNode, NodeConfig, NodeState};
