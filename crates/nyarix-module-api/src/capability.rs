//! Capability declaration model (see issue #21).
//!
//! A module declares what it needs (`required_capabilities`) and what it
//! grants to others (`provided_capabilities`) as a `Vec<Capability>` — the
//! readable, serializable form used in [`crate::metadata::ModuleMetadata`]
//! and package manifests. [`CapabilityMask`] is the corresponding bitmask,
//! for O(1) checks once a module is loaded.
//!
//! [`CapabilityGrant`]/[`CapabilityGrant::request`] (#70) computes which
//! of a module's required capabilities the Runtime's policy actually
//! grants — [`crate::context::RuntimeContext::request_capabilities`] is
//! the module-facing entry point.
//!
//! **Still out of scope here:** *enforcing* the result (killing/degrading
//! a module that didn't get everything it asked for) — that's #73
//! (Runtime enforcement) and #74 (denied capability handling); this
//! only decides what's granted, not what happens if it's incomplete.

use bitflags::bitflags;
use serde::{Deserialize, Serialize};

/// A single capability a module may require or provide.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Capability {
    /// Read/write access to the filesystem.
    Filesystem,
    /// Open network sockets.
    Network,
    /// Create/read/write a TUN device.
    Tun,
    /// Access to a cryptographically secure RNG.
    Random,
    /// Perform cryptographic operations (encrypt/decrypt/sign/verify).
    Crypto,
    /// Read the system clock.
    Clock,
    /// Schedule background tasks on the Runtime's worker pools.
    Scheduler,
    /// Persistent key/value or blob storage.
    Storage,
    /// Publish metrics.
    Metrics,
    /// Perform DNS resolution.
    Dns,
    /// Register UI hooks/extension points.
    UiHooks,
    /// Run in the background (mobile-relevant: survive app suspension).
    Background,
    /// Bind privileged (low-numbered) sockets.
    PrivilegedSockets,
}

impl Capability {
    /// All capability variants, in declaration order.
    pub const ALL: [Self; 13] = [
        Self::Filesystem,
        Self::Network,
        Self::Tun,
        Self::Random,
        Self::Crypto,
        Self::Clock,
        Self::Scheduler,
        Self::Storage,
        Self::Metrics,
        Self::Dns,
        Self::UiHooks,
        Self::Background,
        Self::PrivilegedSockets,
    ];

    /// The single-bit mask corresponding to this capability.
    #[must_use]
    pub const fn mask(self) -> CapabilityMask {
        match self {
            Self::Filesystem => CapabilityMask::FILESYSTEM,
            Self::Network => CapabilityMask::NETWORK,
            Self::Tun => CapabilityMask::TUN,
            Self::Random => CapabilityMask::RANDOM,
            Self::Crypto => CapabilityMask::CRYPTO,
            Self::Clock => CapabilityMask::CLOCK,
            Self::Scheduler => CapabilityMask::SCHEDULER,
            Self::Storage => CapabilityMask::STORAGE,
            Self::Metrics => CapabilityMask::METRICS,
            Self::Dns => CapabilityMask::DNS,
            Self::UiHooks => CapabilityMask::UI_HOOKS,
            Self::Background => CapabilityMask::BACKGROUND,
            Self::PrivilegedSockets => CapabilityMask::PRIVILEGED_SOCKETS,
        }
    }
}

bitflags! {
    /// Bitmask form of a set of [`Capability`] values, for fast checks
    /// (e.g. "does this module's granted mask cover what it requires?")
    /// without walking a `Vec<Capability>`.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
    pub struct CapabilityMask: u16 {
        /// See [`Capability::Filesystem`].
        const FILESYSTEM = 1 << 0;
        /// See [`Capability::Network`].
        const NETWORK = 1 << 1;
        /// See [`Capability::Tun`].
        const TUN = 1 << 2;
        /// See [`Capability::Random`].
        const RANDOM = 1 << 3;
        /// See [`Capability::Crypto`].
        const CRYPTO = 1 << 4;
        /// See [`Capability::Clock`].
        const CLOCK = 1 << 5;
        /// See [`Capability::Scheduler`].
        const SCHEDULER = 1 << 6;
        /// See [`Capability::Storage`].
        const STORAGE = 1 << 7;
        /// See [`Capability::Metrics`].
        const METRICS = 1 << 8;
        /// See [`Capability::Dns`].
        const DNS = 1 << 9;
        /// See [`Capability::UiHooks`].
        const UI_HOOKS = 1 << 10;
        /// See [`Capability::Background`].
        const BACKGROUND = 1 << 11;
        /// See [`Capability::PrivilegedSockets`].
        const PRIVILEGED_SOCKETS = 1 << 12;
    }
}

impl CapabilityMask {
    /// Build a mask from a set of capabilities.
    #[must_use]
    pub fn from_capabilities(capabilities: &[Capability]) -> Self {
        capabilities
            .iter()
            .fold(Self::empty(), |mask, cap| mask | cap.mask())
    }

