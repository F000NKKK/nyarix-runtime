//! Module Loader integration (see issue #41).
//!
//! Ties the Runtime to `nyarix-loader`/`nyarix-package` (M5/M6, #50-#66):
//! scans configured module directories, reads every candidate `.nyp`
//! found, and runs each one through verify+resolve
//! ([`nyarix_loader::validate_package`], which already covers unpacking,
//! manifest schema validation, signature checking, dependency/API
//! version conflicts, platform compatibility, and trust level
//! assignment). "Неудачные модули логируются, но не останавливают
//! Runtime": a package that fails to even open, or fails validation, is
//! recorded in the returned [`ModuleLoadReport`] and skipped — the scan
//! continues with the rest.
//!
//! **Scope note:** the lifecycle's last step — actually instantiating
//! each valid package's module ([`nyarix_loader::instantiate`], #57) and
//! registering it — isn't run here. That needs a real `Box<dyn Module>`,
//! which needs loading the package's compiled code (WASM or native),
//! which #107 documents doesn't exist anywhere in this workspace yet.
//! [`ModuleLoadReport::valid`] is exactly the set of packages that would
//! feed into that step once it exists — each is a validated,
//! `is_valid()` package with its manifest and archive bytes still at
//! hand.

use std::fs;
use std::path::PathBuf;

use nyarix_error::PackageError;
use nyarix_loader::{ScanError, ValidationReport, scan_directories, validate_package};
use nyarix_module_api::ApiVersion;
use nyarix_package::{PackageReader, TrustStore};

/// One package that passed validation ([`ValidationReport::is_valid`]).
#[derive(Debug)]
pub struct LoadedPackage {
    /// Where the `.nyp` file is on disk.
    pub path: PathBuf,
    /// Its raw archive bytes, kept around for the eventual Module
    /// instantiation step (#57/#107) so it doesn't need to be re-read.
    pub data: Vec<u8>,
    /// The full validation report (dependencies, trust level, signature
    /// status, ...).
    pub validation: ValidationReport,
}

/// One package that was found but failed validation.
#[derive(Debug)]
pub struct RejectedPackage {
    /// Where the `.nyp` file is on disk.
    pub path: PathBuf,
    /// Why it was rejected.
    pub validation: ValidationReport,
}

/// The outcome of scanning and validating every module in a set of
/// directories.
#[derive(Debug, Default)]
pub struct ModuleLoadReport {
    /// Packages that passed validation.
    pub valid: Vec<LoadedPackage>,
    /// Packages found but that failed validation (see
    /// [`ValidationReport::is_valid`]) — still logged, not fatal.
    pub invalid: Vec<RejectedPackage>,
    /// Files that failed to even be read or opened as a `.nyp` archive.
    pub errors: Vec<ScanError>,
}

/// Scan `directories` for `.nyp` packages and validate every one found.
///
/// Every package is cross-checked against every other package found in
/// the same scan for dependency/API-version conflicts (see
/// [`nyarix_loader::validate_package`]'s docs on what `conflicts` means
/// when validating a whole set at once).
#[must_use]
pub fn load_modules(
    directories: &[PathBuf],
    trust_store: &TrustStore,
    required_api_version: ApiVersion,
) -> ModuleLoadReport {
    let scan = scan_directories(directories);
    let mut report = ModuleLoadReport {
        errors: scan.errors,
        ..ModuleLoadReport::default()
    };

    let mut candidates: Vec<(PathBuf, Vec<u8>)> = Vec::new();
    for name in scan.index.names() {
        for discovered in scan.index.get(name) {
            match fs::read(&discovered.path) {
                Ok(data) => candidates.push((discovered.path.clone(), data)),
                Err(source) => {
                    tracing::warn!(
                        path = %discovered.path.display(),
                        error = %source,
                        "skipping module: failed to read package file"
                    );
                    report.errors.push(ScanError {
                        path: discovered.path.clone(),
                        error: PackageError::Io {
                            path: discovered.path.display().to_string(),
                            source,
                        },
                    });
                }
            }
        }
    }

    // Parse every candidate's manifest up front so each package can be
    // cross-checked against the others' dependencies/API versions. A
    // candidate that fails to parse here fails again — and gets
    // recorded — inside `validate_package` for its own turn below.
    let manifests: Vec<Option<nyarix_package::PackageManifest>> = candidates
        .iter()
        .map(|(_, data)| {
            PackageReader::open(data)
                .ok()
                .map(|reader| reader.manifest().clone())
        })
        .collect();

    for (index, (path, data)) in candidates.into_iter().enumerate() {
        let own_name = manifests[index].as_ref().map(|m| m.package.name.as_str());
        let other_manifests = manifests
            .iter()
            .enumerate()
            .filter(|(other_index, _)| *other_index != index)
            .filter_map(|(_, manifest)| manifest.as_ref());

        match validate_package(
            &data,
            other_manifests,
            &scan.index,
            trust_store,
            required_api_version,
        ) {
            Ok(mut validation) => {
                // `validate_package`'s `conflicts` reflects the whole
                // combined set it was run against (see its own docs) —
                // narrow it down to conflicts this specific package is
                // actually party to, so one unrelated package's broken
                // dependency doesn't mark every other package in the
                // directory invalid too.
                if let Some(name) = own_name {
                    validation
                        .conflicts
                        .retain(|conflict| conflict_involves(conflict, name));
                }

                if validation.is_valid() {
                    report.valid.push(LoadedPackage {
                        path,
                        data,
                        validation,
                    });
                } else {
                    tracing::warn!(
                        path = %path.display(),
                        conflicts = validation.conflicts.len(),
                        platform_supported = validation.platform_supported,
                        "skipping module: failed validation"
                    );
                    report.invalid.push(RejectedPackage { path, validation });
                }
            }
            Err(error) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %error,
                    "skipping module: failed to open package"
                );
                report.errors.push(ScanError { path, error });
            }
        }
    }

    report
}

