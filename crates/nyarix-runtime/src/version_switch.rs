//! Version switching (see issue #68): moving an already-loaded package
//! to a different installed version at runtime.
//!
//! **Scope note:** producing a *new* `Box<dyn Module>` instance for the
//! target version's code isn't implemented here — nothing in this
//! workspace can load `payload/` bytes into a live module yet, tracked
//! by #107 (blocks #57's [`nyarix_loader::instantiate`] the same way).
//! [`switch_version`] operates on an already-in-hand `&mut dyn Module`
//! (whatever the caller already produced for the target version, e.g.
//! via a test double today, or a real instantiation once #107 lands)
//! and answers the two questions that *are* fully implementable now:
//! is the target version even present in the cache, and can this
//! module migrate in place or does the caller need to fall back to a
//! full graph restart. "Перезапуск графа" itself is just
//! [`crate::shutdown_all_nodes`] followed by rebuilding and
//! [`crate::initialize_all_nodes`] — both already exist (#43/#44); this
//! module only decides *when* that fallback is needed, it doesn't
//! re-implement graph teardown/rebuild itself.

use nyarix_loader::PackageCache;
use nyarix_module_api::{Module, RuntimeContext};
use semver::Version;

/// Switching a package's active version failed.
#[derive(Debug, thiserror::Error)]
pub enum VersionSwitchError {
    /// The requested version isn't present in the cache — nothing to
    /// switch to.
    #[error("package {name} version {version} is not present in the cache")]
    NotCached {
        /// The package's name.
        name: String,
        /// The version that wasn't found.
        version: Version,
    },
    /// The module claimed to support migration but [`Module::migrate`]
    /// returned an error.
    #[error(transparent)]
    MigrationFailed(#[from] nyarix_error::ModuleError),
}

/// The outcome of [`switch_version`]: whether the module migrated in
/// place, or the caller needs to fall back to a full graph restart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwitchOutcome {
    /// [`Module::migrate`] was called and succeeded — the module is now
    /// running the target version in place, no graph rebuild needed.
    HotSwapped,
    /// The module doesn't support live migration
    /// ([`Module::supports_migration`] returned `false`) — the caller
    /// must shut down and rebuild the graph (see this module's docs)
    /// to actually run the target version.
    RestartRequired,
}

/// Whether `name`'s `version` is present in `cache` — the issue's
/// "Проверка наличия версии в кэше".
#[must_use]
pub fn version_is_cached(cache: &PackageCache, name: &str, version: &Version) -> bool {
    cache
        .entries()
        .iter()
        .any(|entry| entry.name == name && &entry.version == version)
}