    /// Check whether a single capability is set.
    #[must_use]
    pub fn has(self, capability: Capability) -> bool {
        self.contains(capability.mask())
    }

    /// Check whether every capability in `required` is present in `self`
    /// (i.e. `self` was granted at least as much as `required` asks for).
    #[must_use]
    pub fn satisfies(self, required: Self) -> bool {
        self.contains(required)
    }
}

/// The result of a module requesting its capabilities from the Runtime
/// (#70): which of what it asked for it actually got, and which were
/// denied by the current security policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityGrant {
    /// The capabilities actually granted — always a subset of what was
    /// requested, never more.
    pub granted: CapabilityMask,
    /// Requested capabilities the policy did not grant, in the order
    /// they were requested.
    pub denied: Vec<Capability>,
}

impl CapabilityGrant {
    /// Compute a [`CapabilityGrant`] for `required` against `granted` —
    /// the Runtime's policy result for this module, already reduced to
    /// a single mask by whatever produced `granted` (the policy engine
    /// itself, #73, isn't this function's concern).
    #[must_use]
    pub fn request(required: &[Capability], granted: CapabilityMask) -> Self {
        let denied = required
            .iter()
            .copied()
            .filter(|capability| !granted.has(*capability))
            .collect();
        Self {
            granted: CapabilityMask::from_capabilities(required) & granted,
            denied,
        }
    }

    /// Whether every requested capability was granted.
    #[must_use]
    pub fn is_fully_granted(&self) -> bool {
        self.denied.is_empty()
    }

    /// Whether `capability` specifically was granted.
    #[must_use]
    pub fn has(&self, capability: Capability) -> bool {
        self.granted.has(capability)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_as_lowercase_matching_manifest_schema() {
        #[derive(Debug, PartialEq, Serialize, Deserialize)]
        struct Wrapper {
            required: Vec<Capability>,
        }

        // manifest.toml's `[capabilities] required = ["network"]` (#59).
        let parsed: Wrapper = toml::from_str("required = [\"network\"]").unwrap();
        assert_eq!(parsed.required, vec![Capability::Network]);
    }

    #[test]
    fn single_capability_round_trips_through_mask() {
        let mask = Capability::Network.mask();
        assert!(mask.has(Capability::Network));
        assert!(!mask.has(Capability::Filesystem));
    }

    #[test]
    fn from_capabilities_combines_bits() {
        let mask = CapabilityMask::from_capabilities(&[Capability::Network, Capability::Crypto]);
        assert!(mask.has(Capability::Network));
        assert!(mask.has(Capability::Crypto));
        assert!(!mask.has(Capability::Tun));
    }

    #[test]
    fn satisfies_checks_superset() {
        let granted = CapabilityMask::from_capabilities(&[
            Capability::Network,
            Capability::Crypto,
            Capability::Clock,
        ]);
        let required =
            CapabilityMask::from_capabilities(&[Capability::Network, Capability::Crypto]);
        let too_much = CapabilityMask::from_capabilities(&[Capability::Network, Capability::Tun]);

        assert!(granted.satisfies(required));
        assert!(!granted.satisfies(too_much));
    }

    #[test]
    fn all_capabilities_have_distinct_bits() {
        let combined = CapabilityMask::from_capabilities(&Capability::ALL);
        let bit_count = combined.bits().count_ones();
        assert_eq!(bit_count as usize, Capability::ALL.len());
    }

    #[test]
    fn a_request_fully_within_the_granted_mask_is_fully_granted() {
        let granted =
            CapabilityMask::from_capabilities(&[Capability::Network, Capability::Crypto]);
        let request = CapabilityGrant::request(&[Capability::Network], granted);

        assert!(request.is_fully_granted());
        assert!(request.has(Capability::Network));
        assert!(request.denied.is_empty());
    }

    #[test]
    fn a_request_beyond_the_granted_mask_lists_what_was_denied() {
        let granted = CapabilityMask::from_capabilities(&[Capability::Network]);
        let request =
            CapabilityGrant::request(&[Capability::Network, Capability::Tun], granted);

        assert!(!request.is_fully_granted());
        assert!(request.has(Capability::Network));
        assert!(!request.has(Capability::Tun));
        assert_eq!(request.denied, vec![Capability::Tun]);
    }

    #[test]
    fn granted_mask_never_exceeds_what_was_requested() {
        // Granted has Crypto too, but it wasn't requested, so it
        // shouldn't show up in the resulting grant.
        let granted =
            CapabilityMask::from_capabilities(&[Capability::Network, Capability::Crypto]);
        let request = CapabilityGrant::request(&[Capability::Network], granted);

        assert_eq!(
            request.granted,
            CapabilityMask::from_capabilities(&[Capability::Network])
        );
        assert!(!request.granted.has(Capability::Crypto));
    }

    #[test]
    fn requesting_nothing_is_trivially_fully_granted() {
        let request = CapabilityGrant::request(&[], CapabilityMask::empty());
        assert!(request.is_fully_granted());
    }
}
