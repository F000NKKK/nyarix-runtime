//! Per-node priority queue (see issue #36).
//!
//! Wired into [`crate::execution`] (#97): every [`crate::node::GraphNode`]
//! owns one, and both `execute_sequential`/`execute_parallel` (#32/#33)
//! route each hop through it — enqueue via [`NodeQueueSender`], then
//! dequeue with priority ([`NodeQueueReceiver::try_recv_prioritized`]/
//! [`NodeQueueReceiver::recv`]) — instead of calling `Module::process`
//! directly on whatever packet the walker happened to be holding. See
//! [`crate::execution`]'s module docs for why [`crate::edge::Edge`]'s
//! own per-edge channel is untouched by this (it was already unused by
//! execution before #97, and stays that way).

use nyarix_packet::{Packet, Tags};
use tokio::sync::mpsc;

/// Which lane a packet is routed to, based on its [`Tags`] (#10).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Lane {
    Control,
    Interactive,
    Bulk,
}

fn lane_for(packet: &Packet) -> Lane {
    let tags = *packet.tags();
    if tags.contains(Tags::CONTROL) {
        Lane::Control
    } else if tags.contains(Tags::INTERACTIVE) {
        Lane::Interactive
    } else {
        // Bulk-tagged and untagged packets both land here: bulk is the
        // catch-all "no special treatment" lane.
        Lane::Bulk
    }
}

/// The sending half of a node's input queue.
///
/// Cloneable: every upstream edge that feeds this node gets its own
/// clone, and packets from all of them land in the same three lanes.
#[derive(Clone)]
pub struct NodeQueueSender {
    control: mpsc::Sender<Packet>,
    interactive: mpsc::Sender<Packet>,
    bulk: mpsc::Sender<Packet>,
}

impl NodeQueueSender {
    /// Send a packet, waiting for space in its lane if needed.
    ///
    /// # Errors
    /// Returns the packet back if the corresponding receiver lane has
    /// been dropped.
    pub async fn send(&self, packet: Packet) -> Result<(), mpsc::error::SendError<Packet>> {
        match lane_for(&packet) {
            Lane::Control => self.control.send(packet).await,
            Lane::Interactive => self.interactive.send(packet).await,
            Lane::Bulk => self.bulk.send(packet).await,
        }
    }

    /// Attempt to send a packet without waiting for lane space.
    ///
    /// # Errors
    /// Returns the packet back if its lane is full or its receiver has
    /// been dropped.
    pub fn try_send(&self, packet: Packet) -> Result<(), mpsc::error::TrySendError<Packet>> {
        match lane_for(&packet) {
            Lane::Control => self.control.try_send(packet),
            Lane::Interactive => self.interactive.try_send(packet),
            Lane::Bulk => self.bulk.try_send(packet),
        }
    }
}

/// The receiving half of a node's input queue.
pub struct NodeQueueReceiver {
    control: mpsc::Receiver<Packet>,
    interactive: mpsc::Receiver<Packet>,
    bulk: mpsc::Receiver<Packet>,
}

impl NodeQueueReceiver {
    /// Receive the next packet, preferring control over interactive over
    /// bulk (a `biased` `tokio::select!` — checked in that order every
    /// time, not fair-random like a plain `select!`).
    ///
    /// Returns `None` once every lane's sender half has been dropped and
    /// drained.
    pub async fn recv(&mut self) -> Option<Packet> {
        tokio::select! {
            biased;
            packet = self.control.recv() => packet,
            packet = self.interactive.recv() => packet,
            packet = self.bulk.recv() => packet,
        }
    }

    /// Try to receive the next packet without waiting, preferring
    /// control over interactive over bulk (the same biased order as
    /// [`Self::recv`], but synchronous — for callers that can't
    /// `.await`, e.g. [`crate::execution::execute_sequential`], #97).
    ///
    /// Returns `None` if every lane is currently empty (not
    /// necessarily closed — unlike `recv`, this doesn't distinguish
    /// "nothing right now" from "never again").
    #[must_use]
    pub fn try_recv_prioritized(&mut self) -> Option<Packet> {
        if let Ok(packet) = self.control.try_recv() {
            return Some(packet);
        }
        if let Ok(packet) = self.interactive.try_recv() {
            return Some(packet);
        }
        self.bulk.try_recv().ok()
    }

    /// Total packets currently buffered across all three lanes (#97) —
    /// the real, live number behind [`nyarix_module_api::Node::input_queue_depth`]
    /// once a node's `NodeQueue` is what actually carries its input,
    /// as opposed to whatever constant a module reports on its own.
    #[must_use]
    pub fn len(&self) -> usize {
        self.control.len() + self.interactive.len() + self.bulk.len()
    }

