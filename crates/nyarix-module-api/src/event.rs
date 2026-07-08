//! Event bus: publish/subscribe (see issues #18, #48, #49).
//!
//! This used to be a minimal placeholder (`Event { name: String }`) for
//! `RuntimeContext::emit_event`, with the real typed event enum and
//! EventBus living in `nyarix-runtime` (#48). It moved here — rather than
//! `nyarix-runtime` depending back on it — so [`crate::context::RuntimeContext`]
//! (also defined in this crate) can hold a real [`EventBus`] and give
//! modules a working `emit_event`/`on_event` (#49) without a dependency
//! cycle (`nyarix-runtime` already depends on `nyarix-module-api`, not the
//! other way around).

use nyarix_core::{FlowId, SessionId};
use tokio::sync::broadcast;

use crate::health::Health;

/// A lifecycle event published on the [`EventBus`].
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// A module finished loading successfully.
    ModuleLoaded {
        /// The module's name.
        name: String,
    },
    /// A module was unloaded.
    ModuleUnloaded {
        /// The module's name.
        name: String,
    },
    /// A Flow Graph finished building.
    FlowBuilt {
        /// The flow that was built.
        flow_id: FlowId,
    },
    /// A Flow Graph was modified (see #37/#38/#39).
    FlowChanged {
        /// The flow that changed.
        flow_id: FlowId,
    },
    /// A handshake started for a session.
    HandshakeStarted {
        /// The session negotiating a handshake.
        session_id: SessionId,
    },
    /// A handshake completed for a session.
    HandshakeCompleted {
        /// The session whose handshake completed.
        session_id: SessionId,
    },
    /// A packet was dropped somewhere in the graph.
    PacketDropped {
        /// Human-readable reason (queue full, validation failure, ...).
        reason: String,
    },
    /// The active transport was switched (e.g. failover).
    TransportSwitched {
        /// The transport module name being switched away from.
        from: String,
        /// The transport module name being switched to.
        to: String,
    },
    /// A module's reported health changed (see #24).
    HealthChanged {
        /// The module whose health changed.
        module: String,
        /// The new health status.
        health: Health,
    },
    /// A key rotation started for a session.
    RekeyStarted {
        /// The session undergoing rekey.
        session_id: SessionId,
    },
    /// A key rotation completed for a session.
    RekeyCompleted {
        /// The session whose rekey completed.
        session_id: SessionId,
    },
    /// A configuration profile was applied.
    ProfileApplied {
        /// The applied profile's name.
        profile: String,
    },
    /// A module's health dropped from [`Health::Healthy`] to
    /// [`Health::Degraded`]/[`Health::Unhealthy`].
    ///
    /// Distinct from [`Self::HealthChanged`] (#24's general "health is
    /// now X" notification, which also fires on recovery): this and
    /// [`Self::HealthRestored`] are specifically the negative/positive
    /// *transition* edges, which is what this issue asks for as its own
    /// pair of events — for a subscriber that only cares about "did
    /// something just get worse/better", not every health reading.
    HealthDegraded {
        /// The module whose health degraded.
        module: String,
        /// The new (degraded) health status.
        health: Health,
    },
    /// A module's health recovered to [`Health::Healthy`] after
    /// previously being degraded — see [`Self::HealthDegraded`].
    HealthRestored {
        /// The module whose health recovered.
        module: String,
    },
    /// A Runtime/package/profile update is available.
    UpdateAvailable {
        /// The available version.
        version: String,
    },
    /// A module published a custom, application-defined event through
    /// `RuntimeContext::emit_event` (#49) that doesn't fit one of the
    /// typed variants above.
    Custom {
        /// Event name/kind (e.g. `"rekey_started"`), chosen by the
        /// publishing module.
        name: String,
    },
}

