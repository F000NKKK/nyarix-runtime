//! Sandbox handle (see issue #18; the real system is M7).

/// Handle a module uses to interact with the Sandbox.
///
/// Currently a marker with no methods — capability/resource enforcement
/// design is M7 (Capability & Sandbox, see #75 Sandbox: изоляция контекста
/// выполнения модуля, and #91 which connects it back to the #21 capability
/// declaration model). Handed out by `RuntimeContext::sandbox()` so module
/// code can already take `&SandboxHandle` without churn once M7 lands.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SandboxHandle {
    _private: (),
}
