//! Runtime capability enforcement (see issue #73): refusing to run a
//! module that didn't get every capability it required.
//!
//! Builds directly on #70's [`RuntimeContext::request_capabilities`]
//! (the "Сверка с выданным CapabilityHandle" bullet — `granted` on the
//! context *is* the handle #70 issues) and #57's
//! [`nyarix_loader::instantiate`]: [`enforce_and_instantiate`] runs the
//! capability check first and only calls through to `instantiate` (so
//! `Module::initialize` never even runs) if it passes — that's this
//! issue's "Блокировка неразрешённых операций".
//!
//! **Scope note on "Изоляция модуля при нарушении":** this refuses to
//! *start* a module that's missing a required capability — the module
//! is simply never instantiated/registered, which is as isolated as a
//! module can be (it never runs). Isolating an *already-running* module
//! that violates its granted capabilities mid-execution (killing its
//! task, a separate Tokio runtime, ...) needs more of the real Sandbox
//! execution boundary (#75) than exists yet — this function does reuse
//! #75's [`crate::sandbox::catch_module_panic`] around `Module::initialize`
//! itself (see [`enforce_and_instantiate`]'s docs), so a module that
//! panics during initialization is contained the same way a module
//! that panics during `process` is (`execution_loop::process_one`),
//! rather than unwinding into the Runtime.

use std::sync::Arc;

use nyarix_error::SecurityError;
use nyarix_loader::InstantiationError;
use nyarix_module_api::{Capability, Module, RuntimeContext};

