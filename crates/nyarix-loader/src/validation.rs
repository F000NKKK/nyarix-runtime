//! Package validation pipeline (see issue #64).
//!
//! Ties together everything already built for a candidate `.nyp`
//! archive: unpacking and manifest schema validation (#58/#59/#60),
//! signature checking (#61/#62), dependency/API-version conflict
//! detection (#53/#54/#55), platform compatibility, and trust level
//! assignment (#63).
//!
//! **Scope note:** the issue's "5. Проверка capabilities" step isn't
//! implemented. `Capability` (#21) is a closed enum, so a manifest's
//! declared capabilities are already guaranteed well-formed the moment
//! it parses — there's nothing left to check *syntactically*. The
//! meaningful version of this check — "is this capability actually
//! grantable, given the package's trust level and the Sandbox's
//! policy" — needs the Sandbox (M7, #69 Capability model design through
//! #78), which doesn't exist yet. Adding a step here that always
//! trivially passes would be dead weight, not validation.

use nyarix_error::PackageError;
use nyarix_module_api::{ApiVersion, Platform};
use nyarix_package::{
    PackageManifest, PackageReader, SignatureStatus, TrustLevel, TrustStore, classify,
};

use crate::ModuleIndex;
use crate::conflict::{Conflict, detect_conflicts};

/// The outcome of running the validation pipeline on one candidate
/// package.
#[derive(Debug)]
pub struct ValidationReport {
    /// Step 3: whether the package's embedded signature (if any)
    /// verifies.
    pub signature_status: SignatureStatus,
    /// Step 7: the trust level its signature (or lack of one) implies.
    pub trust_level: TrustLevel,
    /// Step 4: dependency and Module API version conflicts across the
    /// candidate package plus every other manifest it was validated
    /// against — not exclusively problems the candidate introduced, see
    /// [`validate_package`]'s docs.
    pub conflicts: Vec<Conflict>,
    /// Step 6: whether the candidate declares support for the platform
    /// this Runtime is currently running on (or declares no supported
    /// platforms at all, meaning "unspecified, assume all").
    pub platform_supported: bool,
}

impl ValidationReport {
    /// Whether the package passed every check this pipeline actually
    /// performs.
    ///
    /// Deliberately doesn't factor in [`Self::trust_level`] or
    /// [`Self::signature_status`] — an unsigned/`Unknown`-trust package
    /// can still be structurally, dependency-wise, and
    /// platform-wise valid; whether `Unknown` trust is *acceptable* is
    /// the caller's policy decision (dev vs. production strict mode,
    /// #62), not this pipeline's.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.conflicts.is_empty() && self.platform_supported
    }
}

