//! Version resolution: picking one concrete version per dependency name
//! that satisfies every requirer's semver requirement (see issue #54).
//!
//! **Scope note:** this only picks versions — it doesn't decide whether
//! an unresolved dependency is fatal (that's severity/categorization
//! Conflict detection, #55, adds — e.g. distinguishing "not found at
//! all" from "found, but every candidate is incompatible", and API
//! version mismatches) and it doesn't know about `optional = true`
//! dependencies (#56), which shouldn't produce a conflict at all when
//! unresolved. Both build on [`ResolvedVersions`].

use std::collections::HashMap;

use nyarix_package::PackageManifest;
use semver::{Version, VersionReq};

use crate::ModuleIndex;

/// One package's declared requirement on a dependency, kept around so a
/// [`VersionConflict`] can explain who wanted what.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Requirement {
    /// The package that declared this requirement.
    pub requirer: String,
    /// The semver range it requires.
    pub version_req: VersionReq,
}

/// No available version of `name` satisfies every requirer's
/// requirement simultaneously (including the degenerate case where no
/// version of `name` is available at all).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionConflict {
    /// The dependency name in conflict.
    pub name: String,
    /// Every requirement declared on it, across all requirers.
    pub requirements: Vec<Requirement>,
}

impl std::fmt::Display for VersionConflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "no available version of \"{}\" satisfies every requirement: ",
            self.name
        )?;
        let parts: Vec<String> = self
            .requirements
            .iter()
            .map(|r| format!("{} requires {} {}", r.requirer, self.name, r.version_req))
            .collect();
        write!(f, "{}", parts.join("; "))
    }
}

impl std::error::Error for VersionConflict {}

/// The outcome of [`resolve_versions`].
#[derive(Debug, Default)]
pub struct ResolvedVersions {
    /// The chosen version for each dependency name that could be
    /// resolved.
    pub resolved: HashMap<String, Version>,
    /// Dependency names for which no available version satisfied every
    /// requirer.
    pub conflicts: Vec<VersionConflict>,
}