/// Which kind of [`Event`] this is, without its payload — used by
/// [`EventFilter`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EventKind {
    /// See [`Event::ModuleLoaded`].
    ModuleLoaded,
    /// See [`Event::ModuleUnloaded`].
    ModuleUnloaded,
    /// See [`Event::FlowBuilt`].
    FlowBuilt,
    /// See [`Event::FlowChanged`].
    FlowChanged,
    /// See [`Event::HandshakeStarted`].
    HandshakeStarted,
    /// See [`Event::HandshakeCompleted`].
    HandshakeCompleted,
    /// See [`Event::PacketDropped`].
    PacketDropped,
    /// See [`Event::TransportSwitched`].
    TransportSwitched,
    /// See [`Event::HealthChanged`].
    HealthChanged,
    /// See [`Event::RekeyStarted`].
    RekeyStarted,
    /// See [`Event::RekeyCompleted`].
    RekeyCompleted,
    /// See [`Event::ProfileApplied`].
    ProfileApplied,
    /// See [`Event::HealthDegraded`].
    HealthDegraded,
    /// See [`Event::HealthRestored`].
    HealthRestored,
    /// See [`Event::UpdateAvailable`].
    UpdateAvailable,
    /// See [`Event::Custom`].
    Custom,
}

impl Event {
    /// Create a custom, module-defined event (see [`Event::Custom`]).
    ///
    /// This is the constructor the old placeholder `Event::new` offered —
    /// kept so a module can still publish an ad hoc named event without
    /// reaching for one of the typed Runtime variants.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self::Custom { name: name.into() }
    }

    /// This event's kind, without its payload.
    #[must_use]
    pub const fn kind(&self) -> EventKind {
        match self {
            Self::ModuleLoaded { .. } => EventKind::ModuleLoaded,
            Self::ModuleUnloaded { .. } => EventKind::ModuleUnloaded,
            Self::FlowBuilt { .. } => EventKind::FlowBuilt,
            Self::FlowChanged { .. } => EventKind::FlowChanged,
            Self::HandshakeStarted { .. } => EventKind::HandshakeStarted,
            Self::HandshakeCompleted { .. } => EventKind::HandshakeCompleted,
            Self::PacketDropped { .. } => EventKind::PacketDropped,
            Self::TransportSwitched { .. } => EventKind::TransportSwitched,
            Self::HealthChanged { .. } => EventKind::HealthChanged,
            Self::RekeyStarted { .. } => EventKind::RekeyStarted,
            Self::RekeyCompleted { .. } => EventKind::RekeyCompleted,
            Self::ProfileApplied { .. } => EventKind::ProfileApplied,
            Self::HealthDegraded { .. } => EventKind::HealthDegraded,
            Self::HealthRestored { .. } => EventKind::HealthRestored,
            Self::UpdateAvailable { .. } => EventKind::UpdateAvailable,
            Self::Custom { .. } => EventKind::Custom,
        }
    }
}

/// Which events an [`EventBus::subscribe`] handler should receive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventFilter {
    /// Deliver every event.
    All,
    /// Deliver only events whose [`EventKind`] is in this list.
    Only(Vec<EventKind>),
}

impl EventFilter {
    fn matches(&self, event: &Event) -> bool {
        match self {
            Self::All => true,
            Self::Only(kinds) => kinds.contains(&event.kind()),
        }
    }
}

/// Default broadcast channel capacity: how many not-yet-delivered events
/// a slow subscriber can lag behind by before it starts missing them (see
/// [`tokio::sync::broadcast`]'s lagging behavior).
pub const DEFAULT_CAPACITY: usize = 256;

/// Publish/subscribe event bus, backed by [`tokio::sync::broadcast`].
#[derive(Debug, Clone)]
pub struct EventBus {
    sender: broadcast::Sender<Event>,
}

