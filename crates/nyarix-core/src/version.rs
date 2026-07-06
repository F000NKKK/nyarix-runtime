//! Version types for semantic versioning and API compatibility.

use std::fmt;

/// A semantic version (major.minor.patch).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SemVer {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl SemVer {
    /// Create a new semantic version.
    #[must_use]
    pub const fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }

    /// Check if `other` is compatible with `self` according to SemVer rules.
    /// Compatible means same major, other.minor >= self.minor.
    #[must_use]
    pub fn is_compatible_with(&self, required: &SemVer) -> bool {
        self.major == required.major && self.minor >= required.minor
    }
}

impl fmt::Display for SemVer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// The API version that a module targets.
/// Each API domain (Runtime, Node, Transport, etc.) has its own version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiVersion {
    /// The domain of the API (e.g., "runtime", "node", "transport")
    pub domain: String,
    /// The semantic version of the API
    pub version: SemVer,
}

impl ApiVersion {
    /// Create a new API version.
    #[must_use]
    pub fn new(domain: impl Into<String>, major: u32, minor: u32, patch: u32) -> Self {
        Self {
            domain: domain.into(),
            version: SemVer::new(major, minor, patch),
        }
    }

    /// Check if this API version satisfies the required version.
    #[must_use]
    pub fn satisfies(&self, required: &ApiVersion) -> bool {
        self.domain == required.domain && self.version.is_compatible_with(&required.version)
    }
}