/// For every dependency named across `manifests`, pick the highest
/// version available in `index` that satisfies every manifest's
/// requirement on that name.
///
/// "Highest compatible version" (per the issue's stated priority): among
/// the versions in `index` that satisfy *all* collected requirements for
/// a name, the maximum by semver ordering is chosen — never an arbitrary
/// or first-seen one.
#[must_use]
pub fn resolve_versions<'a>(
    manifests: impl IntoIterator<Item = &'a PackageManifest>,
    index: &ModuleIndex,
) -> ResolvedVersions {
    let mut requirements: HashMap<String, Vec<Requirement>> = HashMap::new();
    for manifest in manifests {
        for (name, version_req) in &manifest.dependencies {
            requirements
                .entry(name.clone())
                .or_default()
                .push(Requirement {
                    requirer: manifest.package.name.clone(),
                    version_req: version_req.clone(),
                });
        }
    }

    let mut resolved = HashMap::new();
    let mut conflicts = Vec::new();

    for (name, reqs) in requirements {
        let best = index
            .get(&name)
            .iter()
            .map(|package| &package.version)
            .filter(|version| reqs.iter().all(|req| req.version_req.matches(version)))
            .max()
            .cloned();

        match best {
            Some(version) => {
                resolved.insert(name, version);
            }
            None => conflicts.push(VersionConflict {
                name,
                requirements: reqs,
            }),
        }
    }

    ResolvedVersions {
        resolved,
        conflicts,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan_directories;
    use nyarix_package::PackageBuilder;
    use std::fs;
    use std::path::{Path, PathBuf};

    fn manifest_with_deps(name: &str, deps: &[(&str, &str)]) -> PackageManifest {
        let deps_toml: String = deps
            .iter()
            .map(|(dep_name, req)| format!("{dep_name} = \"{req}\"\n"))
            .collect();
        let toml = format!(
            r#"
[package]
name = "{name}"
version = "0.1.0"
module_type = "flow"
api_version = "1.0"
author = "Nyarix"
description = "test"

[dependencies]
{deps_toml}
"#
        );
        PackageManifest::from_toml(&toml).unwrap()
    }

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
            "nyarix-loader-version-resolver-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }

    fn index_with_versions(name: &str, versions: &[&str]) -> ModuleIndex {
        let dir = tempdir();
        for (i, version) in versions.iter().enumerate() {
            let manifest_toml = format!(
                r#"
[package]
name = "{name}"
version = "{version}"
module_type = "flow"
api_version = "1.0"
author = "Nyarix"
description = "test"
"#
            );
            let data = PackageBuilder::new()
                .add_file("manifest.toml", manifest_toml.into_bytes())
                .build()
                .unwrap();
            fs::write(dir.path().join(format!("pkg-{i}.nyp")), data).unwrap();
        }
        // `scan_directories` reads every file's contents into the index
        // immediately, so the temp directory can be cleaned up right
        // after — nothing later needs the files to still exist on disk.
        let report = scan_directories(&[dir.path()]);
        drop(dir);
        report.index
    }

    #[test]
    fn resolves_a_single_matching_version() {
        let index = index_with_versions("crypto-core", &["0.1.0"]);
        let requirer = manifest_with_deps("udp-transport", &[("crypto-core", "^0.1")]);

        let result = resolve_versions([&requirer], &index);

        assert!(result.conflicts.is_empty());
        assert_eq!(
            result.resolved.get("crypto-core"),
            Some(&Version::new(0, 1, 0))
        );
    }

    #[test]
    fn picks_the_highest_compatible_version_not_the_first() {
        let index = index_with_versions("crypto-core", &["0.1.0", "0.1.5", "0.1.2"]);
        let requirer = manifest_with_deps("udp-transport", &[("crypto-core", "^0.1")]);

        let result = resolve_versions([&requirer], &index);

        assert_eq!(
            result.resolved.get("crypto-core"),
            Some(&Version::new(0, 1, 5))
        );
    }

    #[test]
    fn satisfies_two_requirers_with_overlapping_ranges() {
        let index = index_with_versions("crypto-core", &["1.0.0", "1.5.0", "1.9.0"]);
        let a = manifest_with_deps("a", &[("crypto-core", ">=1.0.0, <2.0.0")]);
        let b = manifest_with_deps("b", &[("crypto-core", "^1.5")]);

        let result = resolve_versions([&a, &b], &index);

        assert!(result.conflicts.is_empty());
        // 1.9.0 is the highest version satisfying both >=1.0,<2.0 and ^1.5.
        assert_eq!(
            result.resolved.get("crypto-core"),
            Some(&Version::new(1, 9, 0))
        );
    }

    #[test]
    fn reports_a_conflict_when_no_version_satisfies_every_requirer() {
        let index = index_with_versions("crypto-core", &["1.0.0", "2.0.0"]);
        let a = manifest_with_deps("a", &[("crypto-core", "^1.0")]);
        let b = manifest_with_deps("b", &[("crypto-core", "^2.0")]);

        let result = resolve_versions([&a, &b], &index);

        assert!(result.resolved.get("crypto-core").is_none());
        assert_eq!(result.conflicts.len(), 1);
        assert_eq!(result.conflicts[0].name, "crypto-core");
        assert_eq!(result.conflicts[0].requirements.len(), 2);
    }

    #[test]
    fn reports_a_conflict_when_the_dependency_is_entirely_missing() {
        let index = ModuleIndex::default();
        let a = manifest_with_deps("a", &[("crypto-core", "^1.0")]);

        let result = resolve_versions([&a], &index);

        assert!(result.resolved.is_empty());
        assert_eq!(result.conflicts.len(), 1);
        assert_eq!(result.conflicts[0].name, "crypto-core");
    }

    #[test]
    fn conflict_message_names_every_requirer_and_its_requirement() {
        let index = index_with_versions("crypto-core", &["1.0.0", "2.0.0"]);
        let a = manifest_with_deps("a", &[("crypto-core", "^1.0")]);
        let b = manifest_with_deps("b", &[("crypto-core", "^2.0")]);

        let result = resolve_versions([&a, &b], &index);
        let message = result.conflicts[0].to_string();

        assert!(message.contains('a'));
        assert!(message.contains('b'));
        assert!(message.contains("crypto-core"));
    }

    #[test]
    fn a_package_with_no_dependencies_resolves_nothing_and_conflicts_nothing() {
        let index = ModuleIndex::default();
        let a = manifest_with_deps("a", &[]);

        let result = resolve_versions([&a], &index);

        assert!(result.resolved.is_empty());
        assert!(result.conflicts.is_empty());
    }
}
