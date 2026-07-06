//! Module metadata (see issue #20).
//!
//! `ModuleMetadata` here is intentionally a minimal slice of the full shape
//! described in #20 — just enough for `Module::metadata()` (#16) to compile
//! and be useful. The remaining fields (`required_capabilities`,
//! `provided_capabilities`, `dependencies`, `platforms`, `resource_limits`,
//! `sandbox_permissions`) depend on types that don't exist yet (#21
//! Capability model, M5 dependency resolver, M7 Sandbox, M11 platform
//! backends) — adding them now would mean guessing their shape. Tracked in
//! #20 to be filled in once those land.

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
        }
    }
}
