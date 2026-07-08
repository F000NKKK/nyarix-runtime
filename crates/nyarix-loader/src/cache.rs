//! Package cache: on-disk storage and indexing (see issue #66).
//!
//! **Scope note:** "cleanup of old/unused versions" is implemented as
//! eviction by install time (oldest first) once the cache exceeds a
//! size limit — [`PackageCache::evict_to_fit`]. "Unused" in the sense
//! of *actually not loaded in a while* would need something recording
//! when a package was last resolved/instantiated, which nothing in this
//! workspace does yet (Module Loader integration, #41, is the earliest
//! place that would exist) — install time is the honest proxy available
//! today.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use semver::Version;
use serde::{Deserialize, Serialize};

/// One entry in the cache's persistent index (`packages.json`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CacheEntry {
    /// The package's name.
    pub name: String,
    /// The package's version.
    pub version: Version,
    /// Where the installed `package.nyp` lives (see #65's installer).
    pub path: PathBuf,
    /// When this entry was recorded.
    pub installed_at: DateTime<Utc>,
    /// The installed package's size on disk, in bytes (just the
    /// archive — not the unpacked `payload/` directory alongside it).
    pub size_bytes: u64,
}

/// A cache operation failed.
#[derive(Debug, thiserror::Error)]
pub enum CacheError {
    /// Reading or writing the cache directory/index failed.
    #[error("failed to access package cache at {path}: {source}")]
    Io {
        /// The path that couldn't be accessed.
        path: PathBuf,
        /// The underlying I/O error.
        source: io::Error,
    },
    /// `packages.json` exists but isn't valid JSON for [`CacheEntry`]s.
    #[error("invalid package cache index at {path}: {source}")]
    InvalidIndex {
        /// The index file's path.
        path: PathBuf,
        /// The underlying JSON error.
        source: serde_json::Error,
    },
}

/// A local package cache: a directory holding installed packages'
/// metadata, indexed in a `packages.json` file.
#[derive(Debug)]
pub struct PackageCache {
    root: PathBuf,
    entries: Vec<CacheEntry>,
}

impl PackageCache {
    /// The default cache root: `~/.nyarix/cache/`.
    ///
    /// Returns `None` if neither `HOME` (Unix) nor `USERPROFILE`
    /// (Windows) is set — same convention as [`crate::default_search_paths`]
    /// (#50) and [`crate::default_install_root`] (#65).
    #[must_use]
    pub fn default_root() -> Option<PathBuf> {
        let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
        Some(PathBuf::from(home).join(".nyarix").join("cache"))
    }

    /// Where the index file lives within `root`.
    #[must_use]
    pub fn index_path(root: &Path) -> PathBuf {
        root.join("packages.json")
    }

    /// Open (or create) a cache rooted at `root`, reading its
    /// `packages.json` index if one already exists there.
    ///
    /// # Errors
    /// Returns [`CacheError::Io`] if the index exists but can't be
    /// read, or [`CacheError::InvalidIndex`] if it exists but isn't
    /// valid JSON.
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, CacheError> {
        let root = root.into();
        let index_path = Self::index_path(&root);

        let entries = match fs::read_to_string(&index_path) {
            Ok(contents) => {
                serde_json::from_str(&contents).map_err(|source| CacheError::InvalidIndex {
                    path: index_path.clone(),
                    source,
                })?
            }
            Err(source) if source.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(source) => {
                return Err(CacheError::Io {
                    path: index_path,
                    source,
                });
            }
        };

        Ok(Self { root, entries })
    }

    /// Persist the current index to `packages.json`, creating the cache
    /// root directory if it doesn't exist yet.
    ///
    /// # Errors
    /// Returns [`CacheError::Io`] if the directory can't be created or
    /// the index can't be written.
    pub fn save(&self) -> Result<(), CacheError> {
        fs::create_dir_all(&self.root).map_err(|source| CacheError::Io {
            path: self.root.clone(),
            source,
        })?;
        let index_path = Self::index_path(&self.root);
        // `CacheEntry` round-trips through `serde_json` cleanly (no
        // untagged/flattened fields to trip on), so this can't actually
        // fail — but propagate rather than `expect` in case that ever
        // changes.
        let json = serde_json::to_string_pretty(&self.entries).map_err(|source| {
            CacheError::InvalidIndex {
                path: index_path.clone(),
                source,
            }
        })?;
        fs::write(&index_path, json).map_err(|source| CacheError::Io {
            path: index_path,
            source,
        })
    }

    /// Record `entry`, replacing any existing entry with the same name
    /// and version.
    pub fn record(&mut self, entry: CacheEntry) {
        self.entries
            .retain(|existing| !(existing.name == entry.name && existing.version == entry.version));
        self.entries.push(entry);
    }

