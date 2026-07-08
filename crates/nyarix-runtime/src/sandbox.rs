//! Runtime-side Sandbox primitives (see issue #75): the parts of module
//! isolation that only the Runtime (not a marker handle in
//! `nyarix-module-api`, see [`nyarix_module_api::SandboxHandle`]'s own
//! docs for the rest of #75's bullets) can actually implement, because
//! they wrap *calls into* a module rather than anything a module itself
//! holds.
//!
//! [`catch_module_panic`] is #75's "Перехват паники (catch_unwind)":
//! a module panicking inside `initialize`/`process`/`shutdown` is
//! converted into an ordinary [`ModuleError::Crashed`] instead of
//! unwinding into (and potentially poisoning/aborting) the Runtime.
//! [`capability_enforcement::enforce_and_instantiate`] wraps
//! `initialize` (via [`nyarix_loader::instantiate`]) with it;
//! [`execution_loop::process_one`] wraps each `process` call the same
//! way.

use std::panic::{self, AssertUnwindSafe};

use nyarix_error::ModuleError;

/// Run `f` and convert a panic into `Err(reason)` — the low-level
/// primitive both [`catch_module_panic`] (which needs a
/// [`ModuleError::Crashed`] specifically) and
/// [`crate::execution_loop::process_one`] (which just logs and moves
/// on, matching how it already treats an ordinary
/// [`nyarix_error::GraphError`]) build on.
///
/// `f` is wrapped in [`AssertUnwindSafe`] rather than required to be
/// genuinely `UnwindSafe` — the whole point of a sandbox boundary is to
/// treat "the module left something in a weird state when it panicked"
/// as *expected*, not as a reason to refuse compiling the boundary
/// itself. Anything reachable through `f` that the panic could have
/// interrupted mid-mutation (a lock guard, `&mut` graph state, ...) is
/// exactly what this function exists to isolate the rest of the Runtime
/// from.
pub fn catch_panic<F, T>(f: F) -> Result<T, String>
where
    F: FnOnce() -> T,
{
    panic::catch_unwind(AssertUnwindSafe(f)).map_err(|payload| panic_message(&*payload))
}

/// Run `f` (some call into a module) and convert a panic into
/// [`ModuleError::Crashed`] instead of letting it unwind further.
///
/// # Errors
/// Returns whatever error `f` returns normally, or
/// [`ModuleError::Crashed`] (with `reason` taken from the panic payload
/// when it's a `&str`/`String`, else a generic message) if `f` panics.
pub fn catch_module_panic<F, T>(module_name: &str, f: F) -> Result<T, ModuleError>
where
    F: FnOnce() -> Result<T, ModuleError>,
{
    match catch_panic(f) {
        Ok(result) => result,
        Err(reason) => {
            tracing::warn!(module = %module_name, reason = %reason, "module panicked");
            Err(ModuleError::Crashed {
                name: module_name.to_string(),
                reason,
            })
        }
    }
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "module panicked with a non-string payload".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catch_panic_returns_ok_when_f_does_not_panic() {
        assert_eq!(catch_panic(|| 42), Ok(42));
    }

    #[test]
    fn catch_panic_returns_the_message_when_f_panics() {
        let result = catch_panic(|| -> () { panic!("boom") });
        assert_eq!(result, Err("boom".to_string()));
    }

    #[test]
    fn returns_the_inner_result_when_f_does_not_panic() {
        let result = catch_module_panic("ok-module", || Ok::<_, ModuleError>(42));
        assert_eq!(result.unwrap(), 42);
    }

    #[test]
    fn propagates_the_inner_error_when_f_returns_one_without_panicking() {
        let result = catch_module_panic("failing-module", || {
            Err::<(), ModuleError>(ModuleError::InitFailed {
                name: "failing-module".to_string(),
                reason: "bad config".to_string(),
            })
        });
        assert!(matches!(result, Err(ModuleError::InitFailed { .. })));
    }

    #[test]
    fn a_string_panic_is_converted_to_crashed_with_the_message() {
        let result = catch_module_panic("panicky-module", || -> Result<(), ModuleError> {
            panic!("everything is on fire");
        });

        let Err(ModuleError::Crashed { name, reason }) = result else {
            panic!("expected Crashed");
        };
        assert_eq!(name, "panicky-module");
        assert_eq!(reason, "everything is on fire");
    }

    #[test]
    fn a_non_string_panic_still_produces_a_readable_crashed_error() {
        let result = catch_module_panic("panicky-module", || -> Result<(), ModuleError> {
            panic::panic_any(42);
        });

        assert!(matches!(result, Err(ModuleError::Crashed { .. })));
    }
}
