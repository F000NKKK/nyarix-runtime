//! Metadata attached to each packet.
//!
//! Metadata carries session, flow, routing, and observability context
//! that nodes use to make decisions without inspecting the payload.

use std::time::Instant;

use nyarix_core::{FlowId, ModuleId, NodeId, RouteId, SessionId, StreamId};
use serde::{Deserialize, Serialize};

/// Default TTL (time-to-live) for packets traversing the graph.
pub const DEFAULT_TTL: u8 = 32;

/// Metadata attached to every packet.
///
/// All fields are cheap to clone — they are either `Copy` types
/// or reference-counted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    ///
    /// Serialized as a relative millisecond offset from the moment of
    /// encoding, then reconstructed as `Instant::now() + offset` on decode
    /// (see `Packet::encode`/`Packet::decode`) — `Instant` itself carries no
    /// serializable wall-clock meaning, so exact equality across an
    /// encode/decode round-trip is not guaranteed when a deadline is set.
    #[serde(with = "deadline_millis")]
    pub deadline: Option<Instant>,
    /// MTU hint for downstream nodes.
    pub mtu_hint: Option<u16>,
    /// Latency requirement hint in microseconds.
    /// `None` means "don't care."
    pub latency_hint_us: Option<u64>,
    /// Reliability requirement hint.
    pub reliability: Reliability,

    // ── Lifecycle / Tracing ──────────────────────────
    /// Wall-clock instant when this packet was created (#84).
    ///
    /// Serialized as a relative millisecond offset (how long
    /// ago the packet was created) — same convention as
    /// [`deadline`](Self::deadline) but in reverse, so
    /// encode/decode round-trips within the same process are
    /// approximately preserved.
    #[serde(with = "created_at_millis")]
    pub created_at: Instant,
    /// Time-to-live in graph hops.
    pub ttl: u8,
    /// Number of times this packet has been retried.
    pub retry_count: u8,
    /// The module that originally created this packet.
    pub origin_module: Option<ModuleId>,
    /// Hop-by-hop trace of node IDs (optional, for debugging).
    pub trace: Vec<NodeId>,
    /// Whether this packet's path is actively being traced (#87) — a
    /// sampling decision made once (typically at creation, e.g. by a
    /// sampling rate), not re-decided per hop. `false` by default, so
    /// tracing costs nothing for packets that don't opt in — this is
    /// #87's "Sampling: не каждый пакет, чтобы не замедлять".
    pub traced: bool,
    /// Whether [`Self::record_hop`] should also capture a timestamp per
    /// hop (#87's "Опциональный детальный режим: timestamps per hop")
    /// — only meaningful when [`Self::traced`] is `true`.
    pub trace_detailed: bool,
    /// Elapsed milliseconds since [`Self::created_at`] at each hop
    /// recorded via [`Self::record_hop`] — parallel to [`Self::trace`]
    /// (same length and order), populated only when
    /// [`Self::trace_detailed`].
    pub trace_timestamps_ms: Vec<u64>,

    // ── Security / Privacy ───────────────────────────
    /// Privacy policy hint for obfuscation/padding nodes.
    pub privacy_policy: PrivacyPolicy,
    /// Capability mask of the creator.
    pub capability_mask: u64,
}

/// Milliseconds between two instants — `later - earlier`, saturating at
/// zero if `earlier` is actually later, and clamped to `u64::MAX` rather
/// than overflowing. Shared by [`created_at_millis`] and
/// [`deadline_millis`]'s serde helpers below: both encode an `Instant`
/// as a relative millisecond offset, just in opposite temporal
/// directions (a creation time is in the past relative to "now"; a
/// deadline is in the future), so this is the one piece of arithmetic
/// both actually share — the surrounding (de)serialization shape
/// (`Instant` vs `Option<Instant>`, subtract-from-now vs add-to-now on
/// the way back) differs enough that unifying further would need a
/// direction parameter threaded through, not a clear win over two thin
/// modules.
fn millis_between(later: Instant, earlier: Instant) -> u64 {
    u64::try_from(later.saturating_duration_since(earlier).as_millis()).unwrap_or(u64::MAX)
}

