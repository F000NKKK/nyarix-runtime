//! Package installer: installing a `.nyp` onto disk (see issue #65).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use nyarix_error::PackageError;
use nyarix_package::PackageReader;

use crate::{DiscoveredPackage, DuplicateModule, ModuleIndex};

fn io_error(path: &Path, source: io::Error) -> PackageError {
    PackageError::Io {
        path: path.display().to_string(),
        source,
    }
}

/// The default root packages are installed under: `~/.nyarix/packages/`.
///
/// Returns `None` if neither `HOME` (Unix) nor `USERPROFILE` (Windows)
/// is set — same convention as [`crate::default_search_paths`] (#50).
#[must_use]
pub fn default_install_root() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    Some(PathBuf::from(home).join(".nyarix").join("packages"))
}

/// The outcome of [`install_package`].
#[derive(Debug)]
pub enum InstallOutcome {
    /// Installed for the first time, at this path.
    Installed(DiscoveredPackage),
    /// This exact name+version was already installed (see
    /// [`ModuleIndex::insert`]'s duplicate check, #50) — not
    /// re-installed, and `index` is unchanged.
    AlreadyInstalled(DuplicateModule),
}

/// Install `data` (a `.nyp` archive's raw bytes) under `install_root`,
/// registering it in `index` (#50's index doubles as the "local index"
/// this issue asks for — it already tracks name+version+path and
/// already detects exact duplicates, which is this issue's "conflict
/// with already-installed versions" check).
///
/// Layout: `<install_root>/<name>/<version>/package.nyp` (the archive,
/// copied verbatim) and `<install_root>/<name>/<version>/payload/...`
/// (the `payload/` member unpacked, so #57's eventual code loader has
/// plain files to read instead of needing to re-open the archive).
///
/// # Errors
/// Returns [`PackageError`] if `data` doesn't parse (see
/// [`PackageReader::open`]), if creating directories or writing files
/// under `install_root` fails, or if a `payload/` entry's path would
/// escape the destination directory (e.g. via `..` components — treated
/// as a corrupt/hostile archive, not followed).
pub fn install_package(
    data: &[u8],
    install_root: &Path,
    index: &mut ModuleIndex,
) -> Result<InstallOutcome, PackageError> {
    let reader = PackageReader::open(data)?;
    let name = reader.manifest().package.name.clone();
    let version = reader.manifest().package.version.clone();

    let package_dir = install_root.join(&name).join(version.to_string());
    fs::create_dir_all(&package_dir).map_err(|source| io_error(&package_dir, source))?;

    let archive_path = package_dir.join("package.nyp");
    fs::write(&archive_path, data).map_err(|source| io_error(&archive_path, source))?;

    let payload_dir = package_dir.join("payload");
    for (path, contents) in reader.entries()? {
        let Some(relative) = path.strip_prefix("payload/") else {
            continue;
        };
        if relative.is_empty() {
            continue;
        }
        let relative_path = Path::new(relative);
        if relative_path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
        {
            return Err(io_error(
                relative_path,
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("payload entry escapes payload/: {path}"),
                ),
            ));
        }

        let destination = payload_dir.join(relative_path);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(|source| io_error(parent, source))?;
        }
        fs::write(&destination, contents).map_err(|source| io_error(&destination, source))?;
    }

    let discovered = DiscoveredPackage {
        path: archive_path,
        name,
        version,
    };
    Ok(match index.insert(discovered.clone()) {
        Some(duplicate) => {
            tracing::info!(
                package = %discovered.name,
                version = %discovered.version,
                "package already installed"
            );
            InstallOutcome::AlreadyInstalled(duplicate)
        }
        None => {
            tracing::info!(
                package = %discovered.name,
                version = %discovered.version,
                "installed package"
            );
            InstallOutcome::Installed(discovered)
        }
    })
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
            "nyarix-loader-installer-test-{}-{}",
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

    #[test]
    fn installs_a_package_and_registers_it_in_the_index() {
        let root = tempdir();
        let data = PackageBuilder::new()
            .add_file("manifest.toml", manifest_toml("a", "0.1.0").into_bytes())
            .build()
            .unwrap();
        let mut index = ModuleIndex::default();

        let outcome = install_package(&data, root.path(), &mut index).unwrap();

        let InstallOutcome::Installed(installed) = outcome else {
            panic!("expected Installed");
        };
        assert_eq!(installed.name, "a");
        assert!(installed.path.exists());
        assert_eq!(index.len(), 1);
    }

    #[test]
    fn unpacks_payload_into_its_own_directory() {
        let root = tempdir();
        let data = PackageBuilder::new()
            .add_file("manifest.toml", manifest_toml("a", "0.1.0").into_bytes())
            .add_file("payload/module.wasm", b"fake wasm".as_slice())
            .add_file("payload/nested/extra.bin", b"extra".as_slice())
            .build()
            .unwrap();
        let mut index = ModuleIndex::default();

        install_package(&data, root.path(), &mut index).unwrap();

        let payload_file = root
            .path()
            .join("a")
            .join("0.1.0")
            .join("payload")
            .join("module.wasm");
        assert_eq!(fs::read(&payload_file).unwrap(), b"fake wasm");

        let nested_file = root
            .path()
            .join("a")
            .join("0.1.0")
            .join("payload")
            .join("nested")
            .join("extra.bin");
        assert_eq!(fs::read(&nested_file).unwrap(), b"extra");
    }

    #[test]
    fn copies_the_raw_archive_verbatim() {
        let root = tempdir();
        let data = PackageBuilder::new()
            .add_file("manifest.toml", manifest_toml("a", "0.1.0").into_bytes())
            .build()
            .unwrap();
        let mut index = ModuleIndex::default();

        install_package(&data, root.path(), &mut index).unwrap();

        let archive_path = root.path().join("a").join("0.1.0").join("package.nyp");
        assert_eq!(fs::read(&archive_path).unwrap(), data);
    }

    #[test]
    fn reinstalling_the_same_name_and_version_reports_already_installed() {
        let root = tempdir();
        let data = PackageBuilder::new()
            .add_file("manifest.toml", manifest_toml("a", "0.1.0").into_bytes())
            .build()
            .unwrap();
        let mut index = ModuleIndex::default();

        install_package(&data, root.path(), &mut index).unwrap();
        let second = install_package(&data, root.path(), &mut index).unwrap();

        assert!(matches!(second, InstallOutcome::AlreadyInstalled(_)));
        assert_eq!(index.len(), 1);
    }

    #[test]
    fn different_versions_of_the_same_name_both_install() {
        let root = tempdir();
        let mut index = ModuleIndex::default();

        let v1 = PackageBuilder::new()
            .add_file("manifest.toml", manifest_toml("a", "0.1.0").into_bytes())
            .build()
            .unwrap();
        let v2 = PackageBuilder::new()
            .add_file("manifest.toml", manifest_toml("a", "0.2.0").into_bytes())
            .build()
            .unwrap();

        install_package(&v1, root.path(), &mut index).unwrap();
        let outcome = install_package(&v2, root.path(), &mut index).unwrap();

        assert!(matches!(outcome, InstallOutcome::Installed(_)));
        assert_eq!(index.get("a").len(), 2);
    }

    #[test]
    fn default_install_root_ends_in_dot_nyarix_packages() {
        if let Some(root) = default_install_root() {
            assert!(root.ends_with(".nyarix/packages"));
        }
    }
}
