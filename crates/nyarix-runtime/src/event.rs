//! Event bus: publish/subscribe (see issue #48).

use nyarix_core::{FlowId, SessionId};
use nyarix_module_api::Health;
use tokio::sync::broadcast;

/// A lifecycle event published on the [`EventBus`].
///
/// This is the Runtime's own typed event enum — distinct from
/// [`nyarix_module_api::Event`] (#18), which is a deliberately minimal
/// placeholder (`{ name: String }`) used by `RuntimeContext::emit_event`
/// until this type existed. Routing a module's `emit_event` calls into
/// this bus is wiring work for whichever issue integrates the Module
/// Loader/Runtime with modules (#41), not this one.
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
    /// A Runtime/package/profile update is available.
    UpdateAvailable {
        /// The available version.
        version: String,
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
    /// See [`Event::UpdateAvailable`].
    UpdateAvailable,
}

impl Event {
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
            Self::UpdateAvailable { .. } => EventKind::UpdateAvailable,
        }
    }
}

/// Which events a [`EventBus::subscribe`] handler should receive.
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

        let handle = bus.subscribe(EventFilter::Only(vec![EventKind::ModuleLoaded]), move |event| {
            received_clone.lock().unwrap().push(event);
        });

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
}
