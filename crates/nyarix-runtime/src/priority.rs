//! Task priorities for the Scheduler (see issue #47).
//!
//! [`TaskPriority`] is the ordering the issue specifies; [`priority_queue`]
//! builds a generic, 5-lane, `biased`-`select`-backed queue for handing
//! out arbitrary work items in that order — the same pattern
//! [`nyarix_graph`]'s per-node `NodeQueue` (#36) already uses for packets,
//! generalized from 3 lanes to the 5 this issue asks for, and from
//! `Packet` specifically to any `T`.
//!
//! **Scope note:** this is the priority *data model* and the *queue
//! primitive*, both real and tested. It is not yet wired into
//! [`crate::io_pool::IoPool`] or [`crate::cpu_pool::CpuPool`] as their
//! submission order — doing that usefully needs a driving loop that
//! actually has competing work to reorder, and neither pool has real
//! consumers yet (#100, #101 track that). Tracked in #102.

use nyarix_packet::Tags;
use tokio::sync::mpsc;

/// Scheduling priority for a unit of runtime work.
///
/// Ordered highest to lowest: variant declaration order is priority
/// order, and `derive(PartialOrd, Ord)` follows it, so
/// `TaskPriority::Realtime < TaskPriority::Background` is `true` —
/// read as "sorts before", i.e. "runs first".
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TaskPriority {
    /// Handshake, rekey, control packets.
    Realtime,
    /// Interactive traffic.
    High,
    /// Bulk data.
    Normal,
    /// Metrics export, update checks.
    Low,
    /// Diagnostics, cleanup.
    Background,
}

impl TaskPriority {
    /// Classify a packet's priority from its [`Tags`] (#10), per the
    /// mapping this issue specifies for packet-carrying work:
    /// handshake/rekey/control is [`Self::Realtime`], interactive traffic
    /// is [`Self::High`], everything else (including bulk and untagged
    /// packets) is [`Self::Normal`].
    ///
    /// [`Self::Low`] and [`Self::Background`] have no packet-tag
    /// equivalent in the issue's mapping — they're for non-packet
    /// maintenance work (metrics export, update checks, diagnostics,
    /// cleanup) that a caller assigns directly, not derived from `Tags`.
    #[must_use]
    pub fn from_tags(tags: Tags) -> Self {
        if tags.intersects(Tags::CONTROL | Tags::HANDSHAKE | Tags::REKEY) {
            Self::Realtime
        } else if tags.contains(Tags::INTERACTIVE) {
            Self::High
        } else {
            Self::Normal
        }
    }
}

/// The sending half of a queue built by [`priority_queue`].
///
/// Cloneable: multiple producers can feed the same 5 lanes.
pub struct PrioritySender<T> {
    lanes: [mpsc::Sender<T>; 5],
}

impl<T> Clone for PrioritySender<T> {
    fn clone(&self) -> Self {
        Self {
            lanes: self.lanes.clone(),
        }
    }
}

impl<T> PrioritySender<T> {
    const fn lane(&self, priority: TaskPriority) -> &mpsc::Sender<T> {
        &self.lanes[priority as usize]
    }

    /// Send an item at the given priority, waiting for space in its lane
    /// if needed.
    ///
    /// # Errors
    /// Returns the item back if that lane's receiver has been dropped.
    pub async fn send(
        &self,
        priority: TaskPriority,
        item: T,
    ) -> Result<(), mpsc::error::SendError<T>> {
        self.lane(priority).send(item).await
    }

    /// Attempt to send an item without waiting for lane space.
    ///
    /// # Errors
    /// Returns the item back if its lane is full or its receiver has
    /// been dropped.
    pub fn try_send(
        &self,
        priority: TaskPriority,
        item: T,
    ) -> Result<(), mpsc::error::TrySendError<T>> {
        self.lane(priority).try_send(item)
    }
}

/// The receiving half of a queue built by [`priority_queue`].
pub struct PriorityReceiver<T> {
    lanes: [mpsc::Receiver<T>; 5],
}

impl<T> PriorityReceiver<T> {
    /// Receive the next item, preferring [`TaskPriority::Realtime`] over
    /// `High` over `Normal` over `Low` over [`TaskPriority::Background`]
    /// (a `biased` `tokio::select!` — checked in that order every time,
    /// not fair-random like a plain `select!`).
    ///
    /// Returns `None` once every lane's sender half has been dropped and
    /// drained.
    pub async fn recv(&mut self) -> Option<T> {
        let [realtime, high, normal, low, background] = &mut self.lanes;
        tokio::select! {
            biased;
            item = realtime.recv() => item,
            item = high.recv() => item,
            item = normal.recv() => item,
            item = low.recv() => item,
            item = background.recv() => item,
        }
    }

