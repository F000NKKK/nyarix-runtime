//! Runtime-to-module context (see issue #18).

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex};

use tokio::task::JoinHandle;

use crate::config::ModuleConfig;
use crate::event::{Event, EventBus, EventFilter, EventHandler};
use crate::metrics::MetricsHandle;
use crate::module::Module;
use crate::platform::Platform;
use crate::sandbox::SandboxHandle;

/// Context handed to a module by the Runtime during its lifecycle.
pub struct RuntimeContext {
    config: ModuleConfig,
    metrics: MetricsHandle,
    sandbox: SandboxHandle,
    platform: Platform,
    dependencies: HashMap<String, Arc<dyn Module>>,
    event_bus: Option<Arc<EventBus>>,
    subscriptions: Mutex<Vec<JoinHandle<()>>>,
}

impl RuntimeContext {
    /// Create an empty context for the current platform: no config, no
    /// resolvable dependencies, no [`EventBus`]. Suitable for unit tests
    /// and as a stand-in until the Runtime (M4) builds real ones.
    ///
    /// `emit_event`/`on_event` still work without a panic in this case —
    /// they just have no bus to publish to or subscribe on (see their
    /// docs).
    #[must_use]
    pub fn empty() -> Self {
        Self {
            config: ModuleConfig::empty(),
            metrics: MetricsHandle::default(),
            sandbox: SandboxHandle::default(),
            platform: Platform::current(),
            dependencies: HashMap::new(),
            event_bus: None,
            subscriptions: Mutex::new(Vec::new()),
        }
    }

    /// Build a context with the given config and resolvable dependencies,
    /// but no [`EventBus`] (see [`Self::with_event_bus`] for one that has
    /// one).
    #[must_use]
    pub fn new(config: ModuleConfig, dependencies: HashMap<String, Arc<dyn Module>>) -> Self {
        Self {
            config,
            metrics: MetricsHandle::default(),
            sandbox: SandboxHandle::default(),
            platform: Platform::current(),
            dependencies,
            event_bus: None,
            subscriptions: Mutex::new(Vec::new()),
        }
    }

    /// Build a context with the given config, resolvable dependencies,
    /// and a live [`EventBus`] — what the Runtime hands modules once it
    /// actually has one running (#48).
    #[must_use]
    pub fn with_event_bus(
        config: ModuleConfig,
        dependencies: HashMap<String, Arc<dyn Module>>,
        event_bus: Arc<EventBus>,
    ) -> Self {
        Self {
            config,
            metrics: MetricsHandle::default(),
            sandbox: SandboxHandle::default(),
            platform: Platform::current(),
            dependencies,
            event_bus: Some(event_bus),
            subscriptions: Mutex::new(Vec::new()),
        }
    }

    /// This module's configuration.
    #[must_use]
    pub fn config(&self) -> &ModuleConfig {
        &self.config
    }

    /// Handle for recording metrics.
    ///
    /// Currently a no-op placeholder — see [`MetricsHandle`] docs (M8).
    #[must_use]
    pub fn metrics(&self) -> &MetricsHandle {
        &self.metrics
    }

    /// Publish an event (#49).
    ///
    /// If this context has no [`EventBus`] attached (e.g. it was built
    /// with [`Self::empty`] or [`Self::new`] rather than
    /// [`Self::with_event_bus`]), the event is only traced so it isn't
    /// silently swallowed.
    pub fn emit_event(&self, event: Event) {
        match &self.event_bus {
            Some(bus) => bus.publish(event),
            None => tracing::debug!(?event, "event emitted (no EventBus attached)"),
        }
    }

    /// Subscribe to events matching `filter` for the lifetime of this
    /// context (#49).
    ///
    /// A module typically calls this from [`crate::module::Module::initialize`],
    /// which receives `&RuntimeContext`. The subscription runs on its own
    /// spawned task (see [`EventBus::subscribe`]); this context tracks the
    /// returned [`JoinHandle`] and aborts it in [`Self::cancel_subscriptions`],
    /// which the Runtime calls right after [`crate::module::Module::shutdown`]
    /// — giving the "automatic unsubscription at shutdown" the issue asks
    /// for without relying on `Drop` timing.
    ///
    /// Returns `false` (and subscribes nothing) if this context has no
    /// [`EventBus`] attached.
    pub fn on_event<F>(&self, filter: EventFilter, handler: F) -> bool
    where
        F: FnMut(Event) + Send + 'static,
    {
        let Some(bus) = &self.event_bus else {
            tracing::debug!("on_event called with no EventBus attached; ignoring subscription");
            return false;
        };
        let handle = bus.subscribe(filter, handler);
        self.subscriptions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(handle);
        true
    }

