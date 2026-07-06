//! The Packet — the central data unit of Nyarix.
//!
//! A Packet consists of:
//! - `id` — unique packet identifier
//! - `payload` — the actual byte data (zero-copy)
//! - `metadata` — session/flow/routing context
//! - `tags` — classification flags for routing decisions

use std::fmt;
use std::sync::Arc;

use bytes::Bytes;
use nyarix_core::{FlowId, PacketId};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::metadata::Metadata;
use crate::payload::Payload;
use crate::tags::Tags;

/// The central data unit flowing through the Flow Graph.
///
/// `Packet` uses `Arc` internally for cheap cloning — when a node
/// only needs to read or mutate metadata, the payload is shared.
pub struct Packet {
    inner: Arc<PacketInner>,
}

struct PacketInner {
    id: PacketId,
    payload: Payload,
    metadata: Metadata,
    tags: Tags,
}

impl Clone for PacketInner {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            payload: self.payload.clone(),
            metadata: self.metadata.clone(),
            tags: self.tags,
        }
    }
}

impl Drop for PacketInner {
    fn drop(&mut self) {
        tracing::trace!(packet_id = %self.id, "packet dropped");
    }
}

impl Packet {
    /// Wrap a freshly built [`PacketInner`], emitting the creation tracing
    /// event exactly once per underlying allocation.
    fn from_inner(inner: PacketInner) -> Self {
        tracing::trace!(packet_id = %inner.id, "packet created");
        Self {
            inner: Arc::new(inner),
        }
    }

    /// Create a new packet with the given payload.
    #[must_use]
    pub fn new(payload: impl Into<Payload>) -> Self {
        Self::from_inner(PacketInner {
            id: PacketId::new(),
            payload: payload.into(),
            metadata: Metadata::new(),
            tags: Tags::new(),
        })
    }

    /// Create a new packet with full metadata.
    #[must_use]
    pub fn with_metadata(payload: impl Into<Payload>, metadata: Metadata) -> Self {
        Self::from_inner(PacketInner {
            id: PacketId::new(),
            payload: payload.into(),
            metadata,
            tags: Tags::new(),
        })
    }

    /// Get the unique packet identifier.
    #[must_use]
    pub fn id(&self) -> PacketId {
        self.inner.id
    }

    /// Assign a new unique identifier to this packet.
    /// This clones the inner `Arc` if there are other references.
    pub fn reset_id(&mut self) {
        Arc::make_mut(&mut self.inner).id = PacketId::new();
    }

    /// Get a reference to the payload.
    #[must_use]
    pub fn payload(&self) -> &Payload {
        &self.inner.payload
    }

    /// Get the payload data as bytes.
    #[must_use]
    pub fn data(&self) -> &[u8] {
        self.inner.payload.as_bytes()
    }

