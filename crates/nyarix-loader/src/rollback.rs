//! Package rollback (see issue #67): reverting a package to its
//! previously-installed version.
//!
//! **Scope note:** "сохранение предыдущей версии при обновлении" needs
//! no new code here — #65's installer already never deletes or
//! overwrites a version's directory when installing a newer one
//! (`<install_root>/<name>/<version>/...`), so every previously
//! installed version simply stays on disk. What was missing, and what
//! this module adds, is a record of *which* version is currently
//! active per package name, so "the previous one" is a well-defined
//! question — [`RollbackHistory`], an append-only log persisted
//! alongside the install root.
//!
//! **Scope note:** the issue's `nyarix rollback <package>` CLI command
//! isn't implemented — this workspace has no CLI binary or argument
//! parser yet (`clap` isn't a dependency anywhere), same gap already
//! tracked for the signing CLI in #110. [`rollback_package`] is the
//! library-level operation a future `nyarix-cli` crate would call.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use nyarix_error::PackageError;
use nyarix_module_api::ApiVersion;
use nyarix_package::TrustStore;
use semver::Version;
use serde::{Deserialize, Serialize};

use crate::ModuleIndex;
use crate::conflict::Conflict;
use crate::validation::validate_package;

/// Whether a [`HistoryEntry`] recorded an install or a rollback.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HistoryKind {
    /// A package version was installed (first time or an update).
    Installed,
    /// A package was rolled back to this version.
    RolledBack,
}

/// One entry in the on-disk install/rollback history (`history.json`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistoryEntry {
    /// The package's name.
    pub name: String,
    /// The version that became active as of this entry.
    pub version: Version,
    /// Whether this was an install or a rollback.
    pub kind: HistoryKind,
    /// When this entry was recorded.
    pub at: DateTime<Utc>,
}

/// A rollback (or history-log) operation failed.
#[derive(Debug, thiserror::Error)]
pub enum RollbackError {
    /// Reading or writing the history file failed.
    #[error("failed to access rollback history at {path}: {source}")]
    Io {
        /// The path that couldn't be accessed.
        path: PathBuf,
        /// The underlying I/O error.
        source: io::Error,
    },
    /// `history.json` exists but isn't valid JSON for [`HistoryEntry`]s.
    #[error("invalid rollback history at {path}: {source}")]
    InvalidHistory {
        /// The history file's path.
        path: PathBuf,
        /// The underlying JSON error.
        source: serde_json::Error,
    },
    /// No install was ever recorded for this package name.
    #[error("no installed version recorded for package {0}")]
    NoActiveVersion(String),
    /// Only one version was ever recorded active for this package —
    /// there's nothing to roll back to.
    #[error("no previous version to roll back to for package {0}")]
    NoPreviousVersion(String),
    /// The previous version's history entry exists, but its archive is
    /// no longer on disk (e.g. evicted by [`crate::PackageCache::evict_to_fit`],
    /// or removed by hand).
    #[error("previous version {version} of {name} is no longer present at {path}")]
    MissingArchive {
        /// The package's name.
        name: String,
        /// The version that should have been on disk.
        version: Version,
        /// Where it was expected.
        path: PathBuf,
    },
    /// The previous version's archive couldn't even be parsed.
    #[error(transparent)]
    Package(#[from] PackageError),
    /// The previous version failed re-validation against the current
    /// environment (see [`validate_package`]) — rolling back to it
    /// would just reintroduce whatever conflict it originally had, or a
    /// new one introduced since (e.g. a sibling package installed after
    /// it that it now conflicts with).
    #[error(
        "previous version {version} of {name} is not compatible with the current environment: {conflicts:?}"
    )]
    Incompatible {
        /// The package's name.
        name: String,
        /// The incompatible previous version.
        version: Version,
        /// Why it's incompatible.
        conflicts: Vec<Conflict>,
    },
}

