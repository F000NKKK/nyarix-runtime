//! Fallback resolution strategy (see issue #26).
//!
//! This is the pure decision logic only — it does not look modules up
//! anywhere. Actual module discovery/instantiation from the resolved name
//! is the Module Loader's job (M5, see #50 Module discovery, #57 Module
//! instantiation), which doesn't exist yet.

use crate::versioning::{ApiVersion, is_compatible};

/// The outcome of resolving a module load request against the modules
/// actually available.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    /// A candidate with exactly the required API version was found.
    ExactMatch(String),
    /// No exact match, but a backward-compatible candidate was found.
    CompatibleVersion(String),
    /// No compatible candidate; falling back to the manifest-declared
    /// alternative.
    Fallback(String),
    /// No compatible candidate and no fallback configured — the Loader
    /// must refuse to load.
    Incompatible,
}

/// Resolve which module to load, given the Runtime's required API version,
/// the available candidates (name + the API version each was built
/// against), and an optional fallback module name from the manifest.
///
/// Priority, per the issue spec: exact match → compatible version →
/// fallback → error.
#[must_use]
pub fn resolve(
    required: ApiVersion,
    candidates: &[(String, ApiVersion)],
    fallback: Option<&str>,
) -> Resolution {
    if let Some((name, _)) = candidates
        .iter()
        .find(|(_, provided)| *provided == required)
    {
        return Resolution::ExactMatch(name.clone());
    }

    if let Some((name, _)) = candidates
        .iter()
        .find(|(_, provided)| is_compatible(required, *provided))
    {
        return Resolution::CompatibleVersion(name.clone());
    }

    match fallback {
        Some(name) => Resolution::Fallback(name.to_string()),
        None => Resolution::Incompatible,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidates() -> Vec<(String, ApiVersion)> {
        vec![
            ("quic-v1".to_string(), ApiVersion::new(1, 0)),
            ("quic-v1-compat".to_string(), ApiVersion::new(1, 2)),
        ]
    }

    #[test]
    fn prefers_exact_match_over_compatible() {
        let outcome = resolve(ApiVersion::new(1, 0), &candidates(), Some("legacy-quic"));
        assert_eq!(outcome, Resolution::ExactMatch("quic-v1".to_string()));
    }

    #[test]
    fn falls_back_to_compatible_version() {
        let only_newer = vec![("quic-v1-compat".to_string(), ApiVersion::new(1, 2))];
        let outcome = resolve(ApiVersion::new(1, 0), &only_newer, Some("legacy-quic"));
        assert_eq!(
            outcome,
            Resolution::CompatibleVersion("quic-v1-compat".to_string())
        );
    }

    #[test]
    fn falls_back_to_manifest_fallback_when_incompatible() {
        let incompatible = vec![("quic-v2".to_string(), ApiVersion::new(2, 0))];
        let outcome = resolve(ApiVersion::new(1, 0), &incompatible, Some("legacy-quic"));
        assert_eq!(outcome, Resolution::Fallback("legacy-quic".to_string()));
    }

    #[test]
    fn errors_when_incompatible_and_no_fallback() {
        let incompatible = vec![("quic-v2".to_string(), ApiVersion::new(2, 0))];
        let outcome = resolve(ApiVersion::new(1, 0), &incompatible, None);
        assert_eq!(outcome, Resolution::Incompatible);
    }

    #[test]
    fn errors_when_no_candidates_and_no_fallback() {
        let outcome = resolve(ApiVersion::new(1, 0), &[], None);
        assert_eq!(outcome, Resolution::Incompatible);
    }
}
