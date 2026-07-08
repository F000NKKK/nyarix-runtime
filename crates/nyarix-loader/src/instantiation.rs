//! Module instantiation (see issue #57).
//!
//! **Scope note:** actually loading a module's compiled code — this
//! issue's "Загрузка WASM/native кода" bullet — needs infrastructure
//! that doesn't exist anywhere in this workspace yet: no WASM engine
//! dependency, no native dynamic-library loading code, and no defined
//! ABI for how a `dyn Module` trait object is supposed to cross either
//! boundary. This workspace's own lints also `deny(unsafe_code)`
//! workspace-wide, which native `dlopen`-based loading would need —
//! resolving that tension is its own design decision, not a guess to
//! make here. See #107 for the tracked follow-up.
//!
//! What this module *does* implement is everything downstream of
//! "you already have a `Box<dyn Module>`, however it was produced":
//! calling `initialize`, handling failure without aborting the whole
//! load, and registering the result so other modules can resolve it as
//! a dependency (#18/#56).

use std::collections::HashMap;
use std::sync::Arc;

use nyarix_module_api::{Module, ModuleError, RuntimeContext};

/// A module failed during [`instantiate`].
#[derive(Debug, thiserror::Error)]
#[error("module '{name}' failed to initialize: {source}")]
pub struct InstantiationError {
    /// The module's declared name.
    pub name: String,
    /// The underlying initialization error.
    #[source]
    pub source: ModuleError,
}

/// Initialize `module` against `ctx` and, on success, hand back a
/// shared, read-only handle to it.
///
/// This takes an already-constructed `Box<dyn Module>` rather than a
/// path or package — see this module's scope note on why actually
/// producing one isn't implemented here yet.
///
/// # Errors
/// Returns [`InstantiationError`] if `Module::initialize` fails. The
/// module is not returned in that case — a caller loading several
/// modules should catch this per-module and keep going (see #41's
/// "a failed module is logged, not fatal to the Runtime" principle),
/// not abort the whole load.
pub fn instantiate(
    mut module: Box<dyn Module>,
    ctx: &RuntimeContext,
) -> Result<Arc<dyn Module>, InstantiationError> {
    let name = module.metadata().name.clone();
    module
        .initialize(ctx)
        .map_err(|source| InstantiationError {
            name: name.clone(),
            source,
        })?;
    Ok(Arc::from(module))
}

/// Successfully instantiated modules, registered by name so other
/// modules can resolve them as dependencies.
#[derive(Default)]
pub struct ModuleRegistry {
    modules: HashMap<String, Arc<dyn Module>>,
}

impl ModuleRegistry {
    /// An empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// [`instantiate`] `module`, and on success register it under its
    /// own declared name (overwriting any previous entry with the same
    /// name).
    ///
    /// # Errors
    /// Returns [`InstantiationError`] if initialization fails; nothing
    /// is registered in that case.
    pub fn instantiate_and_register(
        &mut self,
        module: Box<dyn Module>,
        ctx: &RuntimeContext,
    ) -> Result<(), InstantiationError> {
        let instance = instantiate(module, ctx)?;
        let name = instance.metadata().name.clone();
        self.modules.insert(name, instance);
        Ok(())
    }

