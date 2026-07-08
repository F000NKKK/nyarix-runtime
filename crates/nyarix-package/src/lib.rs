//! The `.nyp` (Nyarix Package) format (see issue #58).
//!
//! A `.nyp` file is a `tar` archive, `zstd`-compressed, containing:
//! - `manifest.toml` — package metadata, dependencies, capabilities (#59)
//! - `payload/` — the module's compiled code (wasm or native)
//! - `assets/` — optional bundled non-code resources
//! - `signatures/` — optional Ed25519 signatures (#61) over the rest of
//!   the archive, absent for unsigned/`unknown`-trust packages (#63)
//!
//! **Scope note:** this crate defines the format's *structural contract*
//! ([`validate_layout`]), parses `manifest.toml` ([`manifest::PackageManifest`],
//! #59), packs/unpacks the archive itself ([`archive::PackageBuilder`]/
//! [`archive::PackageReader`], #60), signs it with Ed25519 ([`signing`],
//! #61), and checks a package's embedded signature on load
//! ([`archive::PackageReader::signature_status`]/`require_valid_signature`,
//! #62). It deliberately does **not** implement trust levels for which
//! public keys to actually accept (#63) — `signature_status` only proves
//! a signature is valid, not that its key is trusted.

pub mod archive;
pub mod manifest;
pub mod signing;

pub use archive::{PackageBuilder, PackageReader, SignatureStatus};
pub use manifest::{Capabilities, PackageInfo, PackageManifest, Platforms};
pub use signing::{Signature, SignatureVerificationFailed, SigningKey, VerifyingKey};

use nyarix_error::PackageError;

/// The `.nyp` container format's own version — how the four top-level
/// members are laid out and named, *not* an individual package's
/// `[package] version` (that's a per-package `semver::Version` in
/// `manifest.toml`, #59's concern).
///
/// A plain incrementing integer rather than semver: this only needs to
/// answer "can this Runtime's loader read this archive's layout at all",
/// which is a single yes/no per version, not a compatibility range.
/// Bump it only for breaking layout changes (e.g. renaming `payload/` or
/// changing the compression codec) — adding a new *optional* top-level
/// member is not breaking and does not need a bump.
pub const NYP_FORMAT_VERSION: u32 = 1;

/// A top-level member of a `.nyp` archive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PackageMember {
    /// `manifest.toml` — package metadata (#59). The only required
    /// member: without it there is nothing to identify or load.
    Manifest,
    /// `payload/` — the module's compiled code (wasm or native).
    Payload,
    /// `assets/` — optional bundled non-code resources.
    Assets,
    /// `signatures/` — optional Ed25519 signatures (#61); absent for
    /// unsigned/`unknown`-trust packages (#63).
    Signatures,
}

impl PackageMember {
    /// Every top-level member, in the order they're listed in the format
    /// spec.
    pub const ALL: [Self; 4] = [
        Self::Manifest,
        Self::Payload,
        Self::Assets,
        Self::Signatures,
    ];

    /// This member's path within the archive.
    #[must_use]
    pub const fn path(self) -> &'static str {
        match self {
            Self::Manifest => "manifest.toml",
            Self::Payload => "payload/",
            Self::Assets => "assets/",
            Self::Signatures => "signatures/",
        }
    }

    /// Whether a valid `.nyp` archive must contain this member.
    ///
    /// Only `manifest.toml` is required — `payload/` is normally present
    /// too (a module with no code is unusual, but e.g. a pure-config
    /// "profile" package might legitimately omit it), and `assets/`/
    /// `signatures/` are always optional.
    #[must_use]
    pub const fn required(self) -> bool {
        matches!(self, Self::Manifest)
    }
}

/// Check that `entries` (the top-level paths present in a candidate
/// `.nyp` archive) satisfy the format's structural contract: every
/// [`PackageMember::required`] member is present.
///
/// Takes plain path strings rather than a real archive, so it doesn't
/// need one to be exercised — [`archive::PackageReader::open`] (#60)
/// calls this with the paths it reads out of a real `tar` archive.
///
/// # Errors
/// Returns [`PackageError::MissingMember`] naming the first required
/// member not found in `entries`.
pub fn validate_layout<S: AsRef<str>>(entries: &[S]) -> Result<(), PackageError> {
    for member in PackageMember::ALL {
        if member.required() && !entries.iter().any(|entry| entry.as_ref() == member.path()) {
            return Err(PackageError::MissingMember {
                path: member.path().to_string(),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_is_the_only_required_member() {
        assert!(PackageMember::Manifest.required());
        assert!(!PackageMember::Payload.required());
        assert!(!PackageMember::Assets.required());
        assert!(!PackageMember::Signatures.required());
    }

    #[test]
    fn member_paths_match_the_spec() {
        assert_eq!(PackageMember::Manifest.path(), "manifest.toml");
        assert_eq!(PackageMember::Payload.path(), "payload/");
        assert_eq!(PackageMember::Assets.path(), "assets/");
        assert_eq!(PackageMember::Signatures.path(), "signatures/");
    }

    #[test]
    fn validate_layout_accepts_manifest_only() {
        assert!(validate_layout(&["manifest.toml"]).is_ok());
    }

    #[test]
    fn validate_layout_accepts_a_full_archive() {
        assert!(validate_layout(&["manifest.toml", "payload/", "assets/", "signatures/",]).is_ok());
    }

    #[test]
    fn validate_layout_rejects_a_missing_manifest() {
        let err = validate_layout(&["payload/", "assets/"]).unwrap_err();
        assert!(matches!(
            err,
            PackageError::MissingMember { path } if path == "manifest.toml"
        ));
    }

    #[test]
    fn validate_layout_rejects_an_empty_archive() {
        assert!(validate_layout::<&str>(&[]).is_err());
    }

    #[test]
    fn format_version_is_a_plain_integer() {
        assert_eq!(NYP_FORMAT_VERSION, 1);
    }
}
