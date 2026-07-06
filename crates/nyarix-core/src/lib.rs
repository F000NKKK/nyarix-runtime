//! Nyarix core types and utilities shared across the platform.
//!
//! This crate provides fundamental types that are used throughout the
//! Nyarix ecosystem: identifiers, version types, platform abstractions,
//! and common utilities.

pub mod id;
pub mod platform;
pub mod version;

// Re-export commonly used types
pub use id::*;
pub use platform::*;
pub use version::*;