fn io_error(path: &Path, source: io::Error) -> RollbackError {
    RollbackError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// An append-only log of every install/rollback for every package,
/// persisted to `<root>/history.json`.
///
/// [`Self::active_version`]/[`Self::previous_version`] derive "current"
/// and "previous" from the last two entries recorded for a given
/// package name — so rolling back twice in a row for the same package
/// alternates between its two most recent versions rather than walking
/// further back; that matches the issue's "return to previous", not a
/// full undo stack.
#[derive(Debug)]
pub struct RollbackHistory {
    path: PathBuf,
    entries: Vec<HistoryEntry>,
}

impl RollbackHistory {
    /// Where the history file lives within `root`.
    #[must_use]
    pub fn file_path(root: &Path) -> PathBuf {
        root.join("history.json")
    }

    /// Open (or create) a history log rooted at `root`, reading its
    /// `history.json` if one already exists there.
    ///
    /// # Errors
    /// Returns [`RollbackError::Io`] if the file exists but can't be
    /// read, or [`RollbackError::InvalidHistory`] if it exists but
    /// isn't valid JSON.
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, RollbackError> {
        let root = root.into();
        let path = Self::file_path(&root);

        let entries = match fs::read_to_string(&path) {
            Ok(contents) => {
                serde_json::from_str(&contents).map_err(|source| RollbackError::InvalidHistory {
                    path: path.clone(),
                    source,
                })?
            }
            Err(source) if source.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(source) => return Err(io_error(&path, source)),
        };

        Ok(Self { path, entries })
    }

    /// Persist the current log to `history.json`, creating the parent
    /// directory if it doesn't exist yet.
    ///
    /// # Errors
    /// Returns [`RollbackError::Io`] if the directory can't be created
    /// or the file can't be written.
    pub fn save(&self) -> Result<(), RollbackError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|source| io_error(parent, source))?;
        }
        let json = serde_json::to_string_pretty(&self.entries).map_err(|source| {
            RollbackError::InvalidHistory {
                path: self.path.clone(),
                source,
            }
        })?;
        fs::write(&self.path, json).map_err(|source| io_error(&self.path, source))
    }

    /// Record that `version` of `name` was installed and is now active.
    pub fn record_install(&mut self, name: &str, version: Version) {
        self.entries.push(HistoryEntry {
            name: name.to_string(),
            version,
            kind: HistoryKind::Installed,
            at: Utc::now(),
        });
    }

    /// Record that `name` was rolled back to `version`, which is now
    /// active.
    fn record_rollback(&mut self, name: &str, version: Version) {
        self.entries.push(HistoryEntry {
            name: name.to_string(),
            version,
            kind: HistoryKind::RolledBack,
            at: Utc::now(),
        });
    }

    /// Every entry currently in the log, oldest first.
    #[must_use]
    pub fn entries(&self) -> &[HistoryEntry] {
        &self.entries
    }

    /// The version currently active for `name` — the version of its
    /// most recent entry (install or rollback) — if any was ever
    /// recorded.
    #[must_use]
    pub fn active_version(&self, name: &str) -> Option<&Version> {
        self.entries
            .iter()
            .rev()
            .find(|entry| entry.name == name)
            .map(|entry| &entry.version)
    }

    /// The version that was active for `name` immediately before the
    /// current one, if at least two entries were recorded for it.
    #[must_use]
    pub fn previous_version(&self, name: &str) -> Option<&Version> {
        self.entries
            .iter()
            .rev()
            .filter(|entry| entry.name == name)
            .nth(1)
            .map(|entry| &entry.version)
    }
}

