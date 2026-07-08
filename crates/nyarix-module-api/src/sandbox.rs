//! Sandbox handle (see issue #18; the real system is M7, #75).
//!
//! Of #75's five bullets:
//! - **"Отдельный токен для tokio"**: [`SandboxHandle::isolated`] gives
//!   each module its own [`CancellationToken`], a child of whatever
//!   parent token the Runtime cancels for a full shutdown — cancelling
//!   *this* module's token (e.g. after a capability violation, #73/#74)
//!   doesn't need cancelling every other module's, and cancelling the
//!   parent still cancels every child. A separate `tokio::Runtime` per
//!   module (the bullet's parenthetical alternative) is a heavier
//!   design than this workspace's single-runtime execution model
//!   currently assumes — not attempted here.
//!
//!   **Caveat:** [`crate::context::RuntimeContext`] is still one shared
//!   instance for a whole execution loop run (`nyarix_runtime::execution_loop`),
//!   not one per node/module — so today every node in a run sees the
//!   same [`SandboxHandle`]/token until something actually builds a
//!   `RuntimeContext` per node, which is #95's decision to make, not
//!   this one's. `SandboxHandle::isolated` exists so that wiring, once
//!   #95 lands, doesn't need a new primitive invented alongside it.
//! - **"Ограничение доступа к глобальному состоянию"**: nothing to add
//!   — this workspace has no global mutable state for a module to reach
//!   in the first place. A module only ever sees what
//!   [`crate::context::RuntimeContext`] hands it (config, resolved
//!   dependencies, the event bus, its capability grant); there's no
//!   singleton/`static mut` a module could reach around that boundary.
//! - **"Перехват паники (catch_unwind)"**: see
//!   [`crate::module::Module`]'s callers — wrapping `initialize`/
//!   `process`/`shutdown` in `catch_unwind` is the *caller's*
//!   responsibility (the Runtime's execution loop and instantiation
//!   path, in `nyarix-runtime`), not something a marker handle in this
//!   crate can enforce by existing. `nyarix_runtime::capability_enforcement`
//!   and `nyarix_runtime::execution_loop` do the actual wrapping.
//! - **"Изоляция через WASM для сторонних модулей"**: needs a WASM
//!   engine and ABI that don't exist in this workspace yet — tracked by
//!   #107, same gap that blocks real module instantiation at all.
//! - **"Graceful shutdown изолированного модуля"**: cancelling
//!   [`SandboxHandle::cancellation_token`] is the signal; actually
//!   stopping *one specific already-running* node while the rest of a
//!   live graph keeps going (rather than the whole Runtime shutting
//!   down together, which #43/#44 already do) needs graph-mutation-
//!   during-live-execution, tracked by #98.

use tokio_util::sync::CancellationToken;

/// Handle a module uses to interact with the Sandbox.
///
/// Carries a [`CancellationToken`] scoped to this one module (see this
/// module's docs) in addition to acting as a marker for the rest of
/// #75's still-unimplemented isolation surface (resource limits, #76/
/// #77/#78/#79 build on this handle once it exists for real).
#[derive(Debug, Clone)]
pub struct SandboxHandle {
    token: CancellationToken,
}

impl Default for SandboxHandle {
    /// A handle with its own, independent (unparented) token — suitable
    /// for tests and as [`crate::context::RuntimeContext`]'s default
    /// before the Runtime attaches a real one via [`Self::isolated`].
    fn default() -> Self {
        Self {
            token: CancellationToken::new(),
        }
    }
}

impl SandboxHandle {
    /// Build a handle whose cancellation token is a child of `parent` —
    /// cancelling `parent` (e.g. the Runtime's overall shutdown token,
    /// #44) cancels this module's token too, but cancelling this
    /// module's token alone leaves `parent` and every other module's
    /// token untouched.
    #[must_use]
    pub fn isolated(parent: &CancellationToken) -> Self {
        Self {
            token: parent.child_token(),
        }
    }

    /// This module's own cancellation token (see this module's docs on
    /// why it's separate from the Runtime's overall one).
    #[must_use]
    pub fn cancellation_token(&self) -> &CancellationToken {
        &self.token
    }

    /// Request this module be isolated/stopped — cancels
    /// [`Self::cancellation_token`]. Doesn't affect the parent token
    /// this handle was built from (if any) or any other module's
    /// handle.
    pub fn cancel(&self) {
        self.token.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_handle_starts_uncancelled() {
        let handle = SandboxHandle::default();
        assert!(!handle.cancellation_token().is_cancelled());
    }

    #[test]
    fn cancelling_the_parent_cancels_an_isolated_child() {
        let parent = CancellationToken::new();
        let handle = SandboxHandle::isolated(&parent);

        parent.cancel();

        assert!(handle.cancellation_token().is_cancelled());
    }

    #[test]
    fn cancelling_one_handle_does_not_cancel_its_sibling() {
        let parent = CancellationToken::new();
        let a = SandboxHandle::isolated(&parent);
        let b = SandboxHandle::isolated(&parent);

        a.cancel();

        assert!(a.cancellation_token().is_cancelled());
        assert!(!b.cancellation_token().is_cancelled());
        assert!(!parent.is_cancelled());
    }

    #[test]
    fn cancelling_a_handle_directly_does_not_cancel_its_parent() {
        let parent = CancellationToken::new();
        let handle = SandboxHandle::isolated(&parent);

        handle.cancel();

        assert!(handle.cancellation_token().is_cancelled());
        assert!(!parent.is_cancelled());
    }
}
