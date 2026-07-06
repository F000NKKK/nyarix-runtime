//! Module health reporting (see issue #24).

/// Health status reported by a module.
///
/// The Runtime polls this periodically; `Degraded`/`Unhealthy` modules may
/// be swapped for a fallback node by the policy layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Health {
    /// The module is operating normally.
    Healthy,
    /// The module is operating but with reduced capability or performance.
    Degraded {
        /// Human-readable explanation.
        reason: String,
    },
    /// The module cannot perform its function.
    Unhealthy {
        /// Human-readable explanation.
        reason: String,
    },
}

impl Default for Health {
    fn default() -> Self {
        Self::Healthy
    }
}