    /// Get the payload length.
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.payload.len()
    }

    /// Check if the payload is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.inner.payload.is_empty()
    }

    /// Get a reference to the metadata.
    #[must_use]
    pub fn metadata(&self) -> &Metadata {
        &self.inner.metadata
    }

    /// Get a mutable reference to the metadata.
    /// This clones the inner `Arc` if there are other references.
    pub fn metadata_mut(&mut self) -> &mut Metadata {
        Arc::make_mut(&mut self.inner).metadata = self.inner.metadata.clone();
        &mut Arc::make_mut(&mut self.inner).metadata
    }

    /// Get a reference to the tags.
    #[must_use]
    pub fn tags(&self) -> &Tags {
        &self.inner.tags
    }

    /// Get a mutable reference to the tags.
    pub fn tags_mut(&mut self) -> &mut Tags {
        &mut Arc::make_mut(&mut self.inner).tags
    }

    /// Set the payload, replacing the current one.
    /// This clones the inner `Arc` if there are other references.
    pub fn set_payload(&mut self, payload: impl Into<Payload>) {
        Arc::make_mut(&mut self.inner).payload = payload.into();
    }

    /// Take the payload out of the packet, replacing it with an empty payload.
    pub fn take_payload(&mut self) -> Payload {
        let inner = Arc::make_mut(&mut self.inner);
        std::mem::replace(&mut inner.payload, Payload::empty())
    }

    /// Split the payload into a `Bytes` and leave an empty payload.
    pub fn take_bytes(&mut self) -> Bytes {
        let inner = Arc::make_mut(&mut self.inner);
        std::mem::replace(&mut inner.payload, Payload::empty()).into_bytes()
    }

    /// Add a tag to the packet.
    pub fn tag(&mut self, tag: crate::tags::Tag) {
        self.tags_mut().insert_tag(tag);
    }

    /// Check if the packet has a specific tag.
    #[must_use]
    pub fn has_tag(&self, tag: crate::tags::Tag) -> bool {
        self.tags().has_tag(tag)
    }

    /// Get the TTL (time-to-live) for this packet in the graph.
    /// Decremented at each hop; packet is dropped when it reaches 0.
    #[must_use]
    pub fn ttl(&self) -> u8 {
        self.inner.metadata.ttl
    }

    /// Decrement the TTL. Returns `true` if the packet is still alive.
    pub fn decrement_ttl(&mut self) -> bool {
        let meta = self.metadata_mut();
        if meta.ttl == 0 {
            return false;
        }
        meta.ttl -= 1;
        meta.ttl > 0
    }

    /// Encode this packet into a compact binary format for passing it
    /// between graph nodes within the same Runtime process.
    ///
    /// This is **not** a network wire format — it has no versioning or
    /// cross-platform stability guarantees, and its only consumer is
    /// [`Packet::decode`].
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let wire = PacketWire {
            id: self.inner.id,
            payload: self.inner.payload.as_bytes().to_vec(),
            metadata: self.inner.metadata.clone(),
            tags: self.inner.tags,
        };
        bincode::serde::encode_to_vec(&wire, bincode::config::standard())
            .expect("packet encoding does not fail")
    }

    /// Decode a packet previously produced by [`Packet::encode`].
    ///
    /// # Errors
    /// Returns [`DecodeError`] if `buf` is not a validly encoded packet.
    pub fn decode(buf: &[u8]) -> Result<Self, DecodeError> {
        let (wire, _): (PacketWire, usize) =
            bincode::serde::decode_from_slice(buf, bincode::config::standard())?;
        Ok(Self::from_inner(PacketInner {
            id: wire.id,
            payload: Payload::from_vec(wire.payload),
            metadata: wire.metadata,
            tags: wire.tags,
        }))
    }

    /// The flow-level identifier that correlates this packet with every
    /// other packet on the same logical flow, for end-to-end tracing across
    /// graph hops.
    #[must_use]
    pub fn trace_id(&self) -> FlowId {
        self.inner.metadata.flow_id
    }
}

impl Clone for Packet {
    /// Cheap `Arc` clone — emits a tracing event so packet fan-out
    /// (e.g. broadcast nodes) is observable.
    fn clone(&self) -> Self {
        tracing::trace!(packet_id = %self.inner.id, "packet cloned");
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

/// Wire representation used by [`Packet::encode`]/[`Packet::decode`].
#[derive(Serialize, Deserialize)]
struct PacketWire {
    id: PacketId,
    payload: Vec<u8>,
    metadata: Metadata,
    tags: Tags,
}

/// Error returned by [`Packet::decode`] when a buffer is not a validly
/// encoded packet.
#[derive(Debug, Error)]
#[error("failed to decode packet: {0}")]
pub struct DecodeError(#[from] bincode::error::DecodeError);

impl PartialEq for Packet {
    /// Structural equality by value (id, payload bytes, metadata, tags) —
    /// not `Arc` pointer identity.
    fn eq(&self, other: &Self) -> bool {
        self.inner.id == other.inner.id
            && self.inner.payload == other.inner.payload
            && self.inner.metadata == other.inner.metadata
            && self.inner.tags == other.inner.tags
    }
}

impl fmt::Debug for Packet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Packet")
            .field("id", &self.inner.id)
            .field("len", &self.inner.payload.len())
            .field("metadata", &self.inner.metadata)
            .field("tags", &self.inner.tags)
            .finish()
    }
}

impl fmt::Display for Packet {
    /// Human-readable one-liner for logs, e.g.
    /// `Packet[0199...] 42B flow=0198... ttl=32 tags=(0x1)`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Packet[{}] {}B flow={} ttl={} tags={:?}",
            self.inner.id,
            self.inner.payload.len(),
            self.inner.metadata.flow_id,
            self.inner.metadata.ttl,
            self.inner.tags
        )
    }
}