/// Enforcing a module's capability grant, or the instantiation it
/// gated, failed.
#[derive(Debug, thiserror::Error)]
pub enum EnforcementError {
    /// The module requires at least one capability its
    /// [`RuntimeContext`] wasn't granted — see this module's docs for
    /// what "denied" means here.
    #[error(transparent)]
    CapabilityDenied(#[from] SecurityError),
    /// The capability check passed, but instantiation itself failed —
    /// either an ordinary `Module::initialize` error, or a panic during
    /// it caught and converted by [`crate::sandbox::catch_module_panic`]
    /// (#75) into [`nyarix_error::ModuleError::Crashed`].
    #[error(transparent)]
    Instantiation(#[from] InstantiationError),
}

/// Check `module`'s required capabilities (#21) against what `ctx` was
/// granted (#70), and only instantiate it (#57) if every one of them
/// was granted.
///
/// Every denied capability is logged via `tracing::warn!` before
/// returning (this issue's "Логирование попыток нарушения") — even
/// though the returned error only names one (matching
/// [`SecurityError::CapabilityDenied`]'s single-`capability` shape),
/// so an operator watching logs sees the full list, not just the first.
///
/// If the capability check passes, `Module::initialize` (via
/// [`nyarix_loader::instantiate`]) runs inside
/// [`crate::sandbox::catch_module_panic`] (#75) — a panic during
/// initialization is caught and reported as an ordinary error rather
/// than unwinding into the Runtime.
///
/// # Errors
/// Returns [`EnforcementError::CapabilityDenied`] (wrapping
/// [`SecurityError::CapabilityDenied`], naming the first denied
/// capability) if the grant is incomplete — `module` is not
/// instantiated in that case, i.e. `Module::initialize` is never
/// called. Returns [`EnforcementError::Instantiation`] if the
/// capability check passed but instantiation itself failed or panicked.
pub fn enforce_and_instantiate(
    module: Box<dyn Module>,
    ctx: &RuntimeContext,
) -> Result<Arc<dyn Module>, EnforcementError> {
    let name = module.metadata().name.clone();
    let grant = ctx.request_capabilities(module.metadata());

    if !grant.is_fully_granted() {
        for capability in &grant.denied {
            tracing::warn!(
                module = %name,
                capability = ?capability,
                "capability denied; refusing to instantiate module"
            );
        }
        // `is_fully_granted` being false guarantees `denied` is
        // non-empty, so this first capability always exists.
        let capability = capability_name(grant.denied[0]);
        return Err(EnforcementError::CapabilityDenied(
            SecurityError::CapabilityDenied {
                module: name,
                capability,
            },
        ));
    }

    tracing::debug!(module = %name, "capability check passed");
    let panic_guard_name = name.clone();
    crate::sandbox::catch_module_panic(&panic_guard_name, move || {
        nyarix_loader::instantiate(module, ctx).map_err(|error| error.source)
    })
    .map_err(|source| EnforcementError::Instantiation(InstantiationError { name, source }))
}

fn capability_name(capability: Capability) -> String {
    format!("{capability:?}").to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use nyarix_error::ModuleError;
    use nyarix_module_api::{CapabilityMask, Health, ModuleMetadata, ModuleType};
    use nyarix_packet::Packet;

    struct StubModule {
        metadata: ModuleMetadata,
        initialized: bool,
    }

    impl StubModule {
        fn requiring(capabilities: Vec<Capability>) -> Self {
            Self {
                metadata: ModuleMetadata::new(
                    "quic-transport",
                    semver::Version::new(0, 1, 0),
                    ModuleType::Transport,
                )
                .with_required_capabilities(capabilities),
                initialized: false,
            }
        }
    }

    impl Module for StubModule {
        fn metadata(&self) -> &ModuleMetadata {
            &self.metadata
        }

        fn initialize(&mut self, _ctx: &RuntimeContext) -> Result<(), ModuleError> {
            self.initialized = true;
            Ok(())
        }

        fn process(&mut self, packet: Packet) -> Result<Option<Packet>, ModuleError> {
            Ok(Some(packet))
        }

        fn shutdown(&mut self, _ctx: &RuntimeContext) -> Result<(), ModuleError> {
            Ok(())
        }

        fn health(&self) -> Health {
            Health::Healthy
        }
    }

    #[test]
    fn a_module_with_a_fully_granted_capability_set_instantiates() {
        let ctx = RuntimeContext::empty()
            .with_granted_capabilities(CapabilityMask::from_capabilities(&[Capability::Network]));
        let module: Box<dyn Module> = Box::new(StubModule::requiring(vec![Capability::Network]));

        let instance = enforce_and_instantiate(module, &ctx).unwrap();

        assert_eq!(instance.metadata().name, "quic-transport");
    }

    #[test]
    fn a_module_missing_a_required_capability_is_refused() {
        let ctx = RuntimeContext::empty();
        let module: Box<dyn Module> = Box::new(StubModule::requiring(vec![Capability::Network]));

        let Err(err) = enforce_and_instantiate(module, &ctx) else {
            panic!("expected enforce_and_instantiate to fail");
        };

        let EnforcementError::CapabilityDenied(SecurityError::CapabilityDenied {
            module,
            capability,
        }) = err
        else {
            panic!("expected CapabilityDenied");
        };
        assert_eq!(module, "quic-transport");
        assert_eq!(capability, "network");
    }

    #[test]
    fn a_module_with_no_required_capabilities_always_instantiates() {
        let ctx = RuntimeContext::empty();
        let module: Box<dyn Module> = Box::new(StubModule::requiring(vec![]));

        assert!(enforce_and_instantiate(module, &ctx).is_ok());
    }

    #[test]
    fn a_partially_granted_module_is_still_refused_entirely() {
        let ctx = RuntimeContext::empty()
            .with_granted_capabilities(CapabilityMask::from_capabilities(&[Capability::Network]));
        let module: Box<dyn Module> = Box::new(StubModule::requiring(vec![
            Capability::Network,
            Capability::Tun,
        ]));

        let Err(err) = enforce_and_instantiate(module, &ctx) else {
            panic!("expected enforce_and_instantiate to fail");
        };

        assert!(matches!(
            err,
            EnforcementError::CapabilityDenied(SecurityError::CapabilityDenied { .. })
        ));
    }

    struct PanickingModule {
        metadata: ModuleMetadata,
    }

    impl Module for PanickingModule {
        fn metadata(&self) -> &ModuleMetadata {
            &self.metadata
        }

        fn initialize(&mut self, _ctx: &RuntimeContext) -> Result<(), ModuleError> {
            panic!("boom");
        }

        fn process(&mut self, packet: Packet) -> Result<Option<Packet>, ModuleError> {
            Ok(Some(packet))
        }

        fn shutdown(&mut self, _ctx: &RuntimeContext) -> Result<(), ModuleError> {
            Ok(())
        }

        fn health(&self) -> Health {
            Health::Healthy
        }
    }

    #[test]
    fn a_panic_during_initialize_is_caught_and_reported_as_crashed() {
        let ctx = RuntimeContext::empty();
        let module: Box<dyn Module> = Box::new(PanickingModule {
            metadata: ModuleMetadata::new(
                "panicky-transport",
                semver::Version::new(0, 1, 0),
                ModuleType::Transport,
            ),
        });

        let Err(EnforcementError::Instantiation(err)) = enforce_and_instantiate(module, &ctx)
        else {
            panic!("expected EnforcementError::Instantiation");
        };
        assert_eq!(err.name, "panicky-transport");
        assert!(matches!(err.source, ModuleError::Crashed { .. }));
    }
}
