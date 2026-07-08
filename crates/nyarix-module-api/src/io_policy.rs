//! I/O access policies (see issue #78): whitelists deciding which
//! filesystem paths and network host/port pairs a module is allowed to
//! reach through the Runtime-mediated API
//! ([`crate::context::RuntimeContext::check_file_access`]/
//! [`crate::context::RuntimeContext::check_network_access`]).
//!
//! Both default to **deny-all** ([`FilesystemPolicy::deny_all`]/
//! [`NetworkPolicy::deny_all`]) — an empty whitelist means nothing is
//! allowed, the same "declare what's granted, absence means denied"
//! convention as [`crate::capability::CapabilityMask::empty`].

use std::path::{Path, PathBuf};

/// A whitelist of filesystem path prefixes a module may access.
#[derive(Debug, Clone, Default)]
pub struct FilesystemPolicy {
    allowed_prefixes: Vec<PathBuf>,
}

impl FilesystemPolicy {
    /// No paths allowed — the default.
    #[must_use]
    pub fn deny_all() -> Self {
        Self::default()
    }

    /// Allow any path under one of `prefixes` (a path is allowed if it
    /// starts with at least one of them).
    #[must_use]
    pub fn allowing(prefixes: impl IntoIterator<Item = PathBuf>) -> Self {
        Self {
            allowed_prefixes: prefixes.into_iter().collect(),
        }
    }

    /// Whether `path` falls under one of the allowed prefixes.
    #[must_use]
    pub fn allows(&self, path: &Path) -> bool {
        self.allowed_prefixes
            .iter()
            .any(|prefix| path.starts_with(prefix))
    }
}

/// A whitelist of network destinations a module may connect to.
#[derive(Debug, Clone, Default)]
pub struct NetworkPolicy {
    allowed: Vec<(String, Option<u16>)>,
}

impl NetworkPolicy {
    /// No destinations allowed — the default.
    #[must_use]
    pub fn deny_all() -> Self {
        Self::default()
    }

    /// Allow connections to `host` on any port.
    #[must_use]
    pub fn allow_host(mut self, host: impl Into<String>) -> Self {
        self.allowed.push((host.into(), None));
        self
    }

    /// Allow connections to `host` on exactly `port`.
    #[must_use]
    pub fn allow_host_port(mut self, host: impl Into<String>, port: u16) -> Self {
        self.allowed.push((host.into(), Some(port)));
        self
    }

    /// Whether `host:port` is allowed — either an entry for `host` with
    /// no port restriction, or one matching this exact port.
    #[must_use]
    pub fn allows(&self, host: &str, port: u16) -> bool {
        self.allowed.iter().any(|(allowed_host, allowed_port)| {
            allowed_host == host && allowed_port.is_none_or(|allowed| allowed == port)
        })
    }

    /// Whether `host` has any whitelist entry at all, regardless of
    /// port — used to gate DNS resolution (#79's "DNS-резолв только
    /// через Runtime"), which has no port of its own to check against.
    #[must_use]
    pub fn allows_host(&self, host: &str) -> bool {
        self.allowed
            .iter()
            .any(|(allowed_host, _)| allowed_host == host)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deny_all_filesystem_policy_allows_nothing() {
        let policy = FilesystemPolicy::deny_all();
        assert!(!policy.allows(Path::new("/etc/passwd")));
        assert!(!policy.allows(Path::new("/")));
    }

    #[test]
    fn filesystem_policy_allows_paths_under_a_whitelisted_prefix() {
        let policy = FilesystemPolicy::allowing([PathBuf::from("/var/nyarix/data")]);
        assert!(policy.allows(Path::new("/var/nyarix/data/file.txt")));
        assert!(!policy.allows(Path::new("/etc/passwd")));
    }

    #[test]
    fn deny_all_network_policy_allows_nothing() {
        let policy = NetworkPolicy::deny_all();
        assert!(!policy.allows("example.com", 443));
    }

    #[test]
    fn network_policy_allows_a_host_on_any_port_when_no_port_specified() {
        let policy = NetworkPolicy::deny_all().allow_host("example.com");
        assert!(policy.allows("example.com", 443));
        assert!(policy.allows("example.com", 8080));
        assert!(!policy.allows("evil.example", 443));
    }

    #[test]
    fn network_policy_restricts_to_the_declared_port() {
        let policy = NetworkPolicy::deny_all().allow_host_port("example.com", 443);
        assert!(policy.allows("example.com", 443));
        assert!(!policy.allows("example.com", 8080));
    }

    #[test]
    fn allows_host_ignores_port_restrictions() {
        let policy = NetworkPolicy::deny_all().allow_host_port("example.com", 443);
        assert!(policy.allows_host("example.com"));
        assert!(!policy.allows_host("evil.example"));
    }
}
