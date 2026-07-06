//! Public API contract that all Nyarix modules implement.
//!
//! The core of this crate is the [`Module`] trait: the minimal lifecycle
//! contract (`metadata`, `initialize`, `process`, `shutdown`, ...) that
//! every module — transport, crypto, obfuscation, policy, observability —
//! implements identically. The Runtime only ever talks to `dyn Module`.

pub mod context;
pub mod health;
pub mod metadata;
mod module;
mod node;

pub use context::RuntimeContext;
pub use health::Health;
pub use metadata::{ModuleMetadata, ModuleType};
pub use module::{Module, Result};
pub use node::{Node, NodeType};