/// Serde helper: encodes `Instant` as "milliseconds ago" (#84).
///
/// On serialize, stores how long ago `created_at` was; on deserialize,
/// reconstructs `Instant::now() - ms_ago` — approximate within the same
/// process, but exact equality is not guaranteed across an encode/decode
/// boundary (the same caveat as [`deadline_millis`]).
mod created_at_millis {
    use std::time::{Duration, Instant};

    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    use super::millis_between;

    pub(super) fn serialize<S: Serializer>(
        value: &Instant,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        millis_between(Instant::now(), *value).serialize(serializer)
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Instant, D::Error> {
        let ms_ago = u64::deserialize(deserializer)?;
        Ok(Instant::now() - Duration::from_millis(ms_ago))
    }
}

/// Serde helper: encodes `Option<Instant>` as a relative millisecond offset.
mod deadline_millis {
    use std::time::{Duration, Instant};

    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    use super::millis_between;

    pub(super) fn serialize<S: Serializer>(
        value: &Option<Instant>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        let millis_from_now = value.map(|deadline| millis_between(deadline, Instant::now()));
        millis_from_now.serialize(serializer)
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<Instant>, D::Error> {
        let millis_from_now = Option::<u64>::deserialize(deserializer)?;
        Ok(millis_from_now.map(|ms| Instant::now() + Duration::from_millis(ms)))
    }
}

/// The role of a node in the network topology.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
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

/// One entry in [`Metadata::trace_summary`] (#87): a visited node,
/// paired with when it was visited if detailed timing was captured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceHop {
    /// The node visited.
    pub node: NodeId,
    /// Milliseconds since packet creation when this hop was recorded,
    /// if [`Metadata::trace_detailed`] was set.
    pub elapsed_ms: Option<u64>,
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
            created_at: Instant::now(),
            ttl: DEFAULT_TTL,
            retry_count: 0,
            origin_module: None,
            trace: Vec::new(),
            traced: false,
            trace_detailed: false,
            trace_timestamps_ms: Vec::new(),
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

    /// Add a node to the trace unconditionally — the low-level
    /// primitive; prefer [`Self::record_hop`] for actual execution-path
    /// tracing (#87), which respects [`Self::traced`]/sampling.
    pub fn trace_push(&mut self, node: NodeId) {
        self.trace.push(node);
    }

    /// Opt this packet into tracing (#87) — the execution engine calls
    /// [`Self::record_hop`] unconditionally on every hop, but it's a
    /// no-op unless this was called first, so most packets pay nothing
    /// (this issue's "Sampling: не каждый пакет"). `detailed` also
    /// captures a timestamp per hop.
    #[must_use]
    pub fn with_tracing(mut self, detailed: bool) -> Self {
        self.traced = true;
        self.trace_detailed = detailed;
        self
    }

    /// Record one hop for tracing (#87) if [`Self::traced`] — a no-op
    /// otherwise, so callers (the execution engine) can call this on
    /// every hop without checking `traced` themselves first.
    ///
    /// If [`Self::trace_detailed`], also appends the elapsed
    /// milliseconds since [`Self::created_at`] to
    /// [`Self::trace_timestamps_ms`].
    pub fn record_hop(&mut self, node: NodeId) {
        if !self.traced {
            return;
        }
        self.trace.push(node);
        if self.trace_detailed {
            let elapsed_ms =
                u64::try_from(self.created_at.elapsed().as_millis()).unwrap_or(u64::MAX);
            self.trace_timestamps_ms.push(elapsed_ms);
        }
    }

    /// Pair each traced hop with its timestamp, if detailed timing was
    /// recorded (#87's "Экспорт трассы для диагностики") — `None` per
    /// hop when [`Self::trace_detailed`] wasn't set, same length as
    /// [`Self::trace`] either way.
    #[must_use]
    pub fn trace_summary(&self) -> Vec<TraceHop> {
        self.trace
            .iter()
            .enumerate()
            .map(|(i, &node)| TraceHop {
                node,
                elapsed_ms: self.trace_timestamps_ms.get(i).copied(),
            })
            .collect()
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

    #[test]
    fn created_at_is_set_at_packet_creation() {
        let before = Instant::now();
        let meta = Metadata::new();
        let after = Instant::now();
        assert!(meta.created_at >= before);
        assert!(meta.created_at <= after);
    }
}