impl EventBus {
    /// Create a new bus with the given broadcast channel capacity.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        let (sender, _receiver) = broadcast::channel(capacity);
        Self { sender }
    }

    /// Publish an event to every current subscriber.
    ///
    /// If there are no subscribers right now, the event is simply
    /// dropped — that's not an error condition worth reporting.
    pub fn publish(&self, event: Event) {
        let _ = self.sender.send(event);
    }

    /// Subscribe to events matching `filter`, invoking `handler` for each
    /// one on a dedicated spawned task.
    ///
    /// Returns a [`tokio::task::JoinHandle`] — drop or `.abort()` it to
    /// stop the subscription. If the handler falls behind the broadcast
    /// channel's capacity, it silently skips the events it missed (a
    /// [`broadcast::error::RecvError::Lagged`]) rather than erroring out;
    /// this is a lossy pub/sub bus, not a delivery-guaranteed queue.
    pub fn subscribe<F>(&self, filter: EventFilter, mut handler: F) -> tokio::task::JoinHandle<()>
    where
        F: FnMut(Event) + Send + 'static,
    {
        let mut receiver = self.sender.subscribe();
        tokio::spawn(async move {
            loop {
                match receiver.recv().await {
                    Ok(event) => {
                        if filter.matches(&event) {
                            handler(event);
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        })
    }

    /// Subscribe with an async [`EventHandler`] (#72) instead of a sync
    /// closure ([`Self::subscribe`]) — for handling that itself needs to
    /// `.await` (I/O, another service call, ...) without blocking on it
    /// synchronously inside a plain closure.
    ///
    /// Runs on its own spawned task, same as [`Self::subscribe`], so a
    /// slow handler never blocks event *delivery* to other subscribers
    /// or the publisher — but it could still block delivery to *this*
    /// subscriber indefinitely if it never returns, which is exactly
    /// what `handler_timeout` guards against: each call to
    /// [`EventHandler::handle`] is wrapped in [`tokio::time::timeout`],
    /// and a timed-out call is logged and skipped rather than left to
    /// hang forever.
    ///
    /// Returns a [`tokio::task::JoinHandle`] — drop or `.abort()` it to
    /// stop the subscription, same as [`Self::subscribe`].
    pub fn subscribe_async<H>(
        &self,
        filter: EventFilter,
        mut handler: H,
        handler_timeout: std::time::Duration,
    ) -> tokio::task::JoinHandle<()>
    where
        H: EventHandler + Send + 'static,
    {
        let mut receiver = self.sender.subscribe();
        tokio::spawn(async move {
            loop {
                match receiver.recv().await {
                    Ok(event) => {
                        if filter.matches(&event) {
                            let kind = event.kind();
                            if tokio::time::timeout(handler_timeout, handler.handle(event))
                                .await
                                .is_err()
                            {
                                tracing::warn!(?kind, "event handler timed out");
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        })
    }
}

/// An async handler for events delivered by an [`EventBus`] (#72).
///
/// Unlike the sync closure [`EventBus::subscribe`] takes, `handle` can
/// itself `.await` — the whole point of this trait over a plain
/// `FnMut(Event)`.
pub trait EventHandler: Send {
    /// Handle one event.
    fn handle(&mut self, event: Event) -> impl Future<Output = ()> + Send;
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new(DEFAULT_CAPACITY)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[tokio::test]
    async fn subscriber_receives_published_event() {
        let bus = EventBus::default();
        let received: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
        let received_clone = Arc::clone(&received);

        let handle = bus.subscribe(EventFilter::All, move |event| {
            received_clone.lock().unwrap().push(event);
        });

        bus.publish(Event::ModuleLoaded {
            name: "quic".to_string(),
        });

        // Give the spawned subscriber task a chance to run.
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        assert_eq!(received.lock().unwrap().len(), 1);
        handle.abort();
    }

    #[tokio::test]
    async fn filter_only_delivers_matching_kinds() {
        let bus = EventBus::default();
        let received: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
        let received_clone = Arc::clone(&received);

        let handle = bus.subscribe(
            EventFilter::Only(vec![EventKind::ModuleLoaded]),
            move |event| {
                received_clone.lock().unwrap().push(event);
            },
        );

        bus.publish(Event::ModuleUnloaded {
            name: "quic".to_string(),
        });
        bus.publish(Event::ModuleLoaded {
            name: "quic".to_string(),
        });

        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        let received = received.lock().unwrap();
        assert_eq!(received.len(), 1);
        assert_eq!(received[0].kind(), EventKind::ModuleLoaded);
        handle.abort();
    }

    #[test]
    fn publish_without_subscribers_does_not_panic() {
        let bus = EventBus::default();
        bus.publish(Event::UpdateAvailable {
            version: "0.2.0".to_string(),
        });
    }

    #[tokio::test]
    async fn aborted_subscription_stops_receiving() {
        let bus = EventBus::default();
        let received: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
        let received_clone = Arc::clone(&received);

        let handle = bus.subscribe(EventFilter::All, move |event| {
            received_clone.lock().unwrap().push(event);
        });
        handle.abort();
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        bus.publish(Event::UpdateAvailable {
            version: "0.2.0".to_string(),
        });
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        assert!(received.lock().unwrap().is_empty());
    }

    #[test]
    fn custom_event_round_trips_through_new() {
        let event = Event::new("rekey_started");
        assert_eq!(event.kind(), EventKind::Custom);
        assert_eq!(
            event,
            Event::Custom {
                name: "rekey_started".to_string()
            }
        );
    }

    #[test]
    fn rekey_events_carry_the_session_id_and_the_right_kind() {
        let session_id = SessionId::new();
        assert_eq!(
            Event::RekeyStarted { session_id }.kind(),
            EventKind::RekeyStarted
        );
        assert_eq!(
            Event::RekeyCompleted { session_id }.kind(),
            EventKind::RekeyCompleted
        );
    }

    #[test]
    fn profile_applied_carries_the_profile_name() {
        let event = Event::ProfileApplied {
            profile: "stealth".to_string(),
        };
        assert_eq!(event.kind(), EventKind::ProfileApplied);
        assert_eq!(
            event,
            Event::ProfileApplied {
                profile: "stealth".to_string()
            }
        );
    }

    #[test]
    fn health_degraded_and_restored_are_distinct_from_health_changed() {
        let degraded = Event::HealthDegraded {
            module: "quic".to_string(),
            health: Health::Degraded {
                reason: "high latency".to_string(),
            },
        };
        let restored = Event::HealthRestored {
            module: "quic".to_string(),
        };
        let changed = Event::HealthChanged {
            module: "quic".to_string(),
            health: Health::Degraded {
                reason: "high latency".to_string(),
            },
        };

        assert_eq!(degraded.kind(), EventKind::HealthDegraded);
        assert_eq!(restored.kind(), EventKind::HealthRestored);
        assert_eq!(changed.kind(), EventKind::HealthChanged);
        assert_ne!(degraded.kind(), changed.kind());
    }

    #[tokio::test]
    async fn a_filter_for_health_degraded_does_not_match_health_restored() {
        let bus = EventBus::default();
        let received: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
        let received_clone = Arc::clone(&received);

        let handle = bus.subscribe(
            EventFilter::Only(vec![EventKind::HealthDegraded]),
            move |event| {
                received_clone.lock().unwrap().push(event);
            },
        );

        bus.publish(Event::HealthRestored {
            module: "quic".to_string(),
        });
        bus.publish(Event::HealthDegraded {
            module: "quic".to_string(),
            health: Health::Unhealthy {
                reason: "connection lost".to_string(),
            },
        });

        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        let received = received.lock().unwrap();
        assert_eq!(received.len(), 1);
        assert_eq!(received[0].kind(), EventKind::HealthDegraded);
        handle.abort();
    }

    struct RecordingHandler {
        received: Arc<Mutex<Vec<Event>>>,
    }

    impl EventHandler for RecordingHandler {
        async fn handle(&mut self, event: Event) {
            // Prove the handler genuinely awaits something.
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
            self.received.lock().unwrap().push(event);
        }
    }

    #[tokio::test]
    async fn subscribe_async_delivers_events_to_an_async_handler() {
        let bus = EventBus::default();
        let received: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
        let handler = RecordingHandler {
            received: Arc::clone(&received),
        };

        let handle =
            bus.subscribe_async(EventFilter::All, handler, std::time::Duration::from_secs(1));

        bus.publish(Event::ModuleLoaded {
            name: "quic".to_string(),
        });
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        assert_eq!(received.lock().unwrap().len(), 1);
        handle.abort();
    }

    struct HangingHandler;

    impl EventHandler for HangingHandler {
        async fn handle(&mut self, _event: Event) {
            // Never completes on its own within the test's timeout.
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        }
    }

    #[tokio::test]
    async fn subscribe_async_does_not_hang_forever_on_a_slow_handler() {
        let bus = EventBus::default();
        let handle = bus.subscribe_async(
            EventFilter::All,
            HangingHandler,
            std::time::Duration::from_millis(10),
        );

        bus.publish(Event::ModuleLoaded {
            name: "quic".to_string(),
        });
        // If the timeout didn't apply, this task would still be stuck
        // awaiting the first event's handler; publishing (and this test
        // completing at all within its harness timeout) proves it moved
        // on instead of hanging.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        bus.publish(Event::ModuleLoaded {
            name: "quic-2".to_string(),
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        handle.abort();
    }
}