impl From<Bytes> for Packet {
    fn from(bytes: Bytes) -> Self {
        Self::new(Payload::from_bytes(bytes))
    }
}

impl From<Vec<u8>> for Packet {
    fn from(vec: Vec<u8>) -> Self {
        Self::new(Payload::from(vec))
    }
}

impl From<&[u8]> for Packet {
    fn from(slice: &[u8]) -> Self {
        Self::new(Payload::from(slice.to_vec()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tags::Tag;

    #[test]
    fn packet_creation() {
        let pkt = Packet::new(b"hello".as_slice());
        assert_eq!(pkt.data(), b"hello");
        assert_eq!(pkt.len(), 5);
        assert!(!pkt.is_empty());
    }

    #[test]
    fn packet_clone_is_cheap() {
        let mut pkt = Packet::new(b"shared data".as_slice());
        let clone = pkt.clone();

        // Both point to the same payload
        assert_eq!(pkt.data(), clone.data());

        // Mutating tags clones the inner Arc
        pkt.tag(Tag::Interactive);
        assert!(pkt.has_tag(Tag::Interactive));
        assert!(!clone.has_tag(Tag::Interactive));
    }

    #[test]
    fn packet_ttl() {
        let mut pkt = Packet::new(b"data".as_slice());
        // Default TTL is set in Metadata
        let initial = pkt.ttl();
        assert!(initial > 0);

        for _ in 0..initial {
            let alive = pkt.decrement_ttl();
            if pkt.ttl() == 0 {
                assert!(!alive);
            }
        }
    }

    #[test]
    fn empty_packet() {
        let pkt = Packet::new(Payload::empty());
        assert!(pkt.is_empty());
        assert_eq!(pkt.len(), 0);
    }

    #[test]
    fn display_is_human_readable() {
        let pkt = Packet::new(b"hello".as_slice());
        let rendered = pkt.to_string();
        assert!(rendered.starts_with("Packet["));
        assert!(rendered.contains("5B"));
        assert!(rendered.contains(&pkt.id().to_string()));
    }

    #[test]
    fn trace_id_matches_flow() {
        let pkt = Packet::new(b"data".as_slice());
        assert_eq!(pkt.trace_id(), pkt.metadata().flow_id);

        let clone = pkt.clone();
        assert_eq!(clone.trace_id(), pkt.trace_id());
    }

    #[test]
    fn encode_decode_round_trip() {
        let mut pkt = Packet::new(b"hello world".as_slice());
        pkt.tag(Tag::Interactive);
        pkt.metadata_mut().priority = 200;

        let encoded = pkt.encode();
        let decoded = Packet::decode(&encoded).unwrap();

        assert_eq!(decoded, pkt);
        assert_eq!(decoded.data(), pkt.data());
    }

    #[test]
    fn encode_decode_preserves_deadline_approximately() {
        use std::time::{Duration, Instant};

        let mut pkt = Packet::new(b"data".as_slice());
        pkt.metadata_mut().deadline = Some(Instant::now() + Duration::from_secs(5));

        let decoded = Packet::decode(&pkt.encode()).unwrap();

        let original_deadline = pkt.metadata().deadline.unwrap();
        let decoded_deadline = decoded.metadata().deadline.unwrap();
        let drift = decoded_deadline
            .saturating_duration_since(original_deadline)
            .max(original_deadline.saturating_duration_since(decoded_deadline));
        assert!(
            drift < Duration::from_millis(50),
            "deadline drifted by {drift:?}"
        );
    }

    #[test]
    fn decode_rejects_garbage() {
        assert!(Packet::decode(&[0xff, 0x00, 0x13, 0x37]).is_err());
    }
}
