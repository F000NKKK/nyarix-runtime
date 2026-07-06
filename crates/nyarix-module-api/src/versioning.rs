//! Module API versioning (see issue #25).

use std::fmt;

use serde::{Deserialize, Serialize};

/// The Module API version a module was built against, or that a Runtime
/// requires.
///
/// Semantics: `major` changes on breaking changes to the Module API;
/// `minor` changes on backward-compatible additions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ApiVersion {
    /// Incremented on breaking changes.
    pub major: u16,
    /// Incremented on backward-compatible additions.
    pub minor: u16,
}

impl ApiVersion {
    /// Create a new API version.
    #[must_use]
    pub const fn new(major: u16, minor: u16) -> Self {
        Self { major, minor }
    }
}

impl fmt::Display for ApiVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

/// A string failed to parse as an [`ApiVersion`] (see [`ApiVersion`]'s
/// `FromStr` impl).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid api_version {input:?}: expected \"major.minor\"")]
pub struct ApiVersionParseError {
    input: String,
}

impl std::str::FromStr for ApiVersion {
    type Err = ApiVersionParseError;

    /// Parse the `"major.minor"` form used in `manifest.toml`'s
    /// `api_version` field (#59) — the inverse of [`Self`]'s `Display`.
    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let invalid = || ApiVersionParseError {
            input: input.to_string(),
        };
        let (major, minor) = input.split_once('.').ok_or_else(invalid)?;
        let major = major.parse().map_err(|_| invalid())?;
        let minor = minor.parse().map_err(|_| invalid())?;
        Ok(Self::new(major, minor))
    }
}

/// Whether a module built against `provided` can run against a Runtime
/// that requires `required`.
///
/// `major` must match exactly (breaking changes aren't bridgeable);
/// `provided.minor` must be at least `required.minor` (a module built
/// against an older, backward-compatible minor version still works).
#[must_use]
pub fn is_compatible(required: ApiVersion, provided: ApiVersion) -> bool {
    required.major == provided.major && provided.minor >= required.minor
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_match_is_compatible() {
        assert!(is_compatible(ApiVersion::new(1, 2), ApiVersion::new(1, 2)));
    }

    #[test]
    fn newer_minor_is_compatible() {
        assert!(is_compatible(ApiVersion::new(1, 2), ApiVersion::new(1, 5)));
    }

    #[test]
    fn older_minor_is_incompatible() {
        assert!(!is_compatible(ApiVersion::new(1, 5), ApiVersion::new(1, 2)));
    }

    #[test]
    fn different_major_is_incompatible() {
        assert!(!is_compatible(ApiVersion::new(1, 0), ApiVersion::new(2, 0)));
    }

    #[test]
    fn display_format() {
        assert_eq!(ApiVersion::new(1, 2).to_string(), "1.2");
    }
}
