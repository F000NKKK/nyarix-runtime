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
//! the Scheduler's priority model and queue primitive.
//! [`module_loader::load_modules`] (#41) scans and validates `.nyp`
//! packages (M5/M6, #50-#66) — everything up to but not including
//! actually instantiating a module, which needs #107.
//! [`graph_builder::build_from_profile`] (#42) builds a [`FlowGraph`]
//! from a profile's module stack, given a name→node lookup (see that
//! module's scope note). The rest (dependency resolver wiring into the
//! graph, metrics) are still their own later milestone issues.

pub mod cpu_pool;
pub mod execution_loop;
pub mod graph_builder;
pub mod init;
pub mod io_pool;
pub mod module_loader;
pub mod priority;
pub mod shutdown;

pub use cpu_pool::CpuPool;
pub use execution_loop::{
    DEFAULT_SHUTDOWN_TIMEOUT, ExecutionLoopError, initialize_all_nodes, run, run_with_timeout,
    shutdown_all_nodes,
};
pub use graph_builder::{BuildError, build_from_profile};
pub use init::{RuntimeHandle, RuntimeInitError};
pub use io_pool::IoPool;
pub use module_loader::{LoadedPackage, ModuleLoadReport, RejectedPackage, load_modules};
pub use nyarix_graph::FlowGraph;
pub use nyarix_module_api::{Event, EventBus, EventFilter, EventKind};
pub use priority::{PriorityReceiver, PrioritySender, TaskPriority, priority_queue};
pub use shutdown::cancel_on_ctrl_c;
