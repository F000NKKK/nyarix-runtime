//! Public API contract that all Nyarix modules implement.
//!
//! The core of this crate is the [`Module`] trait: the minimal lifecycle
//! contract (`metadata`, `initialize`, `process`, `shutdown`, ...) that
//! every module — transport, crypto, obfuscation, policy, observability —
//! implements identically. The Runtime only ever talks to `dyn Module`.

pub mod capability;
pub mod config;
pub mod context;
pub mod event;
pub mod fallback;
pub mod health;
pub mod io_policy;
pub mod metadata;
pub mod metrics;
mod module;
mod node;
pub mod platform;
pub mod rate_limiter;
pub mod resource_limits;
pub mod sandbox;
pub mod sandbox_permission;
pub mod versioning;

pub use capability::{Capability, CapabilityGrant, CapabilityMask};
pub use config::ModuleConfig;
pub use context::RuntimeContext;
pub use event::{Event, EventBus, EventFilter, EventHandler, EventKind};
pub use fallback::{Resolution, resolve};
pub use health::Health;
pub use io_policy::{FilesystemPolicy, NetworkPolicy};
pub use metadata::{ModuleMetadata, ModuleType};
pub use metrics::{Counter, Gauge, Histogram, HistogramSnapshot, MetricRegistry, MetricsHandle};
pub use module::{Module, Result};
pub use node::{Node, NodeType};
pub use platform::Platform;
pub use rate_limiter::RateLimiter;
pub use resource_limits::ResourceLimits;
pub use sandbox::SandboxHandle;
pub use sandbox_permission::{AccessMode, SandboxPermission};
pub use versioning::{ApiVersion, is_compatible};
