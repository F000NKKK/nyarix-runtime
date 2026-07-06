//! Platform detection and abstraction types.

/// The target platform for a build or execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Platform {
    /// Linux (x86_64, aarch64)
    Linux,
    /// Windows (x86_64, aarch64)
    Windows,
    /// macOS (x86_64, aarch64)
    MacOs,
    /// iOS
    Ios,
    /// Android
    Android,
    /// Unknown platform
    Unknown,
}

impl Platform {
    /// Detect the current platform at compile time.
    #[must_use]
    pub const fn current() -> Self {
        if cfg!(target_os = "linux") {
            Self::Linux
        } else if cfg!(target_os = "windows") {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::MacOs
        } else if cfg!(target_os = "ios") {
            Self::Ios
        } else if cfg!(target_os = "android") {
            Self::Android
        } else {
            Self::Unknown
        }
    }

    /// Whether this platform is a mobile OS.
    #[must_use]
    pub const fn is_mobile(&self) -> bool {
        matches!(self, Self::Ios | Self::Android)
    }

    /// Whether this platform is a desktop OS.
    #[must_use]
    pub const fn is_desktop(&self) -> bool {
        matches!(self, Self::Linux | Self::Windows | Self::MacOs)
    }
}