/// Switch `module` (already representing `name`) to `target_version`.
///
/// First checks [`version_is_cached`] — returns
/// [`VersionSwitchError::NotCached`] if the target version was never
/// installed. Otherwise, if `module.supports_migration()`, calls
/// [`Module::migrate`] and returns [`SwitchOutcome::HotSwapped`] on
/// success (propagating any migration error); if not, returns
/// [`SwitchOutcome::RestartRequired`] without touching `module` at all,
/// leaving it running its current version until the caller rebuilds
/// the graph.
///
/// # Errors
/// See [`VersionSwitchError`].
pub fn switch_version(
    name: &str,
    target_version: &Version,
    cache: &PackageCache,
    module: &mut dyn Module,
    ctx: &RuntimeContext,
) -> Result<SwitchOutcome, VersionSwitchError> {
    if !version_is_cached(cache, name, target_version) {
        return Err(VersionSwitchError::NotCached {
            name: name.to_string(),
            version: target_version.clone(),
        });
    }

    if !module.supports_migration() {
        return Ok(SwitchOutcome::RestartRequired);
    }

    module.migrate(ctx)?;
    Ok(SwitchOutcome::HotSwapped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use nyarix_error::ModuleError;
    use nyarix_loader::CacheEntry;
    use nyarix_module_api::{Health, ModuleMetadata, ModuleType};
    use nyarix_packet::Packet;
    use std::path::PathBuf;

    struct StubModule {
        metadata: ModuleMetadata,
        migratable: bool,
        migrate_calls: usize,
        fail_migration: bool,
    }

    impl StubModule {
        fn new(migratable: bool) -> Self {
            Self {
                metadata: ModuleMetadata::new(
                    "a",
                    semver::Version::new(0, 1, 0),
                    ModuleType::Flow,
                ),
                migratable,
                migrate_calls: 0,
                fail_migration: false,
            }
        }
    }

    impl Module for StubModule {
        fn metadata(&self) -> &ModuleMetadata {
            &self.metadata
        }

        fn initialize(&mut self, _ctx: &RuntimeContext) -> Result<(), ModuleError> {
            Ok(())
        }

        fn process(&mut self, packet: Packet) -> Result<Option<Packet>, ModuleError> {
            Ok(Some(packet))
        }

        fn shutdown(&mut self, _ctx: &RuntimeContext) -> Result<(), ModuleError> {
            Ok(())
        }

        fn health(&self) -> Health {
            Health::Healthy
        }

        fn migrate(&mut self, _ctx: &RuntimeContext) -> Result<(), ModuleError> {
            self.migrate_calls += 1;
            if self.fail_migration {
                return Err(ModuleError::InitializationFailed {
                    reason: "simulated migration failure".to_string(),
                });
            }
            Ok(())
        }

        fn supports_migration(&self) -> bool {
            self.migratable
        }
    }

    fn cache_with(name: &str, version: &str) -> PackageCache {
        let mut cache = PackageCache::open(std::env::temp_dir().join(format!(
            "nyarix-runtime-version-switch-test-{}-{}",
            std::process::id(),
            name
        )))
        .unwrap();
        cache.record(CacheEntry {
            name: name.to_string(),
            version: Version::parse(version).unwrap(),
            path: PathBuf::from(format!("/fake/{name}/{version}/package.nyp")),
            installed_at: Utc::now(),
            size_bytes: 100,
        });
        cache
    }

    #[test]
    fn version_is_cached_matches_name_and_version() {
        let cache = cache_with("a", "0.1.0");
        assert!(version_is_cached(&cache, "a", &Version::new(0, 1, 0)));
        assert!(!version_is_cached(&cache, "a", &Version::new(0, 2, 0)));
        assert!(!version_is_cached(&cache, "b", &Version::new(0, 1, 0)));
    }

    #[test]
    fn switching_to_an_uncached_version_is_a_clear_error() {
        let cache = cache_with("a", "0.1.0");
        let mut module = StubModule::new(true);
        let ctx = RuntimeContext::empty();

        let err = switch_version(
            "a",
            &Version::new(0, 2, 0),
            &cache,
            &mut module,
            &ctx,
        )
        .unwrap_err();

        assert!(matches!(err, VersionSwitchError::NotCached { .. }));
        assert_eq!(module.migrate_calls, 0);
    }

    #[test]
    fn a_migratable_module_hot_swaps() {
        let cache = cache_with("a", "0.2.0");
        let mut module = StubModule::new(true);
        let ctx = RuntimeContext::empty();

        let outcome =
            switch_version("a", &Version::new(0, 2, 0), &cache, &mut module, &ctx).unwrap();

        assert_eq!(outcome, SwitchOutcome::HotSwapped);
        assert_eq!(module.migrate_calls, 1);
    }

    #[test]
    fn a_non_migratable_module_requires_a_restart_and_is_not_touched() {
        let cache = cache_with("a", "0.2.0");
        let mut module = StubModule::new(false);
        let ctx = RuntimeContext::empty();

        let outcome =
            switch_version("a", &Version::new(0, 2, 0), &cache, &mut module, &ctx).unwrap();

        assert_eq!(outcome, SwitchOutcome::RestartRequired);
        assert_eq!(module.migrate_calls, 0);
    }

    #[test]
    fn a_failed_migration_propagates_the_error() {
        let cache = cache_with("a", "0.2.0");
        let mut module = StubModule::new(true);
        module.fail_migration = true;
        let ctx = RuntimeContext::empty();

        let err =
            switch_version("a", &Version::new(0, 2, 0), &cache, &mut module, &ctx).unwrap_err();

        assert!(matches!(err, VersionSwitchError::MigrationFailed(_)));
        assert_eq!(module.migrate_calls, 1);
    }
}
