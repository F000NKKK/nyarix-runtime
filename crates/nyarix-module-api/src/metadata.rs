//! Module metadata (see issue #20).
//!
//! `ModuleMetadata` here is still not the *complete* #20 shape: `dependencies`
//! and `sandbox_permissions` remain deferred. The dependency-matching syntax
//! this comment used to say was still an open question is now decided —
//! `nyarix_package::manifest::DependencySpec` (#59/#56: a semver
//! requirement plus an `optional` flag) — but adding a `dependencies`
//! field here still isn't done: nothing yet consumes it from
//! `ModuleMetadata` itself (Module instantiation, #57, is the earliest
//! candidate), so it would be a guess which crate should own the type and
//! whether `ModuleMetadata` needs its own copy at all. `sandbox_permissions`
//! has no settled taxonomy anywhere in the backlog yet (unlike
//! capabilities, which #21 fully specifies) — it needs Sandbox design
//! (M7, #75) before its shape can be more than a guess.

use serde::{Deserialize, Serialize};

use crate::capability::Capability;
use crate::platform::Platform;
use crate::resource_limits::ResourceLimits;
use crate::versioning::ApiVersion;

/// The functional category of a module.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModuleType {
    /// Delivers packets between parties (QUIC, UDP, TCP, WebSocket, ...).
    Transport,
    /// Encryption, key exchange, authentication, rekeying.
    Crypto,
    /// Transforms the packet as a graph object (routing, fragmentation, ...).
    Flow,
    /// Masks traffic shape, timing, or structure.
    Obfuscation,
    /// Makes decisions (path selection, fallback, padding amount, ...).
    Policy,
    /// Collects metrics without altering payload traffic.
    Observability,
}

/// Metadata describing a module.
///
/// See the module-level docs for the two fields still missing relative to
/// the full #20 spec.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleMetadata {
    /// Module name, unique within its registry namespace.
    pub name: String,
    /// Module version.
    pub version: semver::Version,
    /// The functional category of this module.
    pub module_type: ModuleType,
    /// The Module API version this module was built against (see #25).
    pub api_version: ApiVersion,
    /// Author name or organization.
    pub author: String,
    /// Human-readable description.
    pub description: String,
    /// Capabilities this module needs from the Runtime to function.
    pub required_capabilities: Vec<Capability>,
    /// System capabilities this module makes available to other modules
    /// (the closed [`Capability`] enum, #21 — a Sandbox-granted
    /// permission, not a feature tag).
    pub provided_capabilities: Vec<Capability>,
    /// Free-form feature/service tags this module advertises to other
    /// modules (e.g. `"transport-udp"`), for the Dependency resolver
    /// (#53) to match other modules' declared dependencies against.
    ///
    /// Deliberately a separate field from [`Self::provided_capabilities`]:
    /// package manifests (#59) can declare arbitrary tags here that will
    /// never be system [`Capability`] variants — see
    /// `nyarix_package::manifest::Capabilities`'s doc comment for the
    /// full reasoning (#104).
    pub provided_tags: Vec<String>,
    /// Platforms this module supports. Empty means "unspecified" (assume
    /// all platforms) rather than "supports no platforms".
    pub platforms: Vec<Platform>,
    /// Resource limits this module declares for itself.
    pub resource_limits: ResourceLimits,
}

impl ModuleMetadata {
    /// Create metadata with the given name, version, and type; other fields
    /// default to empty/unbounded.
    #[must_use]
    pub fn new(name: impl Into<String>, version: semver::Version, module_type: ModuleType) -> Self {
        Self {
            name: name.into(),
            version,
            module_type,
            api_version: ApiVersion::new(1, 0),
            author: String::new(),
            description: String::new(),
            required_capabilities: Vec::new(),
            provided_capabilities: Vec::new(),
            provided_tags: Vec::new(),
            platforms: Vec::new(),
            resource_limits: ResourceLimits::unbounded(),
        }
    }

