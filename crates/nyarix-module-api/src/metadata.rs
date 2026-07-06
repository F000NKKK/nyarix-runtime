//! Module metadata (see issue #20).
//!
//! `ModuleMetadata` here is intentionally a minimal slice of the full shape
//! described in #20 — just enough for `Module::metadata()` (#16) to compile
//! and be useful. `required_capabilities`/`provided_capabilities` are now
//! filled in (#21). The rest (`dependencies`, `platforms`, `resource_limits`,
//! `sandbox_permissions`) still depend on types that don't exist yet (M5
//! dependency resolver, M7 Sandbox, M11 platform backends) — adding them
//! now would mean guessing their shape. Tracked in #20 to be filled in once
//! those land.

use crate::capability::Capability;

/// The functional category of a module.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
/// See the module-level docs for why this is a subset of the full #20 spec.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleMetadata {
    /// Module name, unique within its registry namespace.
    pub name: String,
    /// Module version.
    ///
    /// A plain string for now; #20 specifies `semver::Version`, which pulls
    /// in the `semver` crate and its own compatibility rules — deferred
    /// until version resolution (M5) actually consumes it.
    pub version: String,
    /// The functional category of this module.
    pub module_type: ModuleType,
    /// The Module API version this module was built against.
    ///
    /// A plain integer for now; full `ApiVersion` + compatibility checker
    /// is #25 (Module API versioning system).
    pub api_version: u32,
    /// Author name or organization.
    pub author: String,
    /// Human-readable description.
    pub description: String,
    /// Capabilities this module needs from the Runtime to function.
    pub required_capabilities: Vec<Capability>,
    /// Capabilities this module makes available to other modules.
    pub provided_capabilities: Vec<Capability>,
}

impl ModuleMetadata {
    /// Create metadata with the given name, version, and type; other fields
    /// default to empty.
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        version: impl Into<String>,
        module_type: ModuleType,
    ) -> Self {
        Self {
            name: name.into(),
            version: version.into(),
            module_type,
            api_version: 1,
            author: String::new(),
            description: String::new(),
            required_capabilities: Vec::new(),
            provided_capabilities: Vec::new(),
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::CapabilityMask;

    #[test]
    fn metadata_declares_capabilities() {
        let meta = ModuleMetadata::new("quic-transport", "0.1.0", ModuleType::Transport)
            .with_required_capabilities(vec![Capability::Network, Capability::Clock])
            .with_provided_capabilities(vec![Capability::Network]);

        assert_eq!(meta.required_capabilities.len(), 2);
        assert_eq!(meta.provided_capabilities, vec![Capability::Network]);

        let required_mask = CapabilityMask::from_capabilities(&meta.required_capabilities);
        let provided_mask = CapabilityMask::from_capabilities(&meta.provided_capabilities);
        assert!(!provided_mask.satisfies(required_mask));
    }
}
