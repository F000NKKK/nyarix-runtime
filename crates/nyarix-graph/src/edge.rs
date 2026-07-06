//! Graph edge structure (see issue #28) and backpressure metrics (#35).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use nyarix_core::NodeId;
use nyarix_packet::Packet;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::PollSender;

use crate::condition::Condition;

/// How an edge routes packets from one node to the next.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EdgeType {
    /// Always taken; the default linear pipeline connection.
    Sequential,
    /// Taken only if this edge's [`Condition`] evaluates to `true`.
    Conditional,
    /// One of several edges a node fans out to concurrently.
    Parallel,
    /// Taken when the primary path is unavailable/failed.
    Fallback,
}

/// A directed connection between two nodes in the Flow Graph, carrying an
/// mpsc channel that packets are actually sent through.
#[derive(Debug)]
pub struct Edge {
    from: NodeId,
    to: NodeId,
    edge_type: EdgeType,
    condition: Option<Condition>,
    queue: mpsc::Sender<Packet>,
    /// Packets lost to [`Self::try_send_or_drop`] finding the queue full
    /// (see #35).
    dropped: Arc<AtomicU64>,
}

impl Edge {
    /// Create a new edge with a bounded channel of the given capacity.
    ///
    /// Returns the `Edge` (holding the sending half) and the `Receiver`,
    /// which the caller attaches to the downstream node's input queue.
    ///
    /// `condition` is only meaningful when `edge_type` is
    /// [`EdgeType::Conditional`] — this constructor doesn't enforce that
    /// pairing; consistency checks are the Graph Builder's job (#30 DAG
    /// validation), not this data structure's.
    #[must_use]
    pub fn new(
        from: NodeId,
        to: NodeId,
        edge_type: EdgeType,
        condition: Option<Condition>,
        capacity: usize,
    ) -> (Self, mpsc::Receiver<Packet>) {
        let (sender, receiver) = mpsc::channel(capacity);
        (
            Self {
                from,
                to,
                edge_type,
                condition,
                queue: sender,
                dropped: Arc::new(AtomicU64::new(0)),
            },
            receiver,
        )
    }

    /// The source node.
    #[must_use]
    pub const fn from(&self) -> NodeId {
        self.from
    }

    /// The destination node.
    #[must_use]
    pub const fn to(&self) -> NodeId {
        self.to
    }

    /// How this edge routes packets.
    #[must_use]
    pub const fn edge_type(&self) -> EdgeType {
        self.edge_type
    }

    /// The routing condition, if this is a [`EdgeType::Conditional`] edge.
    #[must_use]
    pub const fn condition(&self) -> Option<&Condition> {
        self.condition.as_ref()
    }

    /// Whether a packet should be routed along this edge, given its
    /// condition (edges without a condition always accept).
    #[must_use]
    pub fn accepts(&self, packet: &Packet) -> bool {
        self.condition
            .as_ref()
            .map_or(true, |condition| condition.evaluate(packet))
    }

    /// Attempt to send a packet along this edge without waiting for queue
    /// space.
    ///
    /// # Errors
    /// Returns the packet back if the queue is full or the receiver has
    /// been dropped.
    pub fn try_send(&self, packet: Packet) -> Result<(), mpsc::error::TrySendError<Packet>> {
        self.queue.try_send(packet)
    }

    /// Send a packet along this edge, waiting for queue space if needed.
    ///
    /// # Errors
    /// Returns the packet back if the receiver has been dropped.
    pub async fn send(&self, packet: Packet) -> Result<(), mpsc::error::SendError<Packet>> {
        self.queue.send(packet).await
    }

    /// A `Sink<Packet>` for this edge's queue (see issue #34), for code
    /// that wants to drive packet flow through the `futures`-style
    /// `Stream`/`Sink` combinators instead of calling [`Self::send`]
    /// directly.
    #[must_use]
    pub fn sink(&self) -> PollSender<Packet> {
        PollSender::new(self.queue.clone())
    }

