//! `manifest.toml` schema (see issue #59).
//!
//! Reuses the module-facing types from `nyarix-module-api` wherever the
//! schema's field matches one exactly — [`ModuleType`], [`Capability`],
//! [`Platform`], [`ApiVersion`] — rather than re-declaring parallel
//! manifest-only versions of the same concepts.

use std::collections::HashMap;

use nyarix_error::PackageError;
use nyarix_module_api::{ApiVersion, Capability, ModuleType, Platform};
use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};

/// A parsed `manifest.toml`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PackageManifest {
    /// The `[package]` table.
    pub package: PackageInfo,
    /// The `[dependencies]` table: other package names mapped to a
    /// semver requirement on their version (e.g. `"^0.1"`).
    ///
    /// This is the manifest's declared *version* constraint on each
    /// dependency — matching it against what's actually installed, and
    /// resolving the dependency graph as a whole, is the Dependency
    /// resolver's job (#53), not this schema's.
    #[serde(default)]
    pub dependencies: HashMap<String, VersionReq>,
    /// The `[capabilities]` table.
    #[serde(default)]
    pub capabilities: Capabilities,
    /// The `[platforms]` table.
    #[serde(default)]
    pub platforms: Platforms,
}

/// The `[package]` table: identity and versioning metadata.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PackageInfo {
    /// Package name, unique within its registry namespace.
    pub name: String,
    /// Package version.
    pub version: Version,
    /// The functional category of the module this package contains.
    pub module_type: ModuleType,
    /// The Module API version this package was built against (see #25),
    /// spelled `"major.minor"` (e.g. `"1.0"`) in TOML — [`ApiVersion`]'s
    /// own derived (de)serialization is a `{ major, minor }` table, which
    /// isn't what this schema's example shows, so this field uses a
    /// custom string (de)serializer instead.
    #[serde(
        deserialize_with = "deserialize_api_version",
        serialize_with = "serialize_api_version"
    )]
    pub api_version: ApiVersion,
    /// Author name or organization.
    pub author: String,
    /// Human-readable description.
    pub description: String,
}

fn deserialize_api_version<'de, D>(deserializer: D) -> Result<ApiVersion, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = String::deserialize(deserializer)?;
    raw.parse().map_err(serde::de::Error::custom)
}

fn serialize_api_version<S>(version: &ApiVersion, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_str(&version.to_string())
}

/// The `[capabilities]` table.
///
/// `required` and `provided` are deliberately different kinds of thing:
/// `required` draws from the closed, system-defined [`Capability`] enum
/// (#21) — permissions the Runtime's Sandbox (M7) actually grants or
/// denies. `provided` is a free-form list of feature/service tags this
/// package makes available to other packages (e.g. `"transport-udp"`,
/// which is not and will never be a system [`Capability`] — it's a
/// package advertising a service, for the Dependency resolver (#53) to
/// match other packages' `[dependencies]` against, much like Cargo
/// features). Modeling `provided` as `Capability` too would force every
/// package-provided feature name into that closed system-permission
/// enum, which is the wrong shape for an open-ended set of packages.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Capabilities {
    /// System capabilities this package's module needs from the Runtime.
    #[serde(default)]
    pub required: Vec<Capability>,
    /// Feature/service tags this package makes available to others.
    #[serde(default)]
    pub provided: Vec<String>,
}

/// The `[platforms]` table.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Platforms {
    /// Platforms this package supports. Empty means "unspecified"
    /// (assume all platforms), matching
    /// [`nyarix_module_api::ModuleMetadata::platforms`]'s convention.
    #[serde(default)]
    pub supported: Vec<Platform>,
}

impl PackageManifest {
    /// Parse a `manifest.toml` document.
    ///
    /// # Errors
    /// Returns [`PackageError::InvalidManifest`] if `input` isn't valid
    /// TOML, or doesn't match this schema (missing required fields,
    /// wrong types, an unparseable `version`/`api_version`/dependency
    /// requirement, ...).
    pub fn from_toml(input: &str) -> Result<Self, PackageError> {
        toml::from_str(input).map_err(|source| PackageError::InvalidManifest {
            reason: source.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str = r#"
[package]
name = "nyarix-transport-udp"
version = "0.1.0"
module_type = "transport"
api_version = "1.0"
author = "Nyarix"
description = "UDP transport"

[dependencies]
nyarix-crypto-core = "^0.1"

[capabilities]
required = ["network"]
provided = ["transport-udp"]

[platforms]
supported = ["linux", "windows", "macos", "android"]
"#;

    #[test]
    fn parses_the_spec_example() {
        let manifest = PackageManifest::from_toml(EXAMPLE).unwrap();

        assert_eq!(manifest.package.name, "nyarix-transport-udp");
        assert_eq!(manifest.package.version, Version::new(0, 1, 0));
        assert_eq!(manifest.package.module_type, ModuleType::Transport);
        assert_eq!(manifest.package.api_version, ApiVersion::new(1, 0));
        assert_eq!(manifest.package.author, "Nyarix");
        assert_eq!(manifest.package.description, "UDP transport");

        let dep = manifest.dependencies.get("nyarix-crypto-core").unwrap();
        assert_eq!(dep, &VersionReq::parse("^0.1").unwrap());

        assert_eq!(manifest.capabilities.required, vec![Capability::Network]);
        assert_eq!(
            manifest.capabilities.provided,
            vec!["transport-udp".to_string()]
        );

        assert_eq!(
            manifest.platforms.supported,
            vec![
                Platform::Linux,
                Platform::Windows,
                Platform::MacOs,
                Platform::Android
            ]
        );
    }

    #[test]
    fn dependencies_capabilities_and_platforms_default_to_empty() {
        let manifest = PackageManifest::from_toml(
            r#"
[package]
name = "minimal"
version = "0.1.0"
module_type = "flow"
api_version = "1.0"
author = "Nyarix"
description = "minimal package"
"#,
        )
        .unwrap();

        assert!(manifest.dependencies.is_empty());
        assert!(manifest.capabilities.required.is_empty());
        assert!(manifest.capabilities.provided.is_empty());
        assert!(manifest.platforms.supported.is_empty());
    }

    #[test]
    fn rejects_malformed_toml() {
        let err = PackageManifest::from_toml("not valid toml [[[").unwrap_err();
        assert!(matches!(err, PackageError::InvalidManifest { .. }));
    }

    #[test]
    fn rejects_a_missing_required_field() {
        let err = PackageManifest::from_toml(
            r#"
[package]
name = "incomplete"
version = "0.1.0"
"#,
        )
        .unwrap_err();
        assert!(matches!(err, PackageError::InvalidManifest { .. }));
    }

    #[test]
    fn rejects_a_malformed_api_version() {
        let err = PackageManifest::from_toml(
            r#"
[package]
name = "bad-api-version"
version = "0.1.0"
module_type = "flow"
api_version = "not-a-version"
author = "Nyarix"
description = "x"
"#,
        )
        .unwrap_err();
        assert!(matches!(err, PackageError::InvalidManifest { .. }));
    }
}
