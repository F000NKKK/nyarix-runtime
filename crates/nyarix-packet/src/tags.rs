//! Tag system for packet classification and routing.
//!
//! Tags are lightweight flags that nodes use to make routing decisions
//! without inspecting the payload.

use bitflags::bitflags;

bitflags! {
    /// Classification tags attached to a packet.
    ///
    /// Tags are used by router nodes, policy engines, and QoS
    /// to determine how a packet should be handled.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub struct Tags: u32 {
        // ── Traffic class ──────────────────────────
        /// Interactive traffic (low latency, e.g., VoIP, gaming).
        const INTERACTIVE  = 1 << 0;
        /// Bulk data transfer (high throughput, latency-tolerant).
        const BULK         = 1 << 1;
        /// Control plane traffic (handshake, key exchange, routing).
        const CONTROL      = 1 << 2;
        /// Diagnostic / introspection traffic.
        const DIAGNOSTIC   = 1 << 3;

        // ── Protocol phase ─────────────────────────
        /// Initial handshake packet.
        const HANDSHAKE    = 1 << 4;
        /// Keep-alive / heartbeat.
        const HEARTBEAT    = 1 << 5;
        /// Key rotation / rekey.
        const REKEY        = 1 << 6;
        /// Graceful shutdown signal.
        const SHUTDOWN     = 1 << 7;

        // ── Network state ──────────────────────────
        /// Roaming — the network interface changed.
        const ROAMING      = 1 << 8;
        /// Fallback — the primary path failed, using alternative.
        const FALLBACK     = 1 << 9;
        /// Retransmission of a previously sent packet.
        const RETRANSMIT   = 1 << 10;
        /// Packet is fragmented (more fragments follow).
        const FRAGMENT     = 1 << 11;
        /// Last fragment in a series.
        const LAST_FRAGMENT = 1 << 12;

        // ── Priority overrides ─────────────────────
        /// High priority — expedite processing.
        const HIGH_PRIORITY = 1 << 13;
        /// Low priority — can be delayed or dropped.
        const LOW_PRIORITY  = 1 << 14;

        // ── Special ────────────────────────────────
        /// Initial packet of a new flow.
        const FLOW_START    = 1 << 15;
        /// Final packet of a flow.
        const FLOW_END      = 1 << 16;
        /// The packet should be processed locally, not forwarded.
        const LOCAL_ONLY    = 1 << 17;
    }
}

impl Tags {
    /// Create an empty tag set.
    #[must_use]
    pub fn new() -> Self {
        Self::empty()
    }

    /// Insert a single tag.
    pub fn insert(&mut self, tag: Tag) {
        match tag {
            Tag::Interactive => self.insert(Self::INTERACTIVE),
            Tag::Bulk => self.insert(Self::BULK),
            Tag::Control => self.insert(Self::CONTROL),
            Tag::Diagnostic => self.insert(Self::DIAGNOSTIC),
            Tag::Handshake => self.insert(Self::HANDSHAKE),
            Tag::Heartbeat => self.insert(Self::HEARTBEAT),
            Tag::Rekey => self.insert(Self::REKEY),
            Tag::Shutdown => self.insert(Self::SHUTDOWN),
            Tag::Roaming => self.insert(Self::ROAMING),
            Tag::Fallback => self.insert(Self::FALLBACK),
            Tag::Retransmit => self.insert(Self::RETRANSMIT),
            Tag::Fragment => self.insert(Self::FRAGMENT),
            Tag::LastFragment => self.insert(Self::LAST_FRAGMENT),
            Tag::HighPriority => self.insert(Self::HIGH_PRIORITY),
            Tag::LowPriority => self.insert(Self::LOW_PRIORITY),
            Tag::FlowStart => self.insert(Self::FLOW_START),
            Tag::FlowEnd => self.insert(Self::FLOW_END),
            Tag::LocalOnly => self.insert(Self::LOCAL_ONLY),
        }
    }

    /// Check if a tag is set.
    #[must_use]
    pub fn contains(&self, tag: Tag) -> bool {
        match tag {
            Tag::Interactive => self.contains(Self::INTERACTIVE),
            Tag::Bulk => self.contains(Self::BULK),
            Tag::Control => self.contains(Self::CONTROL),
            Tag::Diagnostic => self.contains(Self::DIAGNOSTIC),
            Tag::Handshake => self.contains(Self::HANDSHAKE),
            Tag::Heartbeat => self.contains(Self::HEARTBEAT),
            Tag::Rekey => self.contains(Self::REKEY),
            Tag::Shutdown => self.contains(Self::SHUTDOWN),
            Tag::Roaming => self.contains(Self::ROAMING),
            Tag::Fallback => self.contains(Self::FALLBACK),
            Tag::Retransmit => self.contains(Self::RETRANSMIT),
            Tag::Fragment => self.contains(Self::FRAGMENT),
            Tag::LastFragment => self.contains(Self::LAST_FRAGMENT),
            Tag::HighPriority => self.contains(Self::HIGH_PRIORITY),
            Tag::LowPriority => self.contains(Self::LOW_PRIORITY),
            Tag::FlowStart => self.contains(Self::FLOW_START),
            Tag::FlowEnd => self.contains(Self::FLOW_END),
            Tag::LocalOnly => self.contains(Self::LOCAL_ONLY),
        }
    }

    /// Check if this packet is interactive traffic.
    #[must_use]
    pub fn is_interactive(&self) -> bool {
        self.contains(Self::INTERACTIVE)
    }

    /// Check if this is a control plane packet.
    #[must_use]
    pub fn is_control(&self) -> bool {
        self.contains(Self::CONTROL)
    }

    /// Check if this packet is part of a handshake.
    #[must_use]
    pub fn is_handshake(&self) -> bool {
        self.contains(Self::HANDSHAKE)
    }
}

/// Individual tags (for ergonomic use without bitflags syntax).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tag {
    Interactive,
    Bulk,
    Control,
    Diagnostic,
    Handshake,
    Heartbeat,
    Rekey,
    Shutdown,
    Roaming,
    Fallback,
    Retransmit,
    Fragment,
    LastFragment,
    HighPriority,
    LowPriority,
    FlowStart,
    FlowEnd,
    LocalOnly,
}

impl Default for Tags {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_tags() {
        let tags = Tags::new();
        assert!(!tags.contains(Tag::Interactive));
        assert!(!tags.is_control());
    }

    #[test]
    fn insert_and_check() {
        let mut tags = Tags::new();
        tags.insert(Tag::Interactive);
        tags.insert(Tag::Control);

        assert!(tags.is_interactive());
        assert!(tags.is_control());
        assert!(!tags.contains(Tag::Handshake));
    }

    #[test]
    fn bitflags_ops() {
        let mut tags = Tags::new();
        tags.insert(Tag::Handshake);
        tags.insert(Tag::Heartbeat);

        assert!(tags.contains(Tags::HANDSHAKE | Tags::HEARTBEAT));
        assert!(!tags.contains(Tags::HANDSHAKE | Tags::FRAGMENT));
    }
}
