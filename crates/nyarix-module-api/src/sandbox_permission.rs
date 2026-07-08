//! Module-declared sandbox permissions (see issue #93).
//!
//! Distinct from [`crate::capability::Capability`] (#21): `Capability`
//! says *whether* a module needs filesystem/network access at all, as
//! a coarse category checked against what the Runtime granted
//! ([`crate::context::RuntimeContext::request_capability`]).
//! [`SandboxPermission`] says specifically *which* paths/hosts a
//! module intends to use — the parameterized detail #75's Sandbox
//! design (now landed as [`crate::io_policy::FilesystemPolicy`]/
//! [`crate::io_policy::NetworkPolicy`], #78) needed to exist first.
//!
//! **Scope note:** this is the declaration only, mirroring how
//! [`crate::metadata::ModuleMetadata::required_capabilities`] is a
//! declaration that [`crate::context::RuntimeContext::request_capability`]
//! separately enforces. Nothing here automatically builds a
//! [`crate::io_policy::FilesystemPolicy`]/[`crate::io_policy::NetworkPolicy`]
//! out of a module's declared permissions — deciding whether the
//! Runtime's policy should default to *exactly* what a module declares,
//! a subset, or an independently-configured allowlist is a real policy
//! decision (does a module get to self-grant just by declaring?), not
//! something to guess at here.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// How a declared filesystem path may be accessed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AccessMode {
    /// Read-only access.
    Read,
    /// Write-only access.
    Write,
    /// Both read and write access.
    ReadWrite,
}

/// One permission a module declares it needs, beyond the coarse
/// [`crate::capability::Capability`] category.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SandboxPermission {
    /// Access to a specific filesystem path (or, per
    /// [`crate::io_policy::FilesystemPolicy::allows`]'s prefix
    /// semantics, everything under it).
    FilesystemPath {
        /// The path (or path prefix) this module needs.
        path: PathBuf,
        /// How it needs to access it.
        mode: AccessMode,
    },
    /// Access to a specific network destination.
    NetworkAddress {
        /// The host (hostname or IP) this module needs to reach.
        host: String,
        /// The specific port needed, or `None` for any port on `host`
        /// (same convention as [`crate::io_policy::NetworkPolicy::allow_host`]
        /// vs. [`crate::io_policy::NetworkPolicy::allow_host_port`]).
        port: Option<u16>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filesystem_path_permission_round_trips_through_serde() {
        let permission = SandboxPermission::FilesystemPath {
            path: PathBuf::from("/var/nyarix/data"),
            mode: AccessMode::ReadWrite,
        };
        let text = toml::to_string(&permission).unwrap();
        let parsed: SandboxPermission = toml::from_str(&text).unwrap();
        assert_eq!(parsed, permission);
    }

    #[test]
    fn network_address_permission_round_trips_through_serde() {
        let permission = SandboxPermission::NetworkAddress {
            host: "example.com".to_string(),
            port: Some(443),
        };
        let text = toml::to_string(&permission).unwrap();
        let parsed: SandboxPermission = toml::from_str(&text).unwrap();
        assert_eq!(parsed, permission);
    }

    #[test]
    fn distinct_variants_are_not_equal() {
        let filesystem = SandboxPermission::FilesystemPath {
            path: PathBuf::from("/tmp"),
            mode: AccessMode::Read,
        };
        let network = SandboxPermission::NetworkAddress {
            host: "example.com".to_string(),
            port: None,
        };
        assert_ne!(filesystem, network);
    }
}
