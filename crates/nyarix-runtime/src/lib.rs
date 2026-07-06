//! Nyarix Runtime: the execution core of the platform.
//!
//! [`RuntimeHandle`] (#40) loads configuration; [`EventBus`] (#48) is the
//! first live subsystem. The rest (module loader, dependency resolver,
//! scheduler, metrics) are still their own later milestone issues.

pub mod event;
pub mod init;

pub use event::{Event, EventBus, EventFilter, EventKind};
pub use init::{RuntimeHandle, RuntimeInitError};