/// Whether `conflict` involves the package named `name` (as a requirer,
/// or as the package itself for an API version mismatch).
fn conflict_involves(conflict: &nyarix_loader::Conflict, name: &str) -> bool {
    use nyarix_loader::Conflict;
    match conflict {
        Conflict::IncompatibleVersions { requirements, .. } => requirements
            .iter()
            .any(|requirement| requirement.requirer == name),
        Conflict::MissingDependency { requirers, .. } => {
            requirers.iter().any(|requirer| requirer == name)
        }
        Conflict::IncompatibleApiVersion { module, .. } => module == name,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nyarix_package::PackageBuilder;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TempDir(PathBuf);

    impl TempDir {
        fn path(&self) -> &std::path::Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn tempdir() -> TempDir {
        let dir = std::env::temp_dir().join(format!(
            "nyarix-runtime-module-loader-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }

    fn manifest_toml(name: &str) -> String {
        format!(
            r#"
[package]
name = "{name}"
version = "0.1.0"
module_type = "flow"
api_version = "1.0"
author = "Nyarix"
description = "test"
"#
        )
    }

    fn write_package(dir: &std::path::Path, filename: &str, name: &str) {
        let data = PackageBuilder::new()
            .add_file("manifest.toml", manifest_toml(name).into_bytes())
            .build()
            .unwrap();
        fs::write(dir.join(filename), data).unwrap();
    }

    #[test]
    fn valid_packages_are_loaded() {
        let dir = tempdir();
        write_package(dir.path(), "a.nyp", "module-a");

        let report = load_modules(
            &[dir.path().to_path_buf()],
            &TrustStore::new(),
            ApiVersion::new(1, 0),
        );

        assert_eq!(report.valid.len(), 1);
        assert!(report.invalid.is_empty());
        assert!(report.errors.is_empty());
    }

    #[test]
    fn a_corrupt_file_is_logged_not_fatal() {
        let dir = tempdir();
        write_package(dir.path(), "good.nyp", "module-a");
        fs::write(dir.path().join("broken.nyp"), b"not a real archive").unwrap();

        let report = load_modules(
            &[dir.path().to_path_buf()],
            &TrustStore::new(),
            ApiVersion::new(1, 0),
        );

        assert_eq!(report.valid.len(), 1);
        assert_eq!(report.errors.len(), 1);
    }

    #[test]
    fn an_unsupported_platform_is_invalid_not_an_error() {
        let dir = tempdir();
        let toml = r#"
[package]
name = "module-a"
version = "0.1.0"
module_type = "flow"
api_version = "1.0"
author = "Nyarix"
description = "test"

[platforms]
supported = ["ios"]
"#;
        let data = PackageBuilder::new()
            .add_file("manifest.toml", toml.as_bytes())
            .build()
            .unwrap();
        fs::write(dir.path().join("a.nyp"), data).unwrap();

        let report = load_modules(
            &[dir.path().to_path_buf()],
            &TrustStore::new(),
            ApiVersion::new(1, 0),
        );

        if !matches!(
            nyarix_module_api::Platform::current(),
            nyarix_module_api::Platform::Ios
        ) {
            assert!(report.valid.is_empty());
            assert_eq!(report.invalid.len(), 1);
        }
    }

    #[test]
    fn cross_package_dependency_conflicts_are_detected() {
        let dir = tempdir();
        let a = r#"
[package]
name = "a"
version = "0.1.0"
module_type = "flow"
api_version = "1.0"
author = "Nyarix"
description = "test"

[dependencies]
crypto-core = "^1.0"
"#;
        let data_a = PackageBuilder::new()
            .add_file("manifest.toml", a.as_bytes())
            .build()
            .unwrap();
        fs::write(dir.path().join("a.nyp"), data_a).unwrap();
        write_package(dir.path(), "b.nyp", "b");

        let report = load_modules(
            &[dir.path().to_path_buf()],
            &TrustStore::new(),
            ApiVersion::new(1, 0),
        );

        // "a" needs crypto-core, which nothing here provides — "a" is
        // invalid, but "b" (no dependencies) still loads fine.
        assert_eq!(report.valid.len(), 1);
        assert_eq!(report.invalid.len(), 1);
    }

    #[test]
    fn scanning_no_directories_yields_an_empty_report() {
        let report = load_modules(&[], &TrustStore::new(), ApiVersion::new(1, 0));
        assert!(report.valid.is_empty());
        assert!(report.invalid.is_empty());
        assert!(report.errors.is_empty());
    }
}
