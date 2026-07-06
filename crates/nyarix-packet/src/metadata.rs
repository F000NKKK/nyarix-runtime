//! Metadata attached to each packet.
//!
//! Metadata carries session, flow, routing, and observability context
//! that nodes use to make decisions without inspecting the payload.

use std::time::Instant;

use nyarix_core::{FlowId, ModuleId, NodeId, RouteId, SessionId, StreamId};

/// Default TTL (time-to-live) for packets traversing the graph.
pub const DEFAULT_TTL: u8 = 32;

/// Metadata attached to every packet.
///
/// All fields are cheap to clone — they are either `Copy` types
/// or reference-counted.
#[derive(Debug, Clone)]
pub struct Metadata {
    // ── Identity ─────────────────────────────────────
    /// The session this packet belongs to.
    pub session_id: SessionId,
    /// The flow within the session.
    pub flow_id: FlowId,
    /// The stream within the flow (for multiplexed transport).
    pub stream_id: StreamId,

    // ── Routing ──────────────────────────────────────
    /// The current route identifier.
    pub route_id: RouteId,
    /// The source module that created this packet.
    pub source_node: Option<NodeId>,
    /// The intended destination module.
    pub destination_node: Option<NodeId>,
    /// The source role (client/server/relay).
    pub source_role: Option<Role>,
    /// The destination role.
    pub destination_role: Option<Role>,

    // ── QoS / Priority ───────────────────────────────
    /// Priority level (0 = lowest, 255 = highest).
    pub priority: u8,
    /// Deadline for this packet (wall clock).
    pub deadline: Option<Instant>,
    /// MTU hint for downstream nodes.
    pub mtu_hint: Option<u16>,
    /// Latency requirement hint in microseconds.
    /// `None` means "don't care."
    pub latency_hint_us: Option<u64>,
    /// Reliability requirement hint.
    pub reliability: Reliability,

    // ── Lifecycle / Tracing ──────────────────────────
    /// Time-to-live in graph hops.
    pub ttl: u8,
    /// Number of times this packet has been retried.
    pub retry_count: u8,
    /// The module that originally created this packet.
    pub origin_module: Option<ModuleId>,
    /// Hop-by-hop trace of node IDs (optional, for debugging).
    pub trace: Vec<NodeId>,

    // ── Security / Privacy ───────────────────────────
    /// Privacy policy hint for obfuscation/padding nodes.
    pub privacy_policy: PrivacyPolicy,
    /// Capability mask of the creator.
    pub capability_mask: u64,
}

/// The role of a node in the network topology.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// Client — originates user traffic.
    Client,
    /// Server — terminates user traffic.
    Server,
    /// Relay — forwards between parties.
    Relay,
    /// Gateway — ingress/egress point.
    Gateway,
}

/// Reliability requirement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Reliability {
    /// Best-effort delivery (UDP-like).
    #[default]
    BestEffort,
    /// Reliable delivery with retransmission (TCP-like).
    Reliable,
    /// At-most-once delivery.
    AtMostOnce,
}

/// Privacy policy for obfuscation decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PrivacyPolicy {
    /// No special privacy requirements.
    #[default]
    None,
    /// Standard obfuscation.
    Standard,
    /// Maximum stealth — aggressive obfuscation and padding.
    Maximum,
    /// Minimal — avoid unnecessary overhead.
    Minimal,
}

impl Metadata {
    /// Create default metadata with a new session, flow, and stream ID.
    #[must_use]
    pub fn new() -> Self {
        Self {
            session_id: SessionId::new(),
            flow_id: FlowId::new(),
            stream_id: StreamId::new(),
            route_id: RouteId::nil(),
            source_node: None,
            destination_node: None,
            source_role: None,
            destination_role: None,
            priority: 128,
            deadline: None,
            mtu_hint: None,
            latency_hint_us: None,
            reliability: Reliability::default(),
            ttl: DEFAULT_TTL,
            retry_count: 0,
            origin_module: None,
            trace: Vec::new(),
            privacy_policy: PrivacyPolicy::default(),
            capability_mask: 0,
        }
    }

    /// Set the session ID (e.g., for packets belonging to an existing session).
    pub fn with_session(mut self, id: SessionId) -> Self {
        self.session_id = id;
        self
    }

    /// Set the flow ID.
    pub fn with_flow(mut self, id: FlowId) -> Self {
        self.flow_id = id;
        self
    }

    /// Set the source node.
    pub fn with_source(mut self, node: NodeId) -> Self {
        self.source_node = Some(node);
        self
    }

    /// Set the destination node.
    pub fn with_destination(mut self, node: NodeId) -> Self {
        self.destination_node = Some(node);
        self
    }

    /// Add a node to the trace.
    pub fn trace_push(&mut self, node: NodeId) {
        self.trace.push(node);
    }

    /// Reset TTL to default.
    pub fn reset_ttl(&mut self) {
        self.ttl = DEFAULT_TTL;
    }

    /// Check if the packet has expired.
    #[must_use]
    pub fn is_expired(&self) -> bool {
        if self.ttl == 0 {
            return true;
        }
        if let Some(deadline) = self.deadline {
            return Instant::now() >= deadline;
        }
        false
    }
}

impl Default for Metadata {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_metadata() {
        let meta = Metadata::new();
        assert!(!meta.session_id.is_nil());
        assert!(!meta.flow_id.is_nil());
        assert_eq!(meta.ttl, DEFAULT_TTL);
        assert_eq!(meta.priority, 128);
    }

    #[test]
    fn metadata_builder_pattern() {
        let session = SessionId::new();
        let flow = FlowId::new();
        let node = NodeId::new();

        let meta = Metadata::new()
            .with_session(session)
            .with_flow(flow)
            .with_source(node);

        assert_eq!(meta.session_id, session);
        assert_eq!(meta.flow_id, flow);
        assert_eq!(meta.source_node, Some(node));
    }

    #[test]
    fn ttl_expiry() {
        let mut meta = Metadata::new();
        assert!(!meta.is_expired());

        meta.ttl = 0;
        assert!(meta.is_expired());
    }
}
