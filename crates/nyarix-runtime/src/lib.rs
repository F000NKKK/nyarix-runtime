//! Nyarix Runtime: the execution core of the platform.
//!
//! [`RuntimeHandle`] (#40) is the first piece: loading configuration and
//! holding the slot for the subsystems that tie the platform together
//! (module loader, dependency resolver, scheduler, event bus, metrics —
//! each its own later milestone issue).

pub mod init;

pub use init::{RuntimeHandle, RuntimeInitError};
