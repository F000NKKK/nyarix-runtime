//! Payload abstraction — zero-copy byte buffer.
//!
//! Uses `bytes::Bytes` internally for:
//! - Cheap cloning via reference counting
//! - Zero-copy slicing
//! - Efficient conversion from/to `Vec<u8>`, `&[u8]`

use bytes::{Bytes, BytesMut};

/// A zero-copy payload buffer.
///
/// `Payload` wraps `Bytes` and provides a clean API for
/// packet data manipulation with minimal allocations.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Payload {
    bytes: Bytes,
}

impl Payload {
    /// Create an empty payload.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            bytes: Bytes::new(),
        }
    }

    /// Create a payload from `Bytes`.
    #[must_use]
    pub fn from_bytes(bytes: Bytes) -> Self {
        Self { bytes }
    }

    /// Create a payload from a byte vector.
    #[must_use]
    pub fn from_vec(vec: Vec<u8>) -> Self {
        Self {
            bytes: Bytes::from(vec),
        }
    }

    /// Create a payload from a static byte slice.
    #[must_use]
    pub fn from_static(slice: &'static [u8]) -> Self {
        Self {
            bytes: Bytes::from_static(slice),
        }
    }

    /// Get the payload data as a byte slice.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Get the length of the payload.
    #[must_use]
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    /// Check if the payload is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    /// Take a slice of the payload without copying.
    ///
    /// # Panics
    /// Panics if the range is out of bounds.
    #[must_use]
    pub fn slice(&self, range: impl std::ops::RangeBounds<usize>) -> Self {
        Self {
            bytes: self.bytes.slice(range),
        }
    }

    /// Split the payload at `at`, returning two new payloads.
    /// The original payload is consumed.
    #[must_use]
    pub fn split_off(self, at: usize) -> (Self, Self) {
        let right = self.bytes.slice(at..);
        let left = self.bytes.slice(..at);
        (Self { bytes: left }, Self { bytes: right })
    }

    /// Convert the payload into `Bytes`.
    #[must_use]
    pub fn into_bytes(self) -> Bytes {
        self.bytes
    }

    /// Get a mutable reference for building a payload.
    /// Returns a `PayloadBuilder`.
    #[must_use]
    pub fn builder() -> PayloadBuilder {
        PayloadBuilder::new()
    }
}

impl From<Bytes> for Payload {
    fn from(bytes: Bytes) -> Self {
        Self::from_bytes(bytes)
    }
}

impl From<Vec<u8>> for Payload {
    fn from(vec: Vec<u8>) -> Self {
        Self::from_vec(vec)
    }
}

impl From<&[u8]> for Payload {
    fn from(slice: &[u8]) -> Self {
        Self::from_vec(slice.to_vec())
    }
}

impl From<&str> for Payload {
    fn from(s: &str) -> Self {
        Self::from_vec(s.as_bytes().to_vec())
    }
}

impl From<Payload> for Bytes {
    fn from(p: Payload) -> Self {
        p.into_bytes()
    }
}

impl std::ops::Deref for Payload {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        self.as_bytes()
    }
}

impl AsRef<[u8]> for Payload {
    fn as_ref(&self) -> &[u8] {
        self.as_bytes()
    }
}

/// Builder for constructing a `Payload` incrementally.
pub struct PayloadBuilder {
    buf: BytesMut,
}

impl PayloadBuilder {
    /// Create a new payload builder with default capacity.
    #[must_use]
    pub fn new() -> Self {
        Self {
            buf: BytesMut::with_capacity(1500), // Typical MTU
        }
    }

    /// Create a new builder with a specified capacity.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            buf: BytesMut::with_capacity(capacity),
        }
    }

    /// Append bytes to the payload.
    pub fn extend(&mut self, data: &[u8]) -> &mut Self {
        self.buf.extend_from_slice(data);
        self
    }

    /// Get the current length.
    #[must_use]
    pub fn len(&self) -> usize {
        self.buf.len()
    }

    /// Check if the builder is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    /// Build the final `Payload`.
    #[must_use]
    pub fn build(self) -> Payload {
        Payload {
            bytes: self.buf.freeze(),
        }
    }

    /// Get a mutable reference to the underlying buffer.
    pub fn as_mut(&mut self) -> &mut BytesMut {
        &mut self.buf
    }
}

impl Default for PayloadBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_payload() {
        let p = Payload::empty();
        assert!(p.is_empty());
        assert_eq!(p.len(), 0);
    }

    #[test]
    fn payload_clone_is_shallow() {
        let data = vec![0u8; 1024];
        let p1 = Payload::from_vec(data);
        let p2 = p1.clone();

        assert_eq!(p1.len(), p2.len());
        assert_eq!(p1.as_bytes().as_ptr(), p2.as_bytes().as_ptr());
    }

    #[test]
    fn payload_slice_is_zero_copy() {
        let p = Payload::from_vec(b"hello world".to_vec());
        let slice = p.slice(0..5);
        assert_eq!(slice.as_bytes(), b"hello");
    }

    #[test]
    fn builder_constructs_payload() {
        let mut builder = Payload::builder();
        builder.extend(b"hello ").extend(b"world");
        let payload = builder.build();
        assert_eq!(payload.as_bytes(), b"hello world");
    }

    #[test]
    fn split_off() {
        let p = Payload::from_vec(b"abcdef".to_vec());
        let (left, right) = p.split_off(3);
        assert_eq!(left.as_bytes(), b"abc");
        assert_eq!(right.as_bytes(), b"def");
    }
}
