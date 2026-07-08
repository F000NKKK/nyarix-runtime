//! Live-graph pause/resume coordination (see issue #98).
//!
//! [`nyarix_graph::FlowGraph::insert_after`]/`remove_and_reconnect`/
//! `swap_node` (#37/#38) are pure topology mutations — they know nothing
//! about a live [`crate::execution_loop::run`] that might be pulling
//! packets through the very graph being mutated. This module is the
//! missing coordination: a [`GraphPauseHandle`] lets a caller (whoever is
//! about to call one of those mutation methods) tell the execution loop
//! to stop accepting new packets, then resume it once the mutation is
//! done.
//!
//! **Contract**: `insert_after`/`remove_and_reconnect`/`swap_node` must
//! only be called while the graph's execution loop is paused — pausing
//! doesn't itself lock anything (the `FlowGraph` is still behind the
//! caller's own `Mutex`, same as always), it only stops the loop from
//! *starting* new work, so packets already past `source.recv()` and
//! in-flight through [`nyarix_graph::execute_sequential`]/`execute_parallel`
//! at the moment of a mutation can still race with it. A caller wanting a
//! hard guarantee should pause, then briefly take the graph's `Mutex`
//! itself before mutating (the loop can't be mid-`execute_*` while it
//! doesn't hold the lock).
//!
//! **Not implemented here** (per #98's own scope note): migrating
//! per-session/per-flow state across a [`nyarix_graph::FlowGraph::diff`]-driven
//! rebuild (#39) — there's no Session/Flow state model yet to migrate;
//! tracked as follow-up work once that lands.
//!
//! Backed by a `tokio::sync::watch<bool>` rather than a `Mutex`/`Notify`:
//! every watcher always observes the latest state (no missed
//! notifications between a `pause()` and a watcher starting to look), and
//! `is_paused()` is a cheap synchronous read.

use tokio::sync::watch;

/// The controlling half — held by whoever performs graph mutations.
#[derive(Debug, Clone)]
pub struct GraphPauseHandle {
    tx: watch::Sender<bool>,
}

impl GraphPauseHandle {
    /// Create a new pause/resume pair, starting in the resumed (not
    /// paused) state.
    #[must_use]
    pub fn new() -> (Self, GraphPauseWatcher) {
        let (tx, rx) = watch::channel(false);
        (Self { tx }, GraphPauseWatcher { rx })
    }

    /// Signal every watcher to stop accepting new packets.
    ///
    /// Doesn't wait for the execution loop to actually reach a paused
    /// state — see the module docs for the weaker guarantee this gives,
    /// and [`GraphPauseWatcher::is_paused`] for a caller that needs to
    /// poll for it before mutating.
    pub fn pause(&self) {
        let _ = self.tx.send(true);
    }

    /// Signal every watcher it's safe to resume pulling packets.
    pub fn resume(&self) {
        let _ = self.tx.send(false);
    }

    /// Whether this handle currently considers the graph paused.
    #[must_use]
    pub fn is_paused(&self) -> bool {
        *self.tx.borrow()
    }
}

/// The observing half — held by the execution loop.
#[derive(Debug, Clone)]
pub struct GraphPauseWatcher {
    rx: watch::Receiver<bool>,
}

impl GraphPauseWatcher {
    /// A watcher that never pauses — for callers (tests, simple
    /// embeddings) that don't need mid-run graph mutation and just want
    /// [`crate::execution_loop::run`] to behave as if #98 didn't exist.
    #[must_use]
    pub fn always_resumed() -> Self {
        GraphPauseHandle::new().1
    }

    /// Whether the graph is currently paused.
    #[must_use]
    pub fn is_paused(&self) -> bool {
        *self.rx.borrow()
    }

    /// Resolve once the graph is resumed (immediately, if it already is).
    pub async fn wait_until_resumed(&mut self) {
        while *self.rx.borrow() {
            if self.rx.changed().await.is_err() {
                // Every `GraphPauseHandle` was dropped. Nothing will ever
                // flip this again, and hanging forever on a resume that
                // can't come would strand the loop — treat a dropped
                // handle as an implicit permanent resume instead.
                return;
            }
        }
    }

    /// Resolve once the graph is paused (immediately, if it already is).
    ///
    /// Used to interrupt an in-progress wait for the next packet as soon
    /// as a pause is requested, rather than only noticing it after that
    /// packet finishes processing.
    pub async fn wait_until_paused(&mut self) {
        loop {
            if *self.rx.borrow() {
                return;
            }
            if self.rx.changed().await.is_err() {
                // No handle can ever pause this again; disable this
                // branch permanently rather than busy-looping.
                std::future::pending::<()>().await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn starts_resumed_and_wait_until_resumed_returns_immediately() {
        let (_handle, mut watcher) = GraphPauseHandle::new();
        assert!(!watcher.is_paused());
        watcher.wait_until_resumed().await;
    }

    #[tokio::test]
    async fn pause_then_resume_unblocks_a_waiter() {
        let (handle, mut watcher) = GraphPauseHandle::new();
        handle.pause();
        assert!(watcher.is_paused());

        let mut waiter = watcher.clone();
        let waited = tokio::spawn(async move {
            waiter.wait_until_resumed().await;
        });

        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert!(!waited.is_finished());

        handle.resume();
        waited.await.unwrap();
        assert!(!watcher.is_paused());
    }

    #[tokio::test]
    async fn wait_until_paused_resolves_as_soon_as_paused_is_signalled() {
        let (handle, mut watcher) = GraphPauseHandle::new();
        let mut waiter = watcher.clone();
        let waited = tokio::spawn(async move {
            waiter.wait_until_paused().await;
        });

        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert!(!waited.is_finished());

        handle.pause();
        waited.await.unwrap();
        assert!(watcher.is_paused());
    }

    #[tokio::test]
    async fn dropping_every_handle_releases_a_waiter_instead_of_hanging() {
        let (handle, mut watcher) = GraphPauseHandle::new();
        handle.pause();
        drop(handle);

        // Should return promptly (implicit resume) rather than hang
        // forever waiting for a resume that can no longer be sent.
        tokio::time::timeout(std::time::Duration::from_secs(1), watcher.wait_until_resumed())
            .await
            .expect("wait_until_resumed must not hang once every handle is dropped");
    }

    #[test]
    fn always_resumed_reports_not_paused() {
        assert!(!GraphPauseWatcher::always_resumed().is_paused());
    }
}
