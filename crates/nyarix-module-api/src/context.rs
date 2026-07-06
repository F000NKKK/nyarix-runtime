//! Runtime-to-module context (see issue #18).
//!
//! `RuntimeContext` here is an intentionally empty placeholder — just enough
//! for `Module::initialize`/`shutdown`/`migrate` (#16) to have a concrete
//! type to take a reference to. The real API surface (`config()`,
//! `metrics()`, `emit_event()`, `resolve_dependency()`, `platform()`,
//! `sandbox()`) each depend on subsystems that don't exist yet (`ModuleConfig`,
//! `MetricsHandle`/M8, `EventBus`/M4, dependency resolution/M5,
//! `SandboxHandle`/M7) — tracked in #18 to be filled in as those land.

/// Context handed to a module by the Runtime during its lifecycle.
///
/// Currently carries no data; see the module-level docs for what's still
/// missing and why.
#[derive(Debug, Default, Clone, Copy)]
pub struct RuntimeContext {
    _private: (),
}

impl RuntimeContext {
    /// Create an empty context.
    ///
    /// Stand-in constructor until the Runtime (M4) can build a real one
    /// carrying config/metrics/event-bus/sandbox handles.
    #[must_use]
    pub fn empty() -> Self {
        Self::default()
    }
}