    /// Whether every lane is currently empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Drain every currently-queued packet across all three lanes
    /// (control first, then interactive, then bulk), without waiting for
    /// more to arrive. Used during graceful shutdown to hand off or
    /// discard whatever was still in flight.
    pub fn drain(&mut self) -> Vec<Packet> {
        let mut drained = Vec::new();
        while let Ok(packet) = self.control.try_recv() {
            drained.push(packet);
        }
        while let Ok(packet) = self.interactive.try_recv() {
            drained.push(packet);
        }
        while let Ok(packet) = self.bulk.try_recv() {
            drained.push(packet);
        }
        drained
    }
}

/// Create a node's input queue with the given per-lane capacity (see
/// [`crate::node::NodeConfig::DEFAULT_QUEUE_CAPACITY`] for the default).
#[must_use]
pub fn node_queue(capacity: usize) -> (NodeQueueSender, NodeQueueReceiver) {
    let (control_tx, control_rx) = mpsc::channel(capacity);
    let (interactive_tx, interactive_rx) = mpsc::channel(capacity);
    let (bulk_tx, bulk_rx) = mpsc::channel(capacity);
    (
        NodeQueueSender {
            control: control_tx,
            interactive: interactive_tx,
            bulk: bulk_tx,
        },
        NodeQueueReceiver {
            control: control_rx,
            interactive: interactive_rx,
            bulk: bulk_rx,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use nyarix_packet::Tag;

    fn tagged(tag: Tag) -> Packet {
        let mut packet = Packet::new(b"data".as_slice());
        packet.tag(tag);
        packet
    }

    #[tokio::test]
    async fn control_is_drained_before_interactive_and_bulk() {
        let (tx, mut rx) = node_queue(8);

        tx.send(tagged(Tag::Bulk)).await.unwrap();
        tx.send(tagged(Tag::Interactive)).await.unwrap();
        tx.send(tagged(Tag::Control)).await.unwrap();

        // Control was sent last but must come out first.
        let first = rx.recv().await.unwrap();
        assert!(first.has_tag(Tag::Control));

        let second = rx.recv().await.unwrap();
        assert!(second.has_tag(Tag::Interactive));

        let third = rx.recv().await.unwrap();
        assert!(third.has_tag(Tag::Bulk));
    }

    #[test]
    fn untagged_packet_routes_to_bulk_not_control() {
        let (tx, _rx) = node_queue(1);
        // Fill the control lane to its capacity of 1.
        tx.try_send(tagged(Tag::Control)).unwrap();
        // If an untagged packet were misrouted into the (full) control
        // lane instead of the separate bulk lane, this would fail.
        tx.try_send(Packet::new(b"data".as_slice())).unwrap();
    }

    #[test]
    fn drain_collects_pending_packets_from_all_lanes() {
        let (tx, mut rx) = node_queue(8);
        tx.try_send(tagged(Tag::Control)).unwrap();
        tx.try_send(tagged(Tag::Interactive)).unwrap();
        tx.try_send(tagged(Tag::Bulk)).unwrap();

        let drained = rx.drain();
        assert_eq!(drained.len(), 3);
        assert!(drained[0].has_tag(Tag::Control));
        assert!(drained[1].has_tag(Tag::Interactive));
        assert!(drained[2].has_tag(Tag::Bulk));
    }

    #[test]
    fn try_recv_prioritized_prefers_control_over_interactive_and_bulk() {
        let (tx, mut rx) = node_queue(8);
        tx.try_send(tagged(Tag::Bulk)).unwrap();
        tx.try_send(tagged(Tag::Interactive)).unwrap();
        tx.try_send(tagged(Tag::Control)).unwrap();

        assert!(rx.try_recv_prioritized().unwrap().has_tag(Tag::Control));
        assert!(rx.try_recv_prioritized().unwrap().has_tag(Tag::Interactive));
        assert!(rx.try_recv_prioritized().unwrap().has_tag(Tag::Bulk));
        assert!(rx.try_recv_prioritized().is_none());
    }

    #[test]
    fn len_sums_all_three_lanes() {
        let (tx, mut rx) = node_queue(8);
        assert!(rx.is_empty());
        assert_eq!(rx.len(), 0);

        tx.try_send(tagged(Tag::Control)).unwrap();
        tx.try_send(tagged(Tag::Interactive)).unwrap();
        tx.try_send(tagged(Tag::Bulk)).unwrap();
        assert_eq!(rx.len(), 3);
        assert!(!rx.is_empty());

        rx.try_recv_prioritized();
        assert_eq!(rx.len(), 2);
    }

    #[tokio::test]
    async fn recv_returns_none_once_all_lanes_closed() {
        let (tx, mut rx) = node_queue(8);
        drop(tx);
        assert!(rx.recv().await.is_none());
    }
}