/// Run the validation pipeline (#64) on `data`, a candidate `.nyp`
/// archive.
///
/// `other_manifests` and `index` provide the context to check
/// dependencies/conflicts against (typically every other package
/// already discovered, #50) — `conflicts` in the returned report
/// reflects the state of that whole combined set, not exclusively
/// problems `data` introduces on its own (distinguishing the two would
/// need re-running conflict detection with and without the candidate,
/// which callers can already do themselves with [`detect_conflicts`] if
/// they need that distinction).
///
/// # Errors
/// Returns [`PackageError`] if `data` can't even be unpacked, or its
/// manifest doesn't parse (steps 1-2) — everything else is folded into
/// the returned [`ValidationReport`] instead of an error, since a
/// package with e.g. unresolved dependencies is still meaningfully "the
/// package", just not one that should be loaded.
pub fn validate_package<'a>(
    data: &[u8],
    other_manifests: impl IntoIterator<Item = &'a PackageManifest>,
    index: &ModuleIndex,
    trust_store: &TrustStore,
    required_api_version: ApiVersion,
) -> Result<ValidationReport, PackageError> {
    let reader = PackageReader::open(data)?;

    let signature_status = reader.signature_status()?;
    let trust_level = classify(&reader, trust_store)?;

    let mut manifests: Vec<&PackageManifest> = other_manifests.into_iter().collect();
    manifests.push(reader.manifest());
    let conflicts = detect_conflicts(manifests, index, required_api_version);

    let supported = &reader.manifest().platforms.supported;
    let platform_supported = supported.is_empty() || supported.contains(&Platform::current());

    Ok(ValidationReport {
        signature_status,
        trust_level,
        conflicts,
        platform_supported,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use nyarix_package::PackageBuilder;

    fn manifest_toml(name: &str, platforms: &str) -> String {
        format!(
            r#"
[package]
name = "{name}"
version = "0.1.0"
module_type = "flow"
api_version = "1.0"
author = "Nyarix"
description = "test"

[platforms]
supported = [{platforms}]
"#
        )
    }

    #[test]
    fn a_minimal_valid_package_passes() {
        let data = PackageBuilder::new()
            .add_file("manifest.toml", manifest_toml("a", "").into_bytes())
            .build()
            .unwrap();

        let report = validate_package(
            &data,
            std::iter::empty(),
            &ModuleIndex::default(),
            &TrustStore::new(),
            ApiVersion::new(1, 0),
        )
        .unwrap();

        assert!(report.is_valid());
        assert_eq!(report.signature_status, SignatureStatus::Unsigned);
        assert_eq!(report.trust_level, TrustLevel::Unknown);
        assert!(report.conflicts.is_empty());
        assert!(report.platform_supported);
    }

    #[test]
    fn a_signed_package_from_a_trusted_key_gets_its_trust_level() {
        let key = nyarix_package::signing::generate_keypair();
        let data = PackageBuilder::new()
            .add_file("manifest.toml", manifest_toml("a", "").into_bytes())
            .sign(&key)
            .build()
            .unwrap();

        let mut store = TrustStore::new();
        store.trust(key.verifying_key(), TrustLevel::Official);

        let report = validate_package(
            &data,
            std::iter::empty(),
            &ModuleIndex::default(),
            &store,
            ApiVersion::new(1, 0),
        )
        .unwrap();

        assert_eq!(report.signature_status, SignatureStatus::Verified);
        assert_eq!(report.trust_level, TrustLevel::Official);
        assert!(report.is_valid());
    }

    #[test]
    fn an_unsupported_platform_fails_validation() {
        // Neither "linux" nor whatever this test happens to run on is
        // in this list, so it can never claim support.
        let data = PackageBuilder::new()
            .add_file(
                "manifest.toml",
                manifest_toml("a", "\"ios\", \"android\"").into_bytes(),
            )
            .build()
            .unwrap();

        let report = validate_package(
            &data,
            std::iter::empty(),
            &ModuleIndex::default(),
            &TrustStore::new(),
            ApiVersion::new(1, 0),
        )
        .unwrap();

        // Only assert the platform check specifically failed if this
        // test binary genuinely isn't running as iOS/Android — true for
        // every CI/dev target this workspace builds on.
        if !matches!(Platform::current(), Platform::Ios | Platform::Android) {
            assert!(!report.platform_supported);
            assert!(!report.is_valid());
        }
    }

    #[test]
    fn an_empty_platform_list_means_all_platforms_supported() {
        let data = PackageBuilder::new()
            .add_file("manifest.toml", manifest_toml("a", "").into_bytes())
            .build()
            .unwrap();

        let report = validate_package(
            &data,
            std::iter::empty(),
            &ModuleIndex::default(),
            &TrustStore::new(),
            ApiVersion::new(1, 0),
        )
        .unwrap();

        assert!(report.platform_supported);
    }

    #[test]
    fn a_dependency_conflict_against_other_manifests_is_reported() {
        let a = {
            let toml = r#"
[package]
name = "a"
version = "0.1.0"
module_type = "flow"
api_version = "1.0"
author = "Nyarix"
description = "test"

[dependencies]
crypto-core = "^2.0"
"#;
            PackageManifest::from_toml(toml).unwrap()
        };
        let data = PackageBuilder::new()
            .add_file("manifest.toml", manifest_toml("b", "").into_bytes())
            .build()
            .unwrap();

        // "b" itself has no dependencies, but validating it alongside
        // "a" (which needs crypto-core ^2.0, unavailable) still surfaces
        // that conflict in the combined report.
        let report = validate_package(
            &data,
            [&a],
            &ModuleIndex::default(),
            &TrustStore::new(),
            ApiVersion::new(1, 0),
        )
        .unwrap();

        assert!(!report.conflicts.is_empty());
        assert!(!report.is_valid());
    }

    #[test]
    fn open_failure_is_a_real_error_not_a_report() {
        let err = validate_package(
            b"not a real archive",
            std::iter::empty(),
            &ModuleIndex::default(),
            &TrustStore::new(),
            ApiVersion::new(1, 0),
        )
        .unwrap_err();

        assert!(matches!(err, PackageError::Io { .. }));
    }
}
