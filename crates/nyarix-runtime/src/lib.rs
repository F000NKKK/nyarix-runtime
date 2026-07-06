//! Nyarix Runtime: the execution core of the platform.
//!
//! [`RuntimeHandle`] (#40) loads configuration; [`EventBus`] (#48) is a
//! live subsystem; [`execution_loop::run`] (#43) is the main packet loop,
//! tying together graph initialization, `execute_sequential`, and
//! graceful shutdown (#44, with draining and a forced-completion
//! timeout). The rest (module loader, dependency resolver, scheduler
//! thread pools, metrics) are still their own later milestone issues.

pub mod event;
pub mod execution_loop;
pub mod init;
pub mod shutdown;

pub use event::{Event, EventBus, EventFilter, EventKind};
pub use execution_loop::{
    initialize_all_nodes, run, run_with_timeout, shutdown_all_nodes, ExecutionLoopError,
    DEFAULT_SHUTDOWN_TIMEOUT,
};
pub use init::{RuntimeHandle, RuntimeInitError};
pub use shutdown::cancel_on_ctrl_c;
