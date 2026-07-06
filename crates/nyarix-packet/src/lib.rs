//! Nyarix Packet model.
//!
//! `Packet` is the central data unit flowing through the graph.
//! It wraps a `Payload` (zero-copy byte buffer), `Metadata`
//! (session/flow/routing context), and `Tags` (classification).
//!
//! The design goals:
//! - Clone is cheap (Arc-based payload sharing)
//! - Metadata mutation does not copy payload
//! - Zero-copy buffer via `bytes::Bytes`
//! - Object pooling for memory reuse
//! - Tracing hooks for observability

pub mod metadata;
pub mod payload;
pub mod pool;
pub mod tags;

mod packet;

pub use metadata::Metadata;
pub use packet::{DecodeError, Packet};
pub use payload::Payload;
pub use pool::PacketPool;
pub use tags::{Tag, Tags};