    /// Look up a registered module by name.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&Arc<dyn Module>> {
        self.modules.get(name)
    }

    /// Build the dependency map a [`RuntimeContext`] needs (#18), containing
    /// whichever of `names` are actually registered — an unregistered
    /// name is simply absent from the result, not an error (see #56:
    /// deciding whether that's fatal is the caller's job, e.g. based on
    /// whether the dependency was declared optional).
    #[must_use]
    pub fn dependencies_for(&self, names: &[String]) -> HashMap<String, Arc<dyn Module>> {
        names
            .iter()
            .filter_map(|name| self.get(name).map(|module| (name.clone(), Arc::clone(module))))
            .collect()
    }

    /// Number of registered modules.
    #[must_use]
    pub fn len(&self) -> usize {
        self.modules.len()
    }

    /// Whether no modules are registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.modules.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nyarix_module_api::{Health, ModuleMetadata, ModuleType};
    use nyarix_packet::Packet;

    struct StubModule {
        metadata: ModuleMetadata,
        fail_init: bool,
        initialized: bool,
    }

    impl StubModule {
        fn new(name: &str) -> Self {
            Self {
                metadata: ModuleMetadata::new(name, semver::Version::new(0, 1, 0), ModuleType::Flow),
                fail_init: false,
                initialized: false,
            }
        }

        fn failing(name: &str) -> Self {
            Self {
                fail_init: true,
                ..Self::new(name)
            }
        }
    }

    impl Module for StubModule {
        fn metadata(&self) -> &ModuleMetadata {
            &self.metadata
        }

        fn initialize(&mut self, _ctx: &RuntimeContext) -> nyarix_module_api::Result<()> {
            if self.fail_init {
                return Err(ModuleError::InitFailed {
                    name: self.metadata.name.clone(),
                    reason: "stub configured to fail".to_string(),
                });
            }
            self.initialized = true;
            Ok(())
        }

        fn process(&mut self, packet: Packet) -> nyarix_module_api::Result<Option<Packet>> {
            Ok(Some(packet))
        }

        fn shutdown(&mut self, _ctx: &RuntimeContext) -> nyarix_module_api::Result<()> {
            Ok(())
        }

        fn health(&self) -> Health {
            Health::Healthy
        }
    }

    #[test]
    fn instantiate_initializes_and_returns_the_module() {
        let ctx = RuntimeContext::empty();
        let module: Box<dyn Module> = Box::new(StubModule::new("udp-transport"));

        let instance = instantiate(module, &ctx).unwrap();

        assert_eq!(instance.metadata().name, "udp-transport");
    }

    #[test]
    fn instantiate_propagates_an_initialization_failure() {
        let ctx = RuntimeContext::empty();
        let module: Box<dyn Module> = Box::new(StubModule::failing("broken-transport"));

        let err = instantiate(module, &ctx).unwrap_err();

        assert_eq!(err.name, "broken-transport");
    }

    #[test]
    fn registry_registers_a_successfully_instantiated_module() {
        let ctx = RuntimeContext::empty();
        let mut registry = ModuleRegistry::new();

        registry
            .instantiate_and_register(Box::new(StubModule::new("dns-resolver")), &ctx)
            .unwrap();

        assert_eq!(registry.len(), 1);
        assert!(registry.get("dns-resolver").is_some());
    }

    #[test]
    fn registry_does_not_register_a_module_that_fails_to_initialize() {
        let ctx = RuntimeContext::empty();
        let mut registry = ModuleRegistry::new();

        let result = registry.instantiate_and_register(Box::new(StubModule::failing("broken")), &ctx);

        assert!(result.is_err());
        assert!(registry.is_empty());
        assert!(registry.get("broken").is_none());
    }

    #[test]
    fn dependencies_for_only_includes_registered_names() {
        let ctx = RuntimeContext::empty();
        let mut registry = ModuleRegistry::new();
        registry
            .instantiate_and_register(Box::new(StubModule::new("dns-resolver")), &ctx)
            .unwrap();

        let deps = registry.dependencies_for(&[
            "dns-resolver".to_string(),
            "nonexistent-module".to_string(),
        ]);

        assert_eq!(deps.len(), 1);
        assert!(deps.contains_key("dns-resolver"));
    }

    #[test]
    fn registering_the_same_name_twice_overwrites_the_previous_entry() {
        let ctx = RuntimeContext::empty();
        let mut registry = ModuleRegistry::new();

        registry
            .instantiate_and_register(Box::new(StubModule::new("dns-resolver")), &ctx)
            .unwrap();
        registry
            .instantiate_and_register(Box::new(StubModule::new("dns-resolver")), &ctx)
            .unwrap();

        assert_eq!(registry.len(), 1);
    }
}
