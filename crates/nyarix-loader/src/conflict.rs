//! Conflict detection: categorizing #54's raw version conflicts and
//! checking Module API compatibility, with human-readable messages (see
//! issue #55).

use nyarix_module_api::{ApiVersion, is_compatible};
use nyarix_package::PackageManifest;

use crate::ModuleIndex;
use crate::version_resolver::{Requirement, resolve_versions};

/// A single detected conflict, in a form meant to be shown to a human
/// (a log line, a CLI error, ...) — see each variant's [`Display`]
/// impl.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Conflict {
    /// Two or more requirers need versions of the same dependency that
    /// no single available version satisfies (see #54's
    /// `VersionConflict`) — typically because they need different major
    /// versions.
    IncompatibleVersions {
        /// The dependency name in conflict.
        name: String,
        /// Every requirement declared on it.
        requirements: Vec<Requirement>,
    },
    /// No known package provides this dependency at all.
    MissingDependency {
        /// The dependency name that couldn't be found.
        name: String,
        /// The packages that require it.
        requirers: Vec<String>,
    },
    /// A package's own declared Module API version (#25) isn't
    /// compatible with what the Runtime requires.
    IncompatibleApiVersion {
        /// The package with the incompatible declaration.
        module: String,
        /// The Module API version the Runtime requires.
        required: ApiVersion,
        /// The Module API version the package declares.
        declared: ApiVersion,
    },
}

impl std::fmt::Display for Conflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IncompatibleVersions { name, requirements } => {
                for (i, req) in requirements.iter().enumerate() {
                    if i == 0 {
                        write!(
                            f,
                            "Module {} requires {name} {}",
                            req.requirer, req.version_req
                        )?;
                    } else {
                        write!(
                            f,
                            ", but module {} requires {name} {}",
                            req.requirer, req.version_req
                        )?;
                    }
                }
                Ok(())
            }
            Self::MissingDependency { name, requirers } => {
                write!(
                    f,
                    "{} require{} \"{name}\", but no installed package provides it",
                    requirers.join(", "),
                    if requirers.len() == 1 { "s" } else { "" }
                )
            }
            Self::IncompatibleApiVersion {
                module,
                required,
                declared,
            } => write!(
                f,
                "module \"{module}\" was built against Module API {declared}, \
                 which is incompatible with the Runtime's required {required}"
            ),
        }
    }
}

/// Detect every conflict among `manifests`: incompatible/missing
/// dependencies (built on #54's [`resolve_versions`]) and Module API
/// version mismatches against `required_api_version`.
#[must_use]
pub fn detect_conflicts<'a>(
    manifests: impl IntoIterator<Item = &'a PackageManifest>,
    index: &ModuleIndex,
    required_api_version: ApiVersion,
) -> Vec<Conflict> {
    let manifests: Vec<&PackageManifest> = manifests.into_iter().collect();
    let mut conflicts = Vec::new();

    let resolved = resolve_versions(manifests.iter().copied(), index);
    for version_conflict in resolved.conflicts {
        if index.get(&version_conflict.name).is_empty() {
            conflicts.push(Conflict::MissingDependency {
                requirers: version_conflict
                    .requirements
                    .iter()
                    .map(|r| r.requirer.clone())
                    .collect(),
                name: version_conflict.name,
            });
        } else {
            conflicts.push(Conflict::IncompatibleVersions {
                name: version_conflict.name,
                requirements: version_conflict.requirements,
            });
        }
    }

    for manifest in &manifests {
        if !is_compatible(required_api_version, manifest.package.api_version) {
            conflicts.push(Conflict::IncompatibleApiVersion {
                module: manifest.package.name.clone(),
                required: required_api_version,
                declared: manifest.package.api_version,
            });
        }
    }

    conflicts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan_directories;
    use nyarix_package::PackageBuilder;
    use std::fs;
    use std::path::{Path, PathBuf};

    fn manifest_with(name: &str, api_version: &str, deps: &[(&str, &str)]) -> PackageManifest {
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
api_version = "{api_version}"
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
            "nyarix-loader-conflict-test-{}-{}",
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
        let report = scan_directories(&[dir.path()]);
        drop(dir);
        report.index
    }

    #[test]
    fn reports_incompatible_major_versions_with_a_readable_message() {
        let index = index_with_versions("crypto-core", &["1.0.0", "2.0.0"]);
        let a = manifest_with("a", "1.0", &[("crypto-core", "^1.0")]);
        let b = manifest_with("b", "1.0", &[("crypto-core", "^2.0")]);

        let conflicts = detect_conflicts([&a, &b], &index, ApiVersion::new(1, 0));

        let version_conflict = conflicts
            .iter()
            .find(|c| matches!(c, Conflict::IncompatibleVersions { .. }))
            .expect("expected an IncompatibleVersions conflict");
        let message = version_conflict.to_string();
        assert!(message.contains("Module a requires crypto-core"));
        assert!(message.contains("but module b requires crypto-core"));
    }

    #[test]
    fn reports_a_missing_dependency_distinctly_from_an_incompatible_one() {
        let index = ModuleIndex::default();
        let a = manifest_with("a", "1.0", &[("crypto-core", "^1.0")]);

        let conflicts = detect_conflicts([&a], &index, ApiVersion::new(1, 0));

        assert_eq!(conflicts.len(), 1);
        match &conflicts[0] {
            Conflict::MissingDependency { name, requirers } => {
                assert_eq!(name, "crypto-core");
                assert_eq!(requirers, &vec!["a".to_string()]);
            }
            other => panic!("expected MissingDependency, got {other:?}"),
        }
        assert!(
            conflicts[0]
                .to_string()
                .contains("no installed package provides it")
        );
    }

    #[test]
    fn reports_an_incompatible_api_version() {
        let index = ModuleIndex::default();
        let old_module = manifest_with("legacy", "1.0", &[]);

        let conflicts = detect_conflicts([&old_module], &index, ApiVersion::new(2, 0));

        assert_eq!(conflicts.len(), 1);
        match &conflicts[0] {
            Conflict::IncompatibleApiVersion {
                module,
                required,
                declared,
            } => {
                assert_eq!(module, "legacy");
                assert_eq!(*required, ApiVersion::new(2, 0));
                assert_eq!(*declared, ApiVersion::new(1, 0));
            }
            other => panic!("expected IncompatibleApiVersion, got {other:?}"),
        }
    }

    #[test]
    fn a_compatible_api_version_is_not_a_conflict() {
        let index = ModuleIndex::default();
        let module = manifest_with("modern", "1.2", &[]);

        let conflicts = detect_conflicts([&module], &index, ApiVersion::new(1, 0));

        assert!(conflicts.is_empty());
    }

    #[test]
    fn no_conflicts_when_everything_resolves_and_api_versions_match() {
        let index = index_with_versions("crypto-core", &["1.0.0"]);
        let a = manifest_with("a", "1.0", &[("crypto-core", "^1.0")]);

        let conflicts = detect_conflicts([&a], &index, ApiVersion::new(1, 0));

        assert!(conflicts.is_empty());
    }
}