/// Roll `name` back to its previously active version under
/// `install_root` (see [`crate::installer::install_package`]'s layout —
/// `<install_root>/<name>/<version>/package.nyp`).
///
/// Re-validates the previous version's archive with
/// [`validate_package`] before switching to it (the issue's "проверка
/// совместимости откатываемой версии") — using `other_manifests: []`,
/// since re-checking it against every other *currently* discovered
/// package's manifest isn't this function's job (a caller doing a full
/// reload already has that context and can re-run [`validate_package`]
/// itself if it wants the stricter check); this call catches the
/// previous version's *own* structural/platform/API-version validity,
/// which is what would make rolling back to it actively harmful rather
/// than just possibly redundant.
///
/// On success, records the rollback in `history` (still in memory —
/// call [`RollbackHistory::save`] to persist it) and returns the
/// version now active.
///
/// # Errors
/// Returns [`RollbackError::NoActiveVersion`] if `name` was never
/// installed, [`RollbackError::NoPreviousVersion`] if only one version
/// was ever recorded for it, [`RollbackError::MissingArchive`] if the
/// previous version's `package.nyp` isn't on disk anymore,
/// [`RollbackError::Package`] if it's on disk but doesn't parse, or
/// [`RollbackError::Incompatible`] if it fails re-validation.
pub fn rollback_package(
    name: &str,
    install_root: &Path,
    history: &mut RollbackHistory,
    trust_store: &TrustStore,
    index: &ModuleIndex,
    required_api_version: ApiVersion,
) -> Result<Version, RollbackError> {
    if history.active_version(name).is_none() {
        return Err(RollbackError::NoActiveVersion(name.to_string()));
    }
    let previous = history
        .previous_version(name)
        .cloned()
        .ok_or_else(|| RollbackError::NoPreviousVersion(name.to_string()))?;

    let archive_path = install_root
        .join(name)
        .join(previous.to_string())
        .join("package.nyp");
    let data = match fs::read(&archive_path) {
        Ok(data) => data,
        Err(source) if source.kind() == io::ErrorKind::NotFound => {
            return Err(RollbackError::MissingArchive {
                name: name.to_string(),
                version: previous,
                path: archive_path,
            });
        }
        Err(source) => return Err(io_error(&archive_path, source)),
    };

    let report = validate_package(
        &data,
        std::iter::empty(),
        index,
        trust_store,
        required_api_version,
    )?;
    if !report.is_valid() {
        return Err(RollbackError::Incompatible {
            name: name.to_string(),
            version: previous,
            conflicts: report.conflicts,
        });
    }

    tracing::info!(package = name, to = %previous, "rolling back package");
    history.record_rollback(name, previous.clone());
    Ok(previous)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nyarix_package::PackageBuilder;
    use std::time::{SystemTime, UNIX_EPOCH};

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
            "nyarix-loader-rollback-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }

    fn manifest_toml(name: &str, version: &str) -> String {
        format!(
            r#"
[package]
name = "{name}"
version = "{version}"
module_type = "flow"
api_version = "1.0"
author = "Nyarix"
description = "test"
"#
        )
    }

    fn install(root: &Path, name: &str, version: &str) {
        let data = PackageBuilder::new()
            .add_file("manifest.toml", manifest_toml(name, version).into_bytes())
            .build()
            .unwrap();
        let mut index = ModuleIndex::default();
        crate::installer::install_package(&data, root, &mut index).unwrap();
    }

    #[test]
    fn history_round_trips_through_save_and_open() {
        let dir = tempdir();
        let mut history = RollbackHistory::open(dir.path()).unwrap();
        history.record_install("a", Version::new(0, 1, 0));
        history.save().unwrap();

        let reopened = RollbackHistory::open(dir.path()).unwrap();
        assert_eq!(reopened.entries().len(), 1);
        assert_eq!(reopened.active_version("a"), Some(&Version::new(0, 1, 0)));
    }

    #[test]
    fn active_version_is_the_most_recently_recorded_one() {
        let mut history = RollbackHistory::open(std::env::temp_dir()).unwrap();
        history.record_install("a", Version::new(0, 1, 0));
        history.record_install("a", Version::new(0, 2, 0));

        assert_eq!(history.active_version("a"), Some(&Version::new(0, 2, 0)));
        assert_eq!(history.previous_version("a"), Some(&Version::new(0, 1, 0)));
    }

    #[test]
    fn a_package_with_only_one_install_has_no_previous_version() {
        let mut history = RollbackHistory::open(std::env::temp_dir()).unwrap();
        history.record_install("a", Version::new(0, 1, 0));

        assert_eq!(history.previous_version("a"), None);
    }

    #[test]
    fn an_unknown_package_has_no_active_version() {
        let history = RollbackHistory::open(std::env::temp_dir()).unwrap();
        assert_eq!(history.active_version("missing"), None);
    }

    #[test]
    fn rolling_back_returns_to_the_previous_version_and_records_it() {
        let root = tempdir();
        install(root.path(), "a", "0.1.0");
        install(root.path(), "a", "0.2.0");

        let mut history = RollbackHistory::open(root.path()).unwrap();
        history.record_install("a", Version::new(0, 1, 0));
        history.record_install("a", Version::new(0, 2, 0));

        let version = rollback_package(
            "a",
            root.path(),
            &mut history,
            &TrustStore::new(),
            &ModuleIndex::default(),
            ApiVersion::new(1, 0),
        )
        .unwrap();

        assert_eq!(version, Version::new(0, 1, 0));
        assert_eq!(history.active_version("a"), Some(&Version::new(0, 1, 0)));
        assert_eq!(
            history.entries().last().unwrap().kind,
            HistoryKind::RolledBack
        );
    }

    #[test]
    fn rolling_back_a_never_installed_package_is_a_clear_error() {
        let root = tempdir();
        let mut history = RollbackHistory::open(root.path()).unwrap();

        let err = rollback_package(
            "missing",
            root.path(),
            &mut history,
            &TrustStore::new(),
            &ModuleIndex::default(),
            ApiVersion::new(1, 0),
        )
        .unwrap_err();

        assert!(matches!(err, RollbackError::NoActiveVersion(name) if name == "missing"));
    }

    #[test]
    fn rolling_back_with_only_one_version_ever_installed_is_a_clear_error() {
        let root = tempdir();
        install(root.path(), "a", "0.1.0");

        let mut history = RollbackHistory::open(root.path()).unwrap();
        history.record_install("a", Version::new(0, 1, 0));

        let err = rollback_package(
            "a",
            root.path(),
            &mut history,
            &TrustStore::new(),
            &ModuleIndex::default(),
            ApiVersion::new(1, 0),
        )
        .unwrap_err();

        assert!(matches!(err, RollbackError::NoPreviousVersion(name) if name == "a"));
    }

    #[test]
    fn rolling_back_to_a_version_whose_archive_was_removed_is_a_clear_error() {
        let root = tempdir();
        install(root.path(), "a", "0.1.0");
        install(root.path(), "a", "0.2.0");
        fs::remove_dir_all(root.path().join("a").join("0.1.0")).unwrap();

        let mut history = RollbackHistory::open(root.path()).unwrap();
        history.record_install("a", Version::new(0, 1, 0));
        history.record_install("a", Version::new(0, 2, 0));

        let err = rollback_package(
            "a",
            root.path(),
            &mut history,
            &TrustStore::new(),
            &ModuleIndex::default(),
            ApiVersion::new(1, 0),
        )
        .unwrap_err();

        assert!(matches!(err, RollbackError::MissingArchive { .. }));
    }

    #[test]
    fn rolling_back_to_an_incompatible_previous_version_is_a_clear_error() {
        let root = tempdir();

        // "0.1.0" declares support for platforms this test can't be
        // running on, so re-validating it fails the platform check.
        let incompatible = PackageBuilder::new()
            .add_file(
                "manifest.toml",
                r#"
[package]
name = "a"
version = "0.1.0"
module_type = "flow"
api_version = "1.0"
author = "Nyarix"
description = "test"

[platforms]
supported = ["ios", "android"]
"#
                .as_bytes()
                .to_vec(),
            )
            .build()
            .unwrap();
        let mut index = ModuleIndex::default();
        crate::installer::install_package(&incompatible, root.path(), &mut index).unwrap();
        install(root.path(), "a", "0.2.0");

        let mut history = RollbackHistory::open(root.path()).unwrap();
        history.record_install("a", Version::new(0, 1, 0));
        history.record_install("a", Version::new(0, 2, 0));

        if !matches!(
            nyarix_module_api::Platform::current(),
            nyarix_module_api::Platform::Ios | nyarix_module_api::Platform::Android
        ) {
            let err = rollback_package(
                "a",
                root.path(),
                &mut history,
                &TrustStore::new(),
                &ModuleIndex::default(),
                ApiVersion::new(1, 0),
            )
            .unwrap_err();

            assert!(matches!(err, RollbackError::Incompatible { .. }));
        }
    }
}
