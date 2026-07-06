//! Configuration model for Nyarix.
//!
//! Configuration is hierarchical:
//!   global defaults → profile → stack → module → per-flow → per-device
//!
//! All configuration is serializable via Serde (TOML/YAML).
//! Internally config is normalized into a typed schema.

use serde::{Deserialize, Serialize};

/// The operating mode of a Runtime instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum RuntimeMode {
    /// Client mode — originates connections.
    #[default]
    Client,
    /// Server mode — accepts connections.
    Server,
    /// Relay mode — forwards between peers.
    Relay,
    /// Gateway mode — bridges networks.
    Gateway,
    /// Bridge mode — transparent passthrough.
    Bridge,
    /// Diagnostic mode — introspection and testing.
    Diagnostic,
}

/// Top-level Runtime configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeConfig {
    /// The operating mode.
    #[serde(default)]
    pub mode: RuntimeMode,
    /// The active profile name.
    #[serde(default = "default_profile")]
    pub profile: String,
    /// Profile definitions.
    #[serde(default)]
    pub profiles: Vec<ProfileConfig>,
    /// Global defaults applied across all profiles.
    #[serde(default)]
    pub defaults: GlobalDefaults,
    /// Per-device overrides.
    #[serde(default)]
    pub device: Option<DeviceConfig>,
    /// Module-specific overrides.
    #[serde(default)]
    pub modules: std::collections::HashMap<String, toml::Value>,
}

/// A named profile — the user-facing concept of a "stack preset."
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileConfig {
    /// Profile name (e.g., "stealth", "mobile", "gaming").
    pub name: String,
    /// Human-readable description.
    #[serde(default)]
    pub description: String,
    /// Stack definition: ordered list of module names.
    #[serde(default)]
    pub stack: Vec<String>,
    /// Policy overrides for this profile.
    #[serde(default)]
    pub policy: std::collections::HashMap<String, toml::Value>,
}

/// Global defaults applied when not overridden by profile/device.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GlobalDefaults {
    /// Default log level.
    #[serde(default)]
    pub log_level: LogLevel,
    /// Maximum concurrent flows.
    #[serde(default = "default_max_flows")]
    pub max_flows: usize,
    /// Maximum packet queue depth per node.
    #[serde(default = "default_queue_depth")]
    pub queue_depth: usize,
    /// Packet pool size.
    #[serde(default = "default_pool_size")]
    pub pool_size: usize,
    /// Default scheduler config.
    #[serde(default)]
    pub scheduler: SchedulerConfig,
}

/// Per-device configuration overrides.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceConfig {
    /// Device name (for identification).
    #[serde(default)]
    pub name: String,
    /// TUN interface name.
    #[serde(default = "default_tun_name")]
    pub tun_name: String,
    /// TUN MTU.
    #[serde(default = "default_mtu")]
    pub mtu: u16,
    /// Battery-aware mode for mobile.
    #[serde(default)]
    pub battery_aware: bool,
}

/// Scheduler configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchedulerConfig {
    /// Number of I/O worker threads.
    #[serde(default = "default_io_threads")]
    pub io_threads: usize,
    /// Number of CPU worker threads.
    #[serde(default = "default_cpu_threads")]
    pub cpu_threads: usize,
    /// Maximum background task count.
    #[serde(default = "default_max_background")]
    pub max_background_tasks: usize,
}

/// Log level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    /// Trace level — very verbose.
    Trace,
    /// Debug level.
    Debug,
    /// Info level (default).
    #[default]
    Info,
    /// Warning level.
    Warn,
    /// Error level.
    Error,
}

// ─── Default value helpers ───────────────────────────────────────────

const fn default_profile() -> String {
    String::new()
}

const fn default_max_flows() -> usize {
    1024
}

const fn default_queue_depth() -> usize {
    256
}

const fn default_pool_size() -> usize {
    4096
}

const fn default_tun_name() -> String {
    String::new()
}

const fn default_mtu() -> u16 {
    1500
}

const fn default_io_threads() -> usize {
    2
}

const fn default_cpu_threads() -> usize {
    4
}

const fn default_max_background() -> usize {
    16
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            io_threads: default_io_threads(),
            cpu_threads: default_cpu_threads(),
            max_background_tasks: default_max_background(),
        }
    }
}

impl Default for DeviceConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            tun_name: default_tun_name(),
            mtu: default_mtu(),
            battery_aware: false,
        }
    }
}

impl RuntimeConfig {
    /// Load configuration from a TOML string.
    ///
    /// # Errors
    /// Returns an error if the TOML is malformed.
    pub fn from_toml(toml_str: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(toml_str)
    }

    /// Load configuration from a TOML file path.
    ///
    /// # Errors
    /// Returns an error if the file cannot be read or parsed.
    pub fn from_file(path: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let content = std::fs::read_to_string(path)?;
        Ok(Self::from_toml(&content)?)
    }

    /// Find a profile by name.
    #[must_use]
    pub fn find_profile(&self, name: &str) -> Option<&ProfileConfig> {
        self.profiles.iter().find(|p| p.name == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_minimal_config() {
        let toml_str = r#"
mode = "client"
profile = "stealth"
"#;
        let config = RuntimeConfig::from_toml(toml_str).unwrap();
        assert_eq!(config.mode, RuntimeMode::Client);
        assert_eq!(config.profile, "stealth");
    }

    #[test]
    fn parse_profile() {
        let toml_str = r#"
mode = "client"
[[profiles]]
name = "stealth"
description = "Maximum stealth profile"
stack = ["quic", "browser-http3", "chacha20"]
"#;
        let config = RuntimeConfig::from_toml(toml_str).unwrap();
        let profile = config.find_profile("stealth").unwrap();
        assert_eq!(profile.stack.len(), 3);
        assert_eq!(profile.stack[0], "quic");
    }
}
