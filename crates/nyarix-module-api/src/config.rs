//! Per-module configuration handed to a module via `RuntimeContext` (see
//! issue #18).

/// Opaque per-module configuration blob.
///
/// A thin wrapper around a `toml::Value` — mirrors how
/// `nyarix_config::RuntimeConfig::modules` already stores per-module
/// overrides as free-form TOML (see #6/#90). Individual modules are
/// responsible for defining and parsing their own typed schema out of
/// this; `nyarix-module-api` doesn't know what any given module's config
/// should look like.
#[derive(Debug, Clone, PartialEq)]
pub struct ModuleConfig(toml::Value);

impl ModuleConfig {
    /// An empty configuration (an empty TOML table).
    #[must_use]
    pub fn empty() -> Self {
        Self(toml::Value::Table(toml::map::Map::new()))
    }

    /// Wrap an existing TOML value as a module's configuration.
    #[must_use]
    pub fn from_value(value: toml::Value) -> Self {
        Self(value)
    }

    /// Borrow the raw TOML value.
    #[must_use]
    pub fn raw(&self) -> &toml::Value {
        &self.0
    }

    /// Deserialize a specific key into a typed value.
    ///
    /// Returns `None` if the key is missing or doesn't deserialize as `T`.
    pub fn get<T: serde::de::DeserializeOwned>(&self, key: &str) -> Option<T> {
        self.0.get(key)?.clone().try_into().ok()
    }
}

impl Default for ModuleConfig {
    fn default() -> Self {
        Self::empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_config_has_no_keys() {
        let config = ModuleConfig::empty();
        assert_eq!(config.get::<String>("missing"), None);
    }

    #[test]
    fn reads_typed_value_from_table() {
        let mut table = toml::map::Map::new();
        table.insert("mtu".to_string(), toml::Value::Integer(1500));
        let config = ModuleConfig::from_value(toml::Value::Table(table));

        assert_eq!(config.get::<i64>("mtu"), Some(1500));
        assert_eq!(config.get::<i64>("missing"), None);
    }
}