    /// Set the capabilities this module requires from the Runtime.
    #[must_use]
    pub fn with_required_capabilities(mut self, capabilities: impl Into<Vec<Capability>>) -> Self {
        self.required_capabilities = capabilities.into();
        self
    }

    /// Set the capabilities this module provides to other modules.
    #[must_use]
    pub fn with_provided_capabilities(mut self, capabilities: impl Into<Vec<Capability>>) -> Self {
        self.provided_capabilities = capabilities.into();
        self
    }

    /// [`Self::required_capabilities`] as a [`CapabilityMask`] (#70/#91)
    /// — the form the Runtime's grant/enforcement checks
    /// (`CapabilityMask::satisfies`/[`crate::capability::CapabilityGrant`])
    /// actually operate on, computed here so callers don't each redo
    /// `CapabilityMask::from_capabilities(&metadata.required_capabilities)`
    /// by hand.
    #[must_use]
    pub fn required_capabilities_mask(&self) -> crate::capability::CapabilityMask {
        crate::capability::CapabilityMask::from_capabilities(&self.required_capabilities)
    }

    /// Set the feature/service tags this module advertises to other
    /// modules.
    #[must_use]
    pub fn with_provided_tags(mut self, tags: impl Into<Vec<String>>) -> Self {
        self.provided_tags = tags.into();
        self
    }

    /// Set the platforms this module supports.
    #[must_use]
    pub fn with_platforms(mut self, platforms: impl Into<Vec<Platform>>) -> Self {
        self.platforms = platforms.into();
        self
    }

    /// Set this module's declared resource limits.
    #[must_use]
    pub const fn with_resource_limits(mut self, limits: ResourceLimits) -> Self {
        self.resource_limits = limits;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::CapabilityMask;

    fn version(text: &str) -> semver::Version {
        semver::Version::parse(text).unwrap()
    }

    #[test]
    fn metadata_declares_capabilities() {
        let meta = ModuleMetadata::new("quic-transport", version("0.1.0"), ModuleType::Transport)
            .with_required_capabilities(vec![Capability::Network, Capability::Clock])
            .with_provided_capabilities(vec![Capability::Network]);

        assert_eq!(meta.required_capabilities.len(), 2);
        assert_eq!(meta.provided_capabilities, vec![Capability::Network]);

        let required_mask = CapabilityMask::from_capabilities(&meta.required_capabilities);
        let provided_mask = CapabilityMask::from_capabilities(&meta.provided_capabilities);
        assert!(!provided_mask.satisfies(required_mask));
    }

    #[test]
    fn metadata_declares_provided_tags_separately_from_capabilities() {
        let meta = ModuleMetadata::new("udp-transport", version("0.1.0"), ModuleType::Transport)
            .with_provided_capabilities(vec![Capability::Network])
            .with_provided_tags(vec!["transport-udp".to_string()]);

        assert_eq!(meta.provided_capabilities, vec![Capability::Network]);
        assert_eq!(meta.provided_tags, vec!["transport-udp".to_string()]);
    }

    #[test]
    fn metadata_declares_platforms_and_limits() {
        let meta = ModuleMetadata::new("tun-bridge", version("0.2.0"), ModuleType::Transport)
            .with_platforms(vec![Platform::Linux, Platform::MacOs])
            .with_resource_limits(ResourceLimits {
                max_memory_bytes: Some(64 * 1024 * 1024),
                ..ResourceLimits::unbounded()
            });

        assert_eq!(meta.platforms, vec![Platform::Linux, Platform::MacOs]);
        assert_eq!(
            meta.resource_limits.max_memory_bytes,
            Some(64 * 1024 * 1024)
        );
        assert_eq!(meta.resource_limits.max_cpu_percent, None);
    }

    #[test]
    fn default_api_version_is_1_0() {
        let meta = ModuleMetadata::new("noop", version("0.1.0"), ModuleType::Observability);
        assert_eq!(meta.api_version, ApiVersion::new(1, 0));
    }
}
