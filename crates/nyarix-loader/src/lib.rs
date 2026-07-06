//! Module discovery: finding `.nyp` packages to load (see issue #50), and
//! building a dependency graph over what was found (#53).
//!
//! **Scope note:** this crate finds, indexes, and orders packages by
//! dependency — it does not pick which concrete version satisfies a
//! dependency requirement (#54), detect version/API conflicts (#55),
//! handle optional dependencies (#56), or actually instantiate a module
//! (#57). Those consume [`ScanReport`]/[`ModuleIndex`]/[`DependencyGraph`]
//! once they exist.

pub mod dependency_graph;

pub use dependency_graph::{DependencyCycle, DependencyGraph};

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use nyarix_error::PackageError;
use nyarix_package::PackageReader;
use semver::Version;

/// A `.nyp` package found on disk, with its parsed manifest already
/// available (see [`nyarix_package::PackageReader::open`], which parses
/// `manifest.toml` up front without unpacking the rest of the archive).
#[derive(Debug, Clone)]
pub struct DiscoveredPackage {
    /// Where the `.nyp` file was found.
    pub path: PathBuf,
    /// The package's name, from its manifest.
    pub name: String,
    /// The package's version, from its manifest.
    pub version: Version,
}

/// A directory scan failed to read or parse one candidate `.nyp` file.
///
/// Collected rather than aborting the whole scan — matching #41's "a
/// module that fails to load is logged, not fatal to the Runtime"
/// principle, applied one step earlier, at discovery time.
#[derive(Debug)]
pub struct ScanError {
    /// The file that failed.
    pub path: PathBuf,
    /// Why it failed.
    pub error: PackageError,
}

/// Two `.nyp` files declare the exact same name and version.
#[derive(Debug, Clone)]
pub struct DuplicateModule {
    /// The package name shared by both files.
    pub name: String,
    /// The version shared by both files.
    pub version: Version,
    /// The file discovered first (kept in the index).
    pub kept_path: PathBuf,
    /// The file discovered afterwards (not kept — see
    /// [`ModuleIndex::get`]).
    pub duplicate_path: PathBuf,
}

/// Packages found during a scan, indexed by name and version.
#[derive(Debug, Default)]
pub struct ModuleIndex {
    packages: HashMap<String, Vec<DiscoveredPackage>>,
}

impl ModuleIndex {
    /// All versions of `name` found during the scan, oldest-discovered
    /// first, or an empty slice if none were found.
    #[must_use]
    pub fn get(&self, name: &str) -> &[DiscoveredPackage] {
        self.packages.get(name).map_or(&[], Vec::as_slice)
    }

    /// Every distinct package name found during the scan.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.packages.keys().map(String::as_str)
    }

    /// Total number of indexed packages (summed across all names and
    /// versions, not counting duplicates that were rejected).
    #[must_use]
    pub fn len(&self) -> usize {
        self.packages.values().map(Vec::len).sum()
    }

    /// Whether the index has no packages at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.packages.is_empty()
    }

    /// Insert a discovered package, unless its exact name+version is
    /// already present (a duplicate).
    fn insert(&mut self, package: DiscoveredPackage) -> Option<DuplicateModule> {
        let existing = self.packages.entry(package.name.clone()).or_default();
        if let Some(first) = existing
            .iter()
            .find(|indexed| indexed.version == package.version)
        {
            return Some(DuplicateModule {
                name: package.name,
                version: package.version,
                kept_path: first.path.clone(),
                duplicate_path: package.path,
            });
        }
        existing.push(package);
        None
    }
}

/// The outcome of [`scan_directories`]: the resulting index, plus
/// anything worth reporting that didn't stop the scan.
#[derive(Debug, Default)]
pub struct ScanReport {
    /// Successfully discovered and indexed packages.
    pub index: ModuleIndex,
    /// Files that failed to open or parse.
    pub errors: Vec<ScanError>,
    /// Exact name+version duplicates found (the first occurrence is
    /// kept in `index`; later ones are recorded here instead).
    pub duplicates: Vec<DuplicateModule>,
}

/// Scan `directories` (non-recursively) for `.nyp` files and index them
/// by name and version.
///
/// A directory that doesn't exist is silently skipped — the two default
/// search paths ([`default_search_paths`]) commonly won't both exist,
/// and that's not an error condition. A `.nyp` file that fails to open
/// or parse is recorded in [`ScanReport::errors`] rather than aborting
/// the scan.
#[must_use]
pub fn scan_directories<P: AsRef<Path>>(directories: &[P]) -> ScanReport {
    let mut report = ScanReport::default();

    for directory in directories {
        let directory = directory.as_ref();
        let Ok(entries) = fs::read_dir(directory) else {
            continue;
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map_or(true, |ext| ext != "nyp") {
                continue;
            }

            match discover_one(&path) {
                Ok(package) => {
                    if let Some(duplicate) = report.index.insert(package) {
                        report.duplicates.push(duplicate);
                    }
                }
                Err(error) => report.errors.push(ScanError { path, error }),
            }
        }
    }

    report
}

