//! Runtime initialization (see issue #40).

use nyarix_config::RuntimeConfig;
use thiserror::Error;

/// Error produced while initializing the Runtime.
#[derive(Debug, Error)]
pub enum RuntimeInitError {
    /// The configuration failed to parse.
    #[error("failed to parse runtime configuration: {0}")]
    Config(#[from] toml::de::Error),
}

/// Handle to an initialized Nyarix Runtime.
///
/// Holds the parsed configuration now. The other subsystems the issue
/// asks for — EventBus (#48), Scheduler (see the Scheduler issues:
/// I/O thread pool, CPU worker pool, priorities), Metrics registry (#80),
/// Module Loader (#41) — aren't built yet; each is its own issue, so
/// there's nothing here to wire up until they exist. `RuntimeHandle`
/// currently just holds the configuration slot so its shape doesn't need
/// to change again once they land.
#[derive(Debug)]
pub struct RuntimeHandle {
    config: RuntimeConfig,
}

impl RuntimeHandle {
    /// Initialize the Runtime from a TOML configuration string.
    ///
    /// This crate does **not** create its own `tokio::Runtime` — unlike
    /// what the issue's "Создание tokio runtime" line might suggest,
    /// having a library spin up its own executor is generally the wrong
    /// design (the embedding application should own that); call this from
    /// within an existing tokio context instead (`#[tokio::main]`, as
    /// `apps/runtime-test` already does).
    ///
    /// # Errors
    /// Returns [`RuntimeInitError`] if `toml_str` doesn't parse as a valid
    /// [`RuntimeConfig`].
    pub fn from_toml(toml_str: &str) -> Result<Self, RuntimeInitError> {
        let config = RuntimeConfig::from_toml(toml_str)?;
        Ok(Self { config })
    }

    /// This Runtime's configuration.
    #[must_use]
    pub fn config(&self) -> &RuntimeConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initializes_from_minimal_toml() {
        let handle = RuntimeHandle::from_toml(r#"mode = "client""#).unwrap();
        assert_eq!(handle.config().mode, nyarix_config::RuntimeMode::Client);
    }

    #[test]
    fn rejects_invalid_toml() {
        assert!(RuntimeHandle::from_toml("not = [valid toml").is_err());
    }
}
