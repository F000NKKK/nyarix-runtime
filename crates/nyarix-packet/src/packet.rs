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
use nyarix_core::PacketId;

use crate::metadata::Metadata;
use crate::payload::Payload;
use crate::tags::Tags;

/// The central data unit flowing through the Flow Graph.
///
/// `Packet` uses `Arc` internally for cheap cloning — when a node
/// only needs to read or mutate metadata, the payload is shared.
#[derive(Clone)]
pub struct Packet {
    inner: Arc<PacketInner>,
}

#[derive(Clone)]
struct PacketInner {
    id: PacketId,
    payload: Payload,
    metadata: Metadata,
    tags: Tags,
}

impl Packet {
    /// Create a new packet with the given payload.
    #[must_use]
    pub fn new(payload: impl Into<Payload>) -> Self {
        Self {
            inner: Arc::new(PacketInner {
                id: PacketId::new(),
                payload: payload.into(),
                metadata: Metadata::new(),
                tags: Tags::new(),
            }),
        }
    }

    /// Create a new packet with full metadata.
    #[must_use]
    pub fn with_metadata(payload: impl Into<Payload>, metadata: Metadata) -> Self {
        Self {
            inner: Arc::new(PacketInner {
                id: PacketId::new(),
                payload: payload.into(),
                metadata,
                tags: Tags::new(),
            }),
        }
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
}