    /// Drain every currently-queued item across all 5 lanes, in priority
    /// order, without waiting for more to arrive.
    pub fn drain(&mut self) -> Vec<T> {
        let mut drained = Vec::new();
        for lane in &mut self.lanes {
            while let Ok(item) = lane.try_recv() {
                drained.push(item);
            }
        }
        drained
    }
}

/// Create a priority queue with the given per-lane capacity.
#[must_use]
pub fn priority_queue<T>(capacity: usize) -> (PrioritySender<T>, PriorityReceiver<T>) {
    let (realtime_tx, realtime_rx) = mpsc::channel(capacity);
    let (high_tx, high_rx) = mpsc::channel(capacity);
    let (normal_tx, normal_rx) = mpsc::channel(capacity);
    let (low_tx, low_rx) = mpsc::channel(capacity);
    let (background_tx, background_rx) = mpsc::channel(capacity);
    (
        PrioritySender {
            lanes: [realtime_tx, high_tx, normal_tx, low_tx, background_tx],
        },
        PriorityReceiver {
            lanes: [realtime_rx, high_rx, normal_rx, low_rx, background_rx],
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn priority_ordering_runs_first_sorts_lowest() {
        assert!(TaskPriority::Realtime < TaskPriority::High);
        assert!(TaskPriority::High < TaskPriority::Normal);
        assert!(TaskPriority::Normal < TaskPriority::Low);
        assert!(TaskPriority::Low < TaskPriority::Background);
    }

    #[test]
    fn from_tags_maps_control_handshake_rekey_to_realtime() {
        assert_eq!(
            TaskPriority::from_tags(Tags::CONTROL),
            TaskPriority::Realtime
        );
        assert_eq!(
            TaskPriority::from_tags(Tags::HANDSHAKE),
            TaskPriority::Realtime
        );
        assert_eq!(TaskPriority::from_tags(Tags::REKEY), TaskPriority::Realtime);
    }

    #[test]
    fn from_tags_maps_interactive_to_high() {
        assert_eq!(
            TaskPriority::from_tags(Tags::INTERACTIVE),
            TaskPriority::High
        );
    }

    #[test]
    fn from_tags_maps_bulk_and_untagged_to_normal() {
        assert_eq!(TaskPriority::from_tags(Tags::BULK), TaskPriority::Normal);
        assert_eq!(TaskPriority::from_tags(Tags::empty()), TaskPriority::Normal);
    }

    #[test]
    fn control_overrides_interactive_if_both_set() {
        assert_eq!(
            TaskPriority::from_tags(Tags::CONTROL | Tags::INTERACTIVE),
            TaskPriority::Realtime
        );
    }

    #[tokio::test]
    async fn realtime_is_drained_before_lower_priorities() {
        let (tx, mut rx) = priority_queue(8);
        tx.send(TaskPriority::Background, "cleanup").await.unwrap();
        tx.send(TaskPriority::Low, "metrics").await.unwrap();
        tx.send(TaskPriority::Normal, "bulk").await.unwrap();
        tx.send(TaskPriority::High, "interactive").await.unwrap();
        tx.send(TaskPriority::Realtime, "handshake").await.unwrap();

        assert_eq!(rx.recv().await, Some("handshake"));
        assert_eq!(rx.recv().await, Some("interactive"));
        assert_eq!(rx.recv().await, Some("bulk"));
        assert_eq!(rx.recv().await, Some("metrics"));
        assert_eq!(rx.recv().await, Some("cleanup"));
    }

    #[test]
    fn drain_collects_pending_items_in_priority_order() {
        let (tx, mut rx) = priority_queue(8);
        tx.try_send(TaskPriority::Low, "metrics").unwrap();
        tx.try_send(TaskPriority::Realtime, "handshake").unwrap();
        tx.try_send(TaskPriority::Normal, "bulk").unwrap();

        let drained = rx.drain();
        assert_eq!(drained, vec!["handshake", "bulk", "metrics"]);
    }

    #[tokio::test]
    async fn recv_returns_none_once_all_lanes_closed() {
        let (tx, mut rx) = priority_queue::<()>(8);
        drop(tx);
        assert!(rx.recv().await.is_none());
    }
}
