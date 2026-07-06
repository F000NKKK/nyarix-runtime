//! Runtime-to-module context (see issue #18).

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use crate::config::ModuleConfig;
use crate::event::Event;
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
}

impl RuntimeContext {
    /// Create an empty context for the current platform: no config, no
    /// resolvable dependencies. Suitable for unit tests and as a
    /// stand-in until the Runtime (M4) builds real ones.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            config: ModuleConfig::empty(),
            metrics: MetricsHandle::default(),
            sandbox: SandboxHandle::default(),
            platform: Platform::current(),
            dependencies: HashMap::new(),
        }
    }

    /// Build a context with the given config and resolvable dependencies.
    #[must_use]
    pub fn new(config: ModuleConfig, dependencies: HashMap<String, Arc<dyn Module>>) -> Self {
        Self {
            config,
            metrics: MetricsHandle::default(),
            sandbox: SandboxHandle::default(),
            platform: Platform::current(),
            dependencies,
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

    /// Publish an event.
    ///
    /// There is no EventBus yet (M4) — for now this only traces the event
    /// so it isn't silently swallowed.
    pub fn emit_event(&self, event: Event) {
        tracing::debug!(event = %event.name, "event emitted (no EventBus wired up yet)");
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
            .field("dependencies", &self.dependencies.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
    fn platform_matches_current_target() {
        let ctx = RuntimeContext::empty();
        assert_eq!(ctx.platform(), Platform::current());
    }

    #[test]
    fn emit_event_does_not_panic_without_a_bus() {
        let ctx = RuntimeContext::empty();
        ctx.emit_event(Event::new("test_event"));
    }
}
