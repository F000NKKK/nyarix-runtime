//! Nyarix Runtime: the execution core of the platform.
//!
//! [`RuntimeHandle`] (#40) loads configuration; [`EventBus`] (#48,
//! re-exported from [`nyarix_module_api`] since #49 moved it there so
//! [`nyarix_module_api::RuntimeContext`] could hold a real one) is a live
//! subsystem; [`execution_loop::run`] (#43) is the main packet loop,
//! tying together graph initialization, `execute_sequential`, and
//! graceful shutdown (#44, with draining and a forced-completion
//! timeout). [`IoPool`] (#45) and [`CpuPool`] (#46) are the Scheduler's
//! two worker pools, isolating I/O-bound and CPU-bound work from each
//! other, and [`TaskPriority`] / [`priority::priority_queue`] (#47) is
//! the Scheduler's priority model and queue primitive. The rest (module
//! loader, dependency resolver, metrics) are still their own later
//! milestone issues.

pub mod cpu_pool;
pub mod execution_loop;
pub mod init;
pub mod io_pool;
pub mod priority;
pub mod shutdown;

pub use cpu_pool::CpuPool;
pub use execution_loop::{
    DEFAULT_SHUTDOWN_TIMEOUT, ExecutionLoopError, initialize_all_nodes, run, run_with_timeout,
    shutdown_all_nodes,
};
pub use init::{RuntimeHandle, RuntimeInitError};
pub use io_pool::IoPool;
pub use nyarix_module_api::{Event, EventBus, EventFilter, EventKind};
pub use priority::{PriorityReceiver, PrioritySender, TaskPriority, priority_queue};
pub use shutdown::cancel_on_ctrl_c;