    /// Every entry currently in the index.
    #[must_use]
    pub fn entries(&self) -> &[CacheEntry] {
        &self.entries
    }

    /// Total size, in bytes, of every entry's recorded `size_bytes`.
    #[must_use]
    pub fn total_size(&self) -> u64 {
        self.entries.iter().map(|entry| entry.size_bytes).sum()
    }

    /// Evict entries — oldest [`CacheEntry::installed_at`] first —
    /// until [`Self::total_size`] is at or under `max_bytes`, deleting
    /// each evicted entry's install directory (`path`'s parent) from
    /// disk on a best-effort basis (a failure to delete doesn't stop
    /// eviction or get reported — the index no longer lists it either
    /// way).
    ///
    /// Returns the evicted entries, oldest first.
    pub fn evict_to_fit(&mut self, max_bytes: u64) -> Vec<CacheEntry> {
        self.entries.sort_by_key(|entry| entry.installed_at);

        let mut evicted = Vec::new();
        while self.total_size() > max_bytes && !self.entries.is_empty() {
            let removed = self.entries.remove(0);
            if let Some(install_dir) = removed.path.parent() {
                let _ = fs::remove_dir_all(install_dir);
            }
            evicted.push(removed);
        }
        evicted
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
            "nyarix-loader-cache-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }

    fn entry(
        name: &str,
        version: &str,
        installed_at: DateTime<Utc>,
        size_bytes: u64,
    ) -> CacheEntry {
        CacheEntry {
            name: name.to_string(),
            version: Version::parse(version).unwrap(),
            path: PathBuf::from(format!("/fake/{name}/{version}/package.nyp")),
            installed_at,
            size_bytes,
        }
    }

    #[test]
    fn opening_a_nonexistent_cache_starts_empty() {
        let dir = tempdir();
        let cache = PackageCache::open(dir.path()).unwrap();
        assert!(cache.entries().is_empty());
    }

    #[test]
    fn saved_entries_survive_a_reopen() {
        let dir = tempdir();
        let mut cache = PackageCache::open(dir.path()).unwrap();
        cache.record(entry("a", "0.1.0", Utc::now(), 100));
        cache.save().unwrap();

        let reopened = PackageCache::open(dir.path()).unwrap();
        assert_eq!(reopened.entries().len(), 1);
        assert_eq!(reopened.entries()[0].name, "a");
    }

    #[test]
    fn recording_the_same_name_and_version_replaces_the_previous_entry() {
        let dir = tempdir();
        let mut cache = PackageCache::open(dir.path()).unwrap();
        cache.record(entry("a", "0.1.0", Utc::now(), 100));
        cache.record(entry("a", "0.1.0", Utc::now(), 200));

        assert_eq!(cache.entries().len(), 1);
        assert_eq!(cache.entries()[0].size_bytes, 200);
    }

    #[test]
    fn total_size_sums_every_entry() {
        let dir = tempdir();
        let mut cache = PackageCache::open(dir.path()).unwrap();
        cache.record(entry("a", "0.1.0", Utc::now(), 100));
        cache.record(entry("b", "0.1.0", Utc::now(), 250));

        assert_eq!(cache.total_size(), 350);
    }

    #[test]
    fn evict_to_fit_removes_the_oldest_entries_first() {
        let dir = tempdir();
        let mut cache = PackageCache::open(dir.path()).unwrap();
        let old = Utc::now() - chrono::Duration::days(2);
        let newer = Utc::now() - chrono::Duration::days(1);
        cache.record(entry("old", "0.1.0", old, 100));
        cache.record(entry("newer", "0.1.0", newer, 100));

        let evicted = cache.evict_to_fit(100);

        assert_eq!(evicted.len(), 1);
        assert_eq!(evicted[0].name, "old");
        assert_eq!(cache.entries().len(), 1);
        assert_eq!(cache.entries()[0].name, "newer");
    }

    #[test]
    fn evict_to_fit_does_nothing_when_already_under_the_limit() {
        let dir = tempdir();
        let mut cache = PackageCache::open(dir.path()).unwrap();
        cache.record(entry("a", "0.1.0", Utc::now(), 100));

        let evicted = cache.evict_to_fit(1000);

        assert!(evicted.is_empty());
        assert_eq!(cache.entries().len(), 1);
    }

    #[test]
    fn default_root_ends_in_dot_nyarix_cache() {
        if let Some(root) = PackageCache::default_root() {
            assert!(root.ends_with(".nyarix/cache"));
        }
    }

    #[test]
    fn an_invalid_index_file_is_a_clear_error() {
        let dir = tempdir();
        fs::write(PackageCache::index_path(dir.path()), "not valid json").unwrap();

        let err = PackageCache::open(dir.path()).unwrap_err();

        assert!(matches!(err, CacheError::InvalidIndex { .. }));
    }
}