    /// Subscribe with an async [`EventHandler`] (#72) instead of a sync
    /// closure — see [`EventBus::subscribe_async`] for what
    /// `handler_timeout` guards against. Tracked in the same
    /// subscription list as [`Self::on_event`], so
    /// [`Self::cancel_subscriptions`] aborts both kinds uniformly.
    ///
    /// Returns `false` (and subscribes nothing) if this context has no
    /// [`EventBus`] attached, same as [`Self::on_event`].
    pub fn on_event_async<H>(
        &self,
        filter: EventFilter,
        handler: H,
        handler_timeout: std::time::Duration,
    ) -> bool
    where
        H: EventHandler + Send + 'static,
    {
        let Some(bus) = &self.event_bus else {
            tracing::debug!(
                "on_event_async called with no EventBus attached; ignoring subscription"
            );
            return false;
        };
        let handle = bus.subscribe_async(filter, handler, handler_timeout);
        self.subscriptions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(handle);
        true
    }

    /// Abort every subscription registered through [`Self::on_event`] or
    /// [`Self::on_event_async`] on this context.
    ///
    /// The Runtime calls this after a module's `shutdown()` completes, so
    /// a module doesn't need to unsubscribe by hand — see [`Self::on_event`].
    pub fn cancel_subscriptions(&self) {
        let mut subscriptions = self
            .subscriptions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for handle in subscriptions.drain(..) {
            handle.abort();
        }
    }

    /// Look up another module this module depends on, if the Runtime
    /// resolved and granted access to it.
    ///
    /// Note: `Arc<dyn Module>` gives shared *read* access only — `Module`'s
    /// `initialize`/`process`/`shutdown` take `&mut self`, so actually
    /// invoking them through a resolved dependency needs an interior
    /// mutability strategy (e.g. `Arc<Mutex<dyn Module>>`) that the Module
    /// Loader (M5) hasn't settled on yet. The signature here matches #18's
    /// spec as written; revisit if M5 lands on a different sharing shape.
    #[must_use]
    pub fn resolve_dependency(&self, name: &str) -> Option<Arc<dyn Module>> {
        self.dependencies.get(name).cloned()
    }

    /// Whether a dependency named `name` was resolved and is available.
    ///
    /// This is how a module checks an *optional* dependency's presence
    /// (#56) without treating its absence as an error — an optional
    /// dependency that isn't installed is simply not granted here, not
    /// a failed [`Self::resolve_dependency`] call worth logging.
    #[must_use]
    pub fn has_module(&self, name: &str) -> bool {
        self.dependencies.contains_key(name)
    }

    /// The platform the Runtime is currently executing on.
    #[must_use]
    pub const fn platform(&self) -> Platform {
        self.platform
    }

    /// Handle for sandbox interaction.
    ///
    /// Currently a no-op placeholder — see [`SandboxHandle`] docs (M7).
    #[must_use]
    pub fn sandbox(&self) -> &SandboxHandle {
        &self.sandbox
    }
}

