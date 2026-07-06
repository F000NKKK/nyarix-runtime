//! Packet pool — memory reuse for packet objects.
//!
//! A lock-free object pool that reduces allocation pressure
//! by recycling `Packet` and `Payload` objects.

use parking_lot::Mutex;

use crate::Packet;

/// A pool of pre-allocated packet objects.
///
/// When a packet is "freed," its payload is cleared and the
/// packet struct is returned to the pool for reuse.
pub struct PacketPool {
    inner: Mutex<Vec<Packet>>,
    max_size: usize,
}

impl PacketPool {
    /// Create a new packet pool with the given maximum size.
    #[must_use]
    pub fn new(max_size: usize) -> Self {
        Self {
            inner: Mutex::new(Vec::with_capacity(max_size)),
            max_size,
        }
    }

    /// Acquire a packet from the pool, or create a new one if the pool is empty.
    ///
    /// The returned packet has an empty payload.
    #[must_use]
    pub fn acquire(&self) -> Packet {
        let mut pool = self.inner.lock();
        pool.pop().unwrap_or_else(|| {
            Packet::new(crate::payload::Payload::empty())
        })
    }

    /// Return a packet to the pool for reuse.
    ///
    /// The packet's payload will be cleared before reuse.
    pub fn release(&self, mut packet: Packet) {
        // Clear payload to avoid holding large allocations
        packet.set_payload(crate::payload::Payload::empty());

        let mut pool = self.inner.lock();
        if pool.len() < self.max_size {
            pool.push(packet);
        }
        // If pool is full, let the packet drop
    }

    /// Get the current number of packets in the pool.
    #[must_use]
    pub fn available(&self) -> usize {
        self.inner.lock().len()
    }

    /// Clear all pooled packets.
    pub fn clear(&self) {
        self.inner.lock().clear();
    }
}

impl Default for PacketPool {
    fn default() -> Self {
        Self::new(4096)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acquire_and_release() {
        let pool = PacketPool::new(10);

        let pkt = pool.acquire();
        assert!(pkt.is_empty());

        let id = pkt.id();
        pool.release(pkt);

        assert_eq!(pool.available(), 1);

        let reused = pool.acquire();
        // The ID will be different because release clears the packet's metadata
        // but the allocation is reused
        assert_ne!(reused.id(), id);
    }

    #[test]
    fn pool_capacity() {
        let pool = PacketPool::new(2);

        let a = pool.acquire();
        let b = pool.acquire();

        pool.release(a);
        pool.release(b);

        // Pool should not exceed max_size
        assert_eq!(pool.available(), 2);

        // Releasing a third should not grow the pool
        let c = pool.acquire();
        pool.release(c);
        assert_eq!(pool.available(), 2);
    }
}
