//! Runtime-to-module context (see issue #18).

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex};

use tokio::task::JoinHandle;

use crate::capability::{Capability, CapabilityGrant, CapabilityMask};
use crate::config::ModuleConfig;
use crate::event::{Event, EventBus, EventFilter, EventHandler};
use crate::io_policy::{FilesystemPolicy, NetworkPolicy};
use crate::metadata::ModuleMetadata;
use crate::metrics::MetricsHandle;
use crate::module::Module;
use crate::platform::Platform;
use crate::rate_limiter::RateLimiter;
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
    granted_capabilities: CapabilityMask,
    filesystem_policy: FilesystemPolicy,
    network_policy: NetworkPolicy,
    io_rate_limiter: Option<RateLimiter>,
    open_connections: Mutex<HashMap<String, usize>>,
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
            granted_capabilities: CapabilityMask::empty(),
            filesystem_policy: FilesystemPolicy::deny_all(),
            network_policy: NetworkPolicy::deny_all(),
            io_rate_limiter: None,
            open_connections: Mutex::new(HashMap::new()),
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
            granted_capabilities: CapabilityMask::empty(),
            filesystem_policy: FilesystemPolicy::deny_all(),
            network_policy: NetworkPolicy::deny_all(),
            io_rate_limiter: None,
            open_connections: Mutex::new(HashMap::new()),
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
            granted_capabilities: CapabilityMask::empty(),
            filesystem_policy: FilesystemPolicy::deny_all(),
            network_policy: NetworkPolicy::deny_all(),
            io_rate_limiter: None,
            open_connections: Mutex::new(HashMap::new()),
        }
    }

    /// Attach a granted capability mask to this context (#70) — the
    /// Runtime calls this (typically right after building the context,
    /// before handing it to a module's `initialize`) with whatever its
    /// security policy decided this module gets. Defaults to
    /// [`CapabilityMask::empty()`] on every other constructor, meaning
    /// "granted nothing" rather than "granted everything" — a module
    /// this wasn't set on can't accidentally pass a capability check.
    #[must_use]
    pub fn with_granted_capabilities(mut self, granted: CapabilityMask) -> Self {
        self.granted_capabilities = granted;
        self
    }

    /// The capability mask the Runtime granted this module (#70), set
    /// via [`Self::with_granted_capabilities`] — [`CapabilityMask::empty()`]
    /// if never set.
    #[must_use]
    pub fn granted_capabilities(&self) -> CapabilityMask {
        self.granted_capabilities
    }

    /// Check `metadata.required_capabilities` (#21) against
    /// [`Self::granted_capabilities`] and report which are actually
    /// granted vs. denied by the Runtime's current policy (#70).
    ///
    /// This only computes the grant — it doesn't reject or fail a
    /// module for an incomplete one; deciding what to do about a
    /// [`CapabilityGrant`] that isn't fully granted (block the module?
    /// let it run degraded?) is Runtime enforcement's job (#73/#74).
    #[must_use]
    pub fn request_capabilities(&self, metadata: &ModuleMetadata) -> CapabilityGrant {
        CapabilityGrant::request(&metadata.required_capabilities, self.granted_capabilities)
    }

    /// Request a single `capability` at runtime — not just at load time
    /// like [`Self::request_capabilities`] — and handle a denial
    /// gracefully rather than fatally (#74's "graceful degradation").
    ///
    /// A module calls this right before doing something that needs a
    /// specific capability (e.g. right before opening a socket, for
    /// [`crate::capability::Capability::Network`]). If it's not in
    /// [`Self::granted_capabilities`]:
    /// - the Runtime is **not** crashed — this returns an `Err`, same as
    ///   any other fallible call;
    /// - the caller (the module) gets that `Err` back and decides its
    ///   own fallback path — this function doesn't invent a generic
    ///   substitute behavior, since only the module knows what
    ///   "degraded" means for it (see [`crate::health::Health::Degraded`]
    ///   for how it can then report that to the Runtime);
    /// - an [`Event::CapabilityDenied`] is published on this context's
    ///   [`EventBus`] (if any), so other subscribers (a UI, monitoring,
    ///   an audit trail) learn about the denial without polling.
    ///
    /// Requesting privilege escalation from the user interactively isn't
    /// implemented — this workspace has no UI/interaction channel yet
    /// (`Capability::UiHooks` is declarable but nothing consumes it),
    /// tracked separately.
    ///
    /// # Errors
    /// Returns [`nyarix_error::SecurityError::CapabilityDenied`] if
    /// `capability` isn't in [`Self::granted_capabilities`].
    pub fn request_capability(
        &self,
        metadata: &ModuleMetadata,
        capability: Capability,
    ) -> Result<(), nyarix_error::SecurityError> {
        if self.granted_capabilities.has(capability) {
            return Ok(());
        }

        let module = metadata.name.clone();
        let capability_name = format!("{capability:?}").to_lowercase();

        tracing::warn!(
            module = %module,
            capability = %capability_name,
            "capability denied at runtime"
        );
        self.emit_event(Event::CapabilityDenied {
            module: module.clone(),
            capability: capability_name.clone(),
        });

        Err(nyarix_error::SecurityError::CapabilityDenied {
            module,
            capability: capability_name,
        })
    }

    /// Attach a filesystem whitelist (#78) — defaults to
    /// [`FilesystemPolicy::deny_all`] on every constructor.
    #[must_use]
    pub fn with_filesystem_policy(mut self, policy: FilesystemPolicy) -> Self {
        self.filesystem_policy = policy;
        self
    }

    /// Attach a network destination whitelist (#78) — defaults to
    /// [`NetworkPolicy::deny_all`] on every constructor.
    #[must_use]
    pub fn with_network_policy(mut self, policy: NetworkPolicy) -> Self {
        self.network_policy = policy;
        self
    }

    /// Attach an I/O rate limiter (#78's "Rate limiting на I/O
    /// операций") — shared by both [`Self::check_file_access`] and
    /// [`Self::check_network_access`]. No limiter attached (the
    /// default) means unlimited.
    #[must_use]
    pub fn with_io_rate_limiter(mut self, limiter: RateLimiter) -> Self {
        self.io_rate_limiter = Some(limiter);
        self
    }

    /// Check whether `metadata`'s module may access `path` (#78's
    /// "Модуль не может открывать произвольные файлы"): requires
    /// [`Capability::Filesystem`] (via [`Self::request_capability`]),
    /// then the I/O rate limit, then [`FilesystemPolicy::allows`].
    ///
    /// **This mediates access for callers that go through it** — it
    /// doesn't (and structurally can't, without the module isolation
    /// boundary #75/#107 track) stop a native module from calling
    /// `std::fs` directly instead. It's the API surface #78 asks
    /// modules to use, not an unbypassable sandbox.
    ///
    /// # Errors
    /// Returns [`nyarix_error::SecurityError::CapabilityDenied`] if
    /// `Capability::Filesystem` wasn't granted, or
    /// [`nyarix_error::SecurityError::SandboxViolation`] if the I/O rate
    /// limit is exceeded or `path` isn't in the whitelist.
    pub fn check_file_access(
        &self,
        metadata: &ModuleMetadata,
        path: &std::path::Path,
    ) -> Result<(), nyarix_error::SecurityError> {
        self.request_capability(metadata, Capability::Filesystem)?;
        self.check_rate_limit(metadata)?;
        if !self.filesystem_policy.allows(path) {
            return Err(nyarix_error::SecurityError::SandboxViolation {
                module: metadata.name.clone(),
                violation: format!("filesystem access to {} is not whitelisted", path.display()),
            });
        }
        Ok(())
    }

    /// Check whether `metadata`'s module may connect to `host:port`
    /// (#78's "Сетевые вызовы только через Runtime API" and "Whitelist
    /// адресов/портов"): requires [`Capability::Network`] (via
    /// [`Self::request_capability`]), then the I/O rate limit, then
    /// [`NetworkPolicy::allows`].
    ///
    /// Same caveat as [`Self::check_file_access`] on this being a
    /// mediated API surface, not an unbypassable sandbox for native
    /// modules.
    ///
    /// # Errors
    /// Returns [`nyarix_error::SecurityError::CapabilityDenied`] if
    /// `Capability::Network` wasn't granted, or
    /// [`nyarix_error::SecurityError::SandboxViolation`] if the I/O rate
    /// limit is exceeded or `host:port` isn't in the whitelist.
    pub fn check_network_access(
        &self,
        metadata: &ModuleMetadata,
        host: &str,
        port: u16,
    ) -> Result<(), nyarix_error::SecurityError> {
        self.request_capability(metadata, Capability::Network)?;
        self.check_rate_limit(metadata)?;
        if !self.network_policy.allows(host, port) {
            return Err(nyarix_error::SecurityError::SandboxViolation {
                module: metadata.name.clone(),
                violation: format!("network access to {host}:{port} is not whitelisted"),
            });
        }
        Ok(())
    }

    fn check_rate_limit(
        &self,
        metadata: &ModuleMetadata,
    ) -> Result<(), nyarix_error::SecurityError> {
        let Some(limiter) = &self.io_rate_limiter else {
            return Ok(());
        };
        if limiter.try_acquire() {
            Ok(())
        } else {
            Err(nyarix_error::SecurityError::SandboxViolation {
                module: metadata.name.clone(),
                violation: "I/O rate limit exceeded".to_string(),
            })
        }
    }

    /// Check whether `metadata`'s module may resolve `host` via DNS
    /// (#79's "DNS-резолв только через Runtime"): requires
    /// [`Capability::Dns`], then the I/O rate limit, then
    /// [`NetworkPolicy::allows_host`] — reusing the same whitelist
    /// [`Self::check_network_access`] checks, on the theory that a host
    /// not worth *connecting* to isn't worth *resolving* either.
    ///
    /// # Errors
    /// Returns [`nyarix_error::SecurityError::CapabilityDenied`] if
    /// `Capability::Dns` wasn't granted, or
    /// [`nyarix_error::SecurityError::SandboxViolation`] if the I/O rate
    /// limit is exceeded or `host` isn't in the whitelist.
    pub fn check_dns_resolve(
        &self,
        metadata: &ModuleMetadata,
        host: &str,
    ) -> Result<(), nyarix_error::SecurityError> {
        self.request_capability(metadata, Capability::Dns)?;
        self.check_rate_limit(metadata)?;
        if !self.network_policy.allows_host(host) {
            return Err(nyarix_error::SecurityError::SandboxViolation {
                module: metadata.name.clone(),
                violation: format!("DNS resolution of {host} is not whitelisted"),
            });
        }
        Ok(())
    }

    /// Check whether `metadata`'s module may bind/listen on `port`
    /// (#79's "Запрет на listen/bind без capability"): requires
    /// [`Capability::Network`], and additionally
    /// [`Capability::PrivilegedSockets`] if `port` is a privileged
    /// (below 1024) port — matching that capability's own documented
    /// purpose ("Bind privileged (low-numbered) sockets").
    ///
    /// No whitelist check here (unlike [`Self::check_network_access`]):
    /// binding is about what *this* module exposes locally, not which
    /// remote destination it reaches, so [`NetworkPolicy`] doesn't
    /// apply.
    ///
    /// # Errors
    /// Returns [`nyarix_error::SecurityError::CapabilityDenied`] if the
    /// required capability/capabilities weren't granted, or
    /// [`nyarix_error::SecurityError::SandboxViolation`] if the I/O rate
    /// limit is exceeded.
    pub fn check_bind_access(
        &self,
        metadata: &ModuleMetadata,
        port: u16,
    ) -> Result<(), nyarix_error::SecurityError> {
        self.request_capability(metadata, Capability::Network)?;
        if port < 1024 {
            self.request_capability(metadata, Capability::PrivilegedSockets)?;
        }
        self.check_rate_limit(metadata)
    }

    /// Record that `metadata`'s module opened one more network
    /// connection (#79's "Connection tracking: сколько соединений
    /// открыл модуль") — the caller (whatever eventually does real
    /// networking) calls this once a connection actually opens, and
    /// [`Self::track_connection_closed`] once it closes.
    pub fn track_connection_opened(&self, metadata: &ModuleMetadata) {
        let mut open = self
            .open_connections
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *open.entry(metadata.name.clone()).or_insert(0) += 1;
    }

    /// Record that one of `metadata`'s module's tracked connections
    /// closed. A no-op if it had none open (never goes negative).
    pub fn track_connection_closed(&self, metadata: &ModuleMetadata) {
        let mut open = self
            .open_connections
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(count) = open.get_mut(&metadata.name) {
            *count = count.saturating_sub(1);
        }
    }

    /// How many open connections are currently tracked for `module_name`
    /// (see [`Self::track_connection_opened`]) — `0` if none were ever
    /// recorded.
    #[must_use]
    pub fn open_connection_count(&self, module_name: &str) -> usize {
        self.open_connections
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(module_name)
            .copied()
            .unwrap_or(0)
    }

    /// This module's configuration.
    #[must_use]
    pub fn config(&self) -> &ModuleConfig {
        &self.config
    }

    /// Handle for recording metrics (#80) — see [`MetricsHandle`] docs.
    #[must_use]
    pub fn metrics(&self) -> &MetricsHandle {
        &self.metrics
    }

    /// Attach a live [`crate::metrics::MetricRegistry`] (#80) — what
    /// the Runtime hands modules once it actually has one running,
    /// same pattern as [`Self::with_event_bus`]. Every other
    /// constructor defaults to [`MetricsHandle::default`] (no
    /// registry attached, so recording is a silent no-op).
    #[must_use]
    pub fn with_metrics_registry(mut self, registry: Arc<crate::metrics::MetricRegistry>) -> Self {
        self.metrics = MetricsHandle::with_registry(registry);
        self
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

    /// Handle for sandbox interaction (#75) — currently just this
    /// module's own cancellation token, see [`SandboxHandle`] docs for
    /// what's implemented vs. still a marker.
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
    fn a_context_with_no_capabilities_attached_grants_nothing() {
        let ctx = RuntimeContext::empty();
        assert_eq!(ctx.granted_capabilities(), CapabilityMask::empty());
    }

    #[test]
    fn with_granted_capabilities_is_reflected_back() {
        let mask = CapabilityMask::from_capabilities(&[Capability::Network]);
        let ctx = RuntimeContext::empty().with_granted_capabilities(mask);
        assert_eq!(ctx.granted_capabilities(), mask);
    }

    #[test]
    fn request_capabilities_reports_what_the_context_was_granted() {
        use crate::capability::Capability;

        let mask = CapabilityMask::from_capabilities(&[Capability::Network]);
        let ctx = RuntimeContext::empty().with_granted_capabilities(mask);
        let metadata = support::new_metadata("quic-transport")
            .with_required_capabilities(vec![Capability::Network, Capability::Tun]);

        let grant = ctx.request_capabilities(&metadata);

        assert!(!grant.is_fully_granted());
        assert!(grant.has(Capability::Network));
        assert!(!grant.has(Capability::Tun));
        assert_eq!(grant.denied, vec![Capability::Tun]);
    }

    #[test]
    fn request_capability_succeeds_when_granted() {
        use crate::capability::Capability;

        let mask = CapabilityMask::from_capabilities(&[Capability::Network]);
        let ctx = RuntimeContext::empty().with_granted_capabilities(mask);
        let metadata = support::new_metadata("quic-transport");

        assert!(
            ctx.request_capability(&metadata, Capability::Network)
                .is_ok()
        );
    }

    #[test]
    fn request_capability_does_not_panic_when_denied_and_returns_an_error() {
        use crate::capability::Capability;

        let ctx = RuntimeContext::empty();
        let metadata = support::new_metadata("quic-transport");

        let Err(err) = ctx.request_capability(&metadata, Capability::Network) else {
            panic!("expected request_capability to fail");
        };

        let nyarix_error::SecurityError::CapabilityDenied { module, capability } = err else {
            panic!("expected CapabilityDenied");
        };
        assert_eq!(module, "quic-transport");
        assert_eq!(capability, "network");
    }

    #[test]
    fn check_file_access_denies_without_the_filesystem_capability() {
        let ctx = RuntimeContext::empty().with_filesystem_policy(FilesystemPolicy::allowing([
            std::path::PathBuf::from("/tmp"),
        ]));
        let metadata = support::new_metadata("storage-module");

        let Err(err) = ctx.check_file_access(&metadata, std::path::Path::new("/tmp/data")) else {
            panic!("expected check_file_access to fail");
        };
        assert!(matches!(
            err,
            nyarix_error::SecurityError::CapabilityDenied { .. }
        ));
    }

    #[test]
    fn check_file_access_denies_a_path_outside_the_whitelist() {
        use crate::capability::Capability;

        let ctx = RuntimeContext::empty()
            .with_granted_capabilities(CapabilityMask::from_capabilities(&[Capability::Filesystem]))
            .with_filesystem_policy(FilesystemPolicy::allowing([std::path::PathBuf::from(
                "/tmp",
            )]));
        let metadata = support::new_metadata("storage-module");

        let Err(err) = ctx.check_file_access(&metadata, std::path::Path::new("/etc/passwd")) else {
            panic!("expected check_file_access to fail");
        };
        assert!(matches!(
            err,
            nyarix_error::SecurityError::SandboxViolation { .. }
        ));
    }

    #[test]
    fn check_file_access_allows_a_whitelisted_path_with_the_capability_granted() {
        use crate::capability::Capability;

        let ctx = RuntimeContext::empty()
            .with_granted_capabilities(CapabilityMask::from_capabilities(&[Capability::Filesystem]))
            .with_filesystem_policy(FilesystemPolicy::allowing([std::path::PathBuf::from(
                "/tmp",
            )]));
        let metadata = support::new_metadata("storage-module");

        assert!(
            ctx.check_file_access(&metadata, std::path::Path::new("/tmp/data"))
                .is_ok()
        );
    }

    #[test]
    fn check_network_access_denies_a_destination_outside_the_whitelist() {
        use crate::capability::Capability;

        let ctx = RuntimeContext::empty()
            .with_granted_capabilities(CapabilityMask::from_capabilities(&[Capability::Network]))
            .with_network_policy(NetworkPolicy::deny_all().allow_host("example.com"));
        let metadata = support::new_metadata("quic-transport");

        let Err(err) = ctx.check_network_access(&metadata, "evil.example", 443) else {
            panic!("expected check_network_access to fail");
        };
        assert!(matches!(
            err,
            nyarix_error::SecurityError::SandboxViolation { .. }
        ));
    }

    #[test]
    fn check_network_access_allows_a_whitelisted_destination() {
        use crate::capability::Capability;

        let ctx = RuntimeContext::empty()
            .with_granted_capabilities(CapabilityMask::from_capabilities(&[Capability::Network]))
            .with_network_policy(NetworkPolicy::deny_all().allow_host("example.com"));
        let metadata = support::new_metadata("quic-transport");

        assert!(
            ctx.check_network_access(&metadata, "example.com", 443)
                .is_ok()
        );
    }

    #[test]
    fn the_io_rate_limiter_is_shared_between_file_and_network_checks() {
        use crate::capability::Capability;

        let ctx = RuntimeContext::empty()
            .with_granted_capabilities(CapabilityMask::from_capabilities(&[
                Capability::Filesystem,
                Capability::Network,
            ]))
            .with_filesystem_policy(FilesystemPolicy::allowing([std::path::PathBuf::from(
                "/tmp",
            )]))
            .with_network_policy(NetworkPolicy::deny_all().allow_host("example.com"))
            .with_io_rate_limiter(RateLimiter::new(1, 0));
        let metadata = support::new_metadata("busy-module");

        assert!(
            ctx.check_file_access(&metadata, std::path::Path::new("/tmp/data"))
                .is_ok()
        );

        let Err(err) = ctx.check_network_access(&metadata, "example.com", 443) else {
            panic!("expected the shared rate limiter to already be exhausted");
        };
        assert!(matches!(
            err,
            nyarix_error::SecurityError::SandboxViolation { .. }
        ));
    }

    #[test]
    fn metrics_are_a_silent_no_op_without_a_registry_attached() {
        let ctx = RuntimeContext::empty();
        assert!(ctx.metrics().counter("quic", "packets_sent").is_none());
    }

    #[test]
    fn with_metrics_registry_attaches_a_working_handle() {
        use crate::metrics::MetricRegistry;

        let registry = Arc::new(MetricRegistry::new());
        let ctx = RuntimeContext::empty().with_metrics_registry(Arc::clone(&registry));

        ctx.metrics()
            .counter("quic", "packets_sent")
            .unwrap()
            .increment(1);

        assert_eq!(registry.counter("quic", "packets_sent").value(), 1);
    }

    #[test]
    fn check_dns_resolve_denies_without_the_dns_capability() {
        let ctx = RuntimeContext::empty()
            .with_network_policy(NetworkPolicy::deny_all().allow_host("example.com"));
        let metadata = support::new_metadata("dns-client");

        let Err(err) = ctx.check_dns_resolve(&metadata, "example.com") else {
            panic!("expected check_dns_resolve to fail");
        };
        assert!(matches!(
            err,
            nyarix_error::SecurityError::CapabilityDenied { .. }
        ));
    }

    #[test]
    fn check_dns_resolve_denies_a_host_outside_the_whitelist() {
        use crate::capability::Capability;

        let ctx = RuntimeContext::empty()
            .with_granted_capabilities(CapabilityMask::from_capabilities(&[Capability::Dns]))
            .with_network_policy(NetworkPolicy::deny_all().allow_host("example.com"));
        let metadata = support::new_metadata("dns-client");

        let Err(err) = ctx.check_dns_resolve(&metadata, "evil.example") else {
            panic!("expected check_dns_resolve to fail");
        };
        assert!(matches!(
            err,
            nyarix_error::SecurityError::SandboxViolation { .. }
        ));
    }

    #[test]
    fn check_dns_resolve_allows_a_whitelisted_host_with_the_capability_granted() {
        use crate::capability::Capability;

        let ctx = RuntimeContext::empty()
            .with_granted_capabilities(CapabilityMask::from_capabilities(&[Capability::Dns]))
            .with_network_policy(NetworkPolicy::deny_all().allow_host("example.com"));
        let metadata = support::new_metadata("dns-client");

        assert!(ctx.check_dns_resolve(&metadata, "example.com").is_ok());
    }

    #[test]
    fn check_bind_access_denies_without_the_network_capability() {
        let ctx = RuntimeContext::empty();
        let metadata = support::new_metadata("listener");

        let Err(err) = ctx.check_bind_access(&metadata, 8080) else {
            panic!("expected check_bind_access to fail");
        };
        assert!(matches!(
            err,
            nyarix_error::SecurityError::CapabilityDenied { .. }
        ));
    }

    #[test]
    fn check_bind_access_allows_an_unprivileged_port_with_just_network() {
        use crate::capability::Capability;

        let ctx = RuntimeContext::empty()
            .with_granted_capabilities(CapabilityMask::from_capabilities(&[Capability::Network]));
        let metadata = support::new_metadata("listener");

        assert!(ctx.check_bind_access(&metadata, 8080).is_ok());
    }

    #[test]
    fn check_bind_access_denies_a_privileged_port_without_privileged_sockets() {
        use crate::capability::Capability;

        let ctx = RuntimeContext::empty()
            .with_granted_capabilities(CapabilityMask::from_capabilities(&[Capability::Network]));
        let metadata = support::new_metadata("listener");

        let Err(err) = ctx.check_bind_access(&metadata, 80) else {
            panic!("expected check_bind_access to fail on a privileged port");
        };
        assert!(matches!(
            err,
            nyarix_error::SecurityError::CapabilityDenied { .. }
        ));
    }

    #[test]
    fn check_bind_access_allows_a_privileged_port_with_both_capabilities() {
        use crate::capability::Capability;

        let ctx =
            RuntimeContext::empty().with_granted_capabilities(CapabilityMask::from_capabilities(
                &[Capability::Network, Capability::PrivilegedSockets],
            ));
        let metadata = support::new_metadata("listener");

        assert!(ctx.check_bind_access(&metadata, 80).is_ok());
    }

    #[test]
    fn connection_tracking_counts_opens_and_closes_per_module() {
        let ctx = RuntimeContext::empty();
        let metadata = support::new_metadata("quic-transport");
        let other = support::new_metadata("other-transport");

        assert_eq!(ctx.open_connection_count("quic-transport"), 0);

        ctx.track_connection_opened(&metadata);
        ctx.track_connection_opened(&metadata);
        ctx.track_connection_opened(&other);
        assert_eq!(ctx.open_connection_count("quic-transport"), 2);
        assert_eq!(ctx.open_connection_count("other-transport"), 1);

        ctx.track_connection_closed(&metadata);
        assert_eq!(ctx.open_connection_count("quic-transport"), 1);
    }

    #[test]
    fn closing_an_untracked_connection_does_not_underflow() {
        let ctx = RuntimeContext::empty();
        let metadata = support::new_metadata("quic-transport");

        ctx.track_connection_closed(&metadata);
        assert_eq!(ctx.open_connection_count("quic-transport"), 0);
    }

    #[tokio::test]
    async fn a_denied_capability_request_is_published_on_the_event_bus() {
        use crate::capability::Capability;

        let bus = Arc::new(EventBus::default());
        let ctx = RuntimeContext::with_event_bus(ModuleConfig::empty(), HashMap::new(), bus);
        let metadata = support::new_metadata("quic-transport");

        let received: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
        let received_clone = Arc::clone(&received);
        ctx.on_event(EventFilter::All, move |event| {
            received_clone.lock().unwrap().push(event);
        });

        let _ = ctx.request_capability(&metadata, Capability::Network);
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        let received = received.lock().unwrap();
        assert_eq!(received.len(), 1);
        assert_eq!(
            received[0],
            Event::CapabilityDenied {
                module: "quic-transport".to_string(),
                capability: "network".to_string(),
            }
        );
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
        async fn handle(&mut self, event: Event) {
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
            self.received.lock().unwrap().push(event);
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