fn discover_one(path: &Path) -> Result<DiscoveredPackage, PackageError> {
    let data = fs::read(path).map_err(|source| PackageError::Io {
        path: path.display().to_string(),
        source,
    })?;
    let reader = PackageReader::open(&data)?;
    let info = &reader.manifest().package;
    Ok(DiscoveredPackage {
        path: path.to_path_buf(),
        name: info.name.clone(),
        version: info.version.clone(),
    })
}

/// The two default module search paths: `~/.nyarix/modules/` and
/// `./modules/` (see #50).
///
/// The home directory is resolved from `HOME` (Unix) or `USERPROFILE`
/// (Windows); if neither is set, only `./modules/` is returned. Neither
/// path is checked for existence here — [`scan_directories`] silently
/// skips ones that don't exist.
#[must_use]
pub fn default_search_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        paths.push(PathBuf::from(home).join(".nyarix").join("modules"));
    }
    paths.push(PathBuf::from("./modules"));
    paths
}

#[cfg(test)]
mod tests {
    use super::*;
    use nyarix_package::PackageBuilder;

    fn manifest_toml(name: &str, version: &str) -> String {
        format!(
            r#"
[package]
name = "{name}"
version = "{version}"
module_type = "transport"
api_version = "1.0"
author = "Nyarix"
description = "test package"
"#
        )
    }

    fn write_package(dir: &Path, filename: &str, name: &str, version: &str) {
        let data = PackageBuilder::new()
            .add_file("manifest.toml", manifest_toml(name, version).into_bytes())
            .build()
            .unwrap();
        fs::write(dir.join(filename), data).unwrap();
    }

    #[test]
    fn finds_and_indexes_a_package() {
        let dir = tempdir();
        write_package(dir.path(), "udp.nyp", "nyarix-transport-udp", "0.1.0");

        let report = scan_directories(&[dir.path()]);

        assert!(report.errors.is_empty());
        assert!(report.duplicates.is_empty());
        assert_eq!(report.index.len(), 1);
        let versions = report.index.get("nyarix-transport-udp");
        assert_eq!(versions.len(), 1);
        assert_eq!(versions[0].version, Version::new(0, 1, 0));
    }

    #[test]
    fn indexes_multiple_versions_of_the_same_name_without_flagging_them_as_duplicates() {
        let dir = tempdir();
        write_package(dir.path(), "udp-1.nyp", "nyarix-transport-udp", "0.1.0");
        write_package(dir.path(), "udp-2.nyp", "nyarix-transport-udp", "0.2.0");

        let report = scan_directories(&[dir.path()]);

        assert!(report.duplicates.is_empty());
        assert_eq!(report.index.get("nyarix-transport-udp").len(), 2);
    }

    #[test]
    fn flags_an_exact_name_and_version_duplicate() {
        let dir = tempdir();
        write_package(dir.path(), "udp-a.nyp", "nyarix-transport-udp", "0.1.0");
        write_package(dir.path(), "udp-b.nyp", "nyarix-transport-udp", "0.1.0");

        let report = scan_directories(&[dir.path()]);

        assert_eq!(report.index.get("nyarix-transport-udp").len(), 1);
        assert_eq!(report.duplicates.len(), 1);
        assert_eq!(report.duplicates[0].name, "nyarix-transport-udp");
    }

    #[test]
    fn ignores_files_that_are_not_dot_nyp() {
        let dir = tempdir();
        fs::write(dir.path().join("readme.txt"), b"not a package").unwrap();

        let report = scan_directories(&[dir.path()]);

        assert!(report.index.is_empty());
        assert!(report.errors.is_empty());
    }

    #[test]
    fn records_a_scan_error_for_a_corrupt_nyp_file() {
        let dir = tempdir();
        fs::write(dir.path().join("broken.nyp"), b"not a real archive").unwrap();

        let report = scan_directories(&[dir.path()]);

        assert!(report.index.is_empty());
        assert_eq!(report.errors.len(), 1);
    }

    #[test]
    fn silently_skips_a_nonexistent_directory() {
        let report = scan_directories(&["/nonexistent/path/for/nyarix/tests"]);
        assert!(report.index.is_empty());
        assert!(report.errors.is_empty());
    }

    #[test]
    fn default_search_paths_always_includes_the_local_modules_dir() {
        let paths = default_search_paths();
        assert!(paths.contains(&PathBuf::from("./modules")));
    }

    // A minimal temp-directory helper, to avoid pulling in a `tempfile`
    // dependency for one test module.
    struct TempDir(PathBuf);

    impl TempDir {
        fn path(&self) -> &Path {
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
            "nyarix-loader-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }
}