    /// Attempt to send a packet, and if the queue is full, drop it
    /// instead of propagating the error — incrementing
    /// [`Self::dropped_count`] (see #35).
    ///
    /// Returns `true` if the packet was enqueued, `false` if it was
    /// dropped. A dropped receiver (not just a full queue) also counts
    /// as a drop here — from the sender's perspective, the packet didn't
    /// get through either way.
    pub fn try_send_or_drop(&self, packet: Packet) -> bool {
        match self.queue.try_send(packet) {
            Ok(()) => true,
            Err(_) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
                false
            }
        }
    }

    /// How many packets have been dropped via [`Self::try_send_or_drop`]
    /// on this edge since it was created.
    #[must_use]
    pub fn dropped_count(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// Current number of packets sitting in this edge's queue.
    #[must_use]
    pub fn queue_depth(&self) -> usize {
        self.queue.max_capacity() - self.queue.capacity()
    }
}

/// Wrap the `Receiver` half returned by [`Edge::new`] as a
/// `Stream<Item = Packet>` (see issue #34), for use at a node's input with
/// `futures`/`tokio-stream`-style combinators instead of calling
/// `Receiver::recv` directly.
#[must_use]
pub fn packet_stream(receiver: mpsc::Receiver<Packet>) -> ReceiverStream<Packet> {
    ReceiverStream::new(receiver)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nyarix_packet::Tag;

    #[test]
    fn unconditional_edge_accepts_everything() {
        let (edge, _rx) = Edge::new(NodeId::new(), NodeId::new(), EdgeType::Sequential, None, 8);
        let pkt = Packet::new(b"data".as_slice());
        assert!(edge.accepts(&pkt));
    }

    #[test]
    fn conditional_edge_respects_condition() {
        let (edge, _rx) = Edge::new(
            NodeId::new(),
            NodeId::new(),
            EdgeType::Conditional,
            Some(Condition::HasTag(Tag::Interactive)),
            8,
        );

        let plain = Packet::new(b"data".as_slice());
        assert!(!edge.accepts(&plain));

        let mut tagged = Packet::new(b"data".as_slice());
        tagged.tag(Tag::Interactive);
        assert!(edge.accepts(&tagged));
    }

    #[tokio::test]
    async fn packet_flows_through_the_channel() {
        let (edge, mut rx) = Edge::new(NodeId::new(), NodeId::new(), EdgeType::Sequential, None, 8);
        let pkt = Packet::new(b"payload".as_slice());
        let id = pkt.id();

        edge.send(pkt).await.unwrap();
        let received = rx.recv().await.unwrap();
        assert_eq!(received.id(), id);
    }

    #[tokio::test]
    async fn sink_and_stream_wrappers_carry_a_packet() {
        use tokio_stream::StreamExt;

        let (edge, rx) = Edge::new(NodeId::new(), NodeId::new(), EdgeType::Sequential, None, 8);
        let mut stream = packet_stream(rx);
        let mut sink = edge.sink();

        let pkt = Packet::new(b"data".as_slice());
        let id = pkt.id();

        std::future::poll_fn(|cx| sink.poll_reserve(cx))
            .await
            .unwrap();
        sink.send_item(pkt).unwrap();

        let received = stream.next().await.unwrap();
        assert_eq!(received.id(), id);
    }

    #[test]
    fn try_send_fails_when_queue_is_full() {
        let (edge, _rx) = Edge::new(NodeId::new(), NodeId::new(), EdgeType::Sequential, None, 1);

        edge.try_send(Packet::new(b"first".as_slice())).unwrap();
        let overflow = edge.try_send(Packet::new(b"second".as_slice()));
        assert!(overflow.is_err());
    }

    #[test]
    fn try_send_or_drop_counts_overflow() {
        let (edge, _rx) = Edge::new(NodeId::new(), NodeId::new(), EdgeType::Sequential, None, 1);

        assert!(edge.try_send_or_drop(Packet::new(b"first".as_slice())));
        assert_eq!(edge.dropped_count(), 0);

        assert!(!edge.try_send_or_drop(Packet::new(b"second".as_slice())));
        assert_eq!(edge.dropped_count(), 1);

        assert!(!edge.try_send_or_drop(Packet::new(b"third".as_slice())));
        assert_eq!(edge.dropped_count(), 2);
    }

    #[test]
    fn queue_depth_reflects_pending_packets() {
        let (edge, _rx) = Edge::new(NodeId::new(), NodeId::new(), EdgeType::Sequential, None, 4);
        assert_eq!(edge.queue_depth(), 0);

        edge.try_send(Packet::new(b"one".as_slice())).unwrap();
        edge.try_send(Packet::new(b"two".as_slice())).unwrap();
        assert_eq!(edge.queue_depth(), 2);
    }
}
