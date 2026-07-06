//! Event type for `RuntimeContext::emit_event` (see issue #18).
//!
//! A minimal placeholder — the full EventBus with typed lifecycle events
//! (`ModuleLoaded`, `FlowBuilt`, `HandshakeStarted`, `PacketDropped`, ...,
//! see the platform vision doc §22) is M4 (Runtime Core, EventBus/publish
//! subscribe issues).

/// An event a module can publish through `RuntimeContext::emit_event`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    /// Event name/kind (e.g. `"rekey_started"`).
    ///
    /// A plain string for now; once the EventBus (M4) defines its typed
    /// event enum, this will likely become (or wrap) that type instead.
    pub name: String,
}

impl Event {
    /// Create a new named event.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }
}
