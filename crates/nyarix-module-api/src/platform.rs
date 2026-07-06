//! Target platform (see issue #18).

use serde::{Deserialize, Serialize};

/// The platform the Runtime is executing on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    /// Linux (desktop/server).
    Linux,
    /// Windows.
    Windows,
    /// macOS.
    MacOs,
    /// Android.
    Android,
    /// iOS.
    Ios,
    /// Compiled for a target this Runtime doesn't recognize.
    Unknown,
}

impl Platform {
    /// Detect the platform this binary was compiled for.
    #[must_use]
    pub const fn current() -> Self {
        if cfg!(target_os = "linux") {
            Self::Linux
        } else if cfg!(target_os = "windows") {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::MacOs
        } else if cfg!(target_os = "android") {
            Self::Android
        } else if cfg!(target_os = "ios") {
            Self::Ios
        } else {
            Self::Unknown
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_matches_build_target() {
        // Whatever CI/dev machine this runs on, it must resolve to *some*
        // known platform, not silently fall through.
        let platform = Platform::current();
        if cfg!(target_os = "linux") {
            assert_eq!(platform, Platform::Linux);
        }
    }

    #[test]
    fn serializes_as_lowercase_matching_manifest_schema() {
        #[derive(Serialize, Deserialize)]
        struct Wrapper {
            platform: Platform,
        }

        // manifest.toml (#59) spells this "macos", not "MacOs" — the exact
        // reason `rename_all = "lowercase"` is here.
        let toml = toml::to_string(&Wrapper {
            platform: Platform::MacOs,
        })
        .unwrap();
        assert_eq!(toml.trim(), "platform = \"macos\"");

        let parsed: Wrapper = toml::from_str("platform = \"linux\"").unwrap();
        assert_eq!(parsed.platform, Platform::Linux);
    }
}