impl fmt::Debug for RuntimeContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RuntimeContext")
            .field("config", &self.config)
            .field("platform", &self.platform)
            .field(
                "dependencies",
                &self.dependencies.keys().collect::<Vec<_>>(),
            )
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use support::StubModule;

    mod support {
        use crate::context::RuntimeContext;
        use crate::metadata::ModuleMetadata;
        use crate::metadata::ModuleType;
        use crate::module::{Module, Result};
        use nyarix_packet::Packet;

        pub struct StubModule {
            pub metadata: ModuleMetadata,
        }

        impl Module for StubModule {
            fn metadata(&self) -> &ModuleMetadata {
                &self.metadata
            }

            fn initialize(&mut self, _ctx: &RuntimeContext) -> Result<()> {
                Ok(())
            }

            fn process(&mut self, packet: Packet) -> Result<Option<Packet>> {
                Ok(Some(packet))
            }

            fn shutdown(&mut self, _ctx: &RuntimeContext) -> Result<()> {
                Ok(())
            }
        }

        pub fn new_metadata(name: &str) -> ModuleMetadata {
            ModuleMetadata::new(name, semver::Version::new(0, 1, 0), ModuleType::Flow)
        }
    }

    #[test]
    fn empty_context_has_no_dependencies() {
        let ctx = RuntimeContext::empty();
        assert!(ctx.resolve_dependency("anything").is_none());
    }

    #[test]
    fn resolves_registered_dependency() {
        let dep: Arc<dyn Module> = Arc::new(StubModule {
            metadata: support::new_metadata("dns-resolver"),
        });
        let mut deps: HashMap<String, Arc<dyn Module>> = HashMap::new();
        deps.insert("dns-resolver".to_string(), dep);

        let ctx = RuntimeContext::new(ModuleConfig::empty(), deps);
        let resolved = ctx.resolve_dependency("dns-resolver");
        assert!(resolved.is_some());
        assert_eq!(resolved.unwrap().metadata().name, "dns-resolver");
        assert!(ctx.resolve_dependency("missing").is_none());
    }

    #[test]
    fn has_module_reflects_resolved_dependencies() {
        let dep: Arc<dyn Module> = Arc::new(StubModule {
            metadata: support::new_metadata("dns-resolver"),
        });
        let mut deps: HashMap<String, Arc<dyn Module>> = HashMap::new();
        deps.insert("dns-resolver".to_string(), dep);

        let ctx = RuntimeContext::new(ModuleConfig::empty(), deps);
        assert!(ctx.has_module("dns-resolver"));
        assert!(!ctx.has_module("nonexistent-optional-plugin"));
    }

    #[test]
    fn platform_matches_current_target() {
        let ctx = RuntimeContext::empty();
        assert_eq!(ctx.platform(), Platform::current());
    }

    #[test]
    fn emit_event_does_not_panic_without_a_bus() {
        let ctx = RuntimeContext::empty();
        ctx.emit_event(Event::new("test_event"));
    }

    #[test]
    fn on_event_returns_false_without_a_bus() {
        let ctx = RuntimeContext::empty();
        assert!(!ctx.on_event(EventFilter::All, |_| {}));
    }

    #[tokio::test]
    async fn emit_event_reaches_a_subscriber_through_the_attached_bus() {
        let bus = Arc::new(EventBus::default());
        let ctx = RuntimeContext::with_event_bus(ModuleConfig::empty(), HashMap::new(), bus);

        let received: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
        let received_clone = Arc::clone(&received);
        let subscribed = ctx.on_event(EventFilter::All, move |event| {
            received_clone.lock().unwrap().push(event);
        });
        assert!(subscribed);

        ctx.emit_event(Event::new("rekey_started"));
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        assert_eq!(received.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn cancel_subscriptions_stops_delivery() {
        let bus = Arc::new(EventBus::default());
        let ctx = RuntimeContext::with_event_bus(ModuleConfig::empty(), HashMap::new(), bus);

        let received: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
        let received_clone = Arc::clone(&received);
        ctx.on_event(EventFilter::All, move |event| {
            received_clone.lock().unwrap().push(event);
        });

        ctx.cancel_subscriptions();
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        ctx.emit_event(Event::new("after_cancel"));
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        assert!(received.lock().unwrap().is_empty());
    }

    struct RecordingHandler {
        received: Arc<Mutex<Vec<Event>>>,
    }

    impl EventHandler for RecordingHandler {
        fn handle(&mut self, event: Event) -> impl Future<Output = ()> + Send {
            let received = Arc::clone(&self.received);
            async move {
                tokio::time::sleep(std::time::Duration::from_millis(1)).await;
                received.lock().unwrap().push(event);
            }
        }
    }

    #[test]
    fn on_event_async_returns_false_without_a_bus() {
        let ctx = RuntimeContext::empty();
        let handler = RecordingHandler {
            received: Arc::new(Mutex::new(Vec::new())),
        };
        assert!(!ctx.on_event_async(EventFilter::All, handler, std::time::Duration::from_secs(1)));
    }

    #[tokio::test]
    async fn on_event_async_reaches_a_subscriber_through_the_attached_bus() {
        let bus = Arc::new(EventBus::default());
        let ctx = RuntimeContext::with_event_bus(ModuleConfig::empty(), HashMap::new(), bus);

        let received: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
        let handler = RecordingHandler {
            received: Arc::clone(&received),
        };
        let subscribed =
            ctx.on_event_async(EventFilter::All, handler, std::time::Duration::from_secs(1));
        assert!(subscribed);

        ctx.emit_event(Event::new("rekey_started"));
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        assert_eq!(received.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn cancel_subscriptions_stops_delivery_to_async_handlers_too() {
        let bus = Arc::new(EventBus::default());
        let ctx = RuntimeContext::with_event_bus(ModuleConfig::empty(), HashMap::new(), bus);

        let received: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
        let handler = RecordingHandler {
            received: Arc::clone(&received),
        };
        ctx.on_event_async(EventFilter::All, handler, std::time::Duration::from_secs(1));

        ctx.cancel_subscriptions();
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        ctx.emit_event(Event::new("after_cancel"));
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        assert!(received.lock().unwrap().is_empty());
    }
}
