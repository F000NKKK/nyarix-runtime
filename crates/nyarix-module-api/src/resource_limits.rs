//! Resource limits declared by a module (see issue #20).
//!
//! This is the **declaration** only — a module says what it needs/expects
//! to be capped at. Actual enforcement is M7 (Capability & Sandbox: #76
//! Memory limits, #77 CPU limits per module, #78 I/O restrictions).

use std::time::Duration;

/// Resource limits a module declares for itself.
///
/// `None` in any field means "no explicit limit declared" — the Runtime
/// falls back to its own defaults once enforcement (M7) exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ResourceLimits {
    /// Maximum resident memory, in bytes.
    pub max_memory_bytes: Option<u64>,
    /// Maximum size, in bytes, of a single packet's payload this module
    /// will accept in `process()` (#76's "Лимит на размер payload в
    /// обработке") — distinct from [`Self::max_memory_bytes`] (that
    /// module's total footprint), this bounds one call's input.
    pub max_payload_bytes: Option<u64>,
    /// Maximum CPU usage, as a percentage of one core (values above 100
    /// express a multi-core budget).
    pub max_cpu_percent: Option<u8>,
    /// Maximum wall-clock time budget for a single `process()` call.
    pub max_processing_time: Option<Duration>,
}

impl ResourceLimits {
    /// No declared limits.
    #[must_use]
    pub const fn unbounded() -> Self {
        Self {
            max_memory_bytes: None,
            max_payload_bytes: None,
            max_cpu_percent: None,
            max_processing_time: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unbounded_declares_no_limits() {
        let limits = ResourceLimits::unbounded();
        assert_eq!(limits.max_memory_bytes, None);
        assert_eq!(limits.max_payload_bytes, None);
        assert_eq!(limits.max_cpu_percent, None);
        assert_eq!(limits.max_processing_time, None);
    }

    #[test]
    fn max_payload_bytes_can_be_declared_independently_of_max_memory_bytes() {
        let limits = ResourceLimits {
            max_payload_bytes: Some(4096),
            ..ResourceLimits::unbounded()
        };
        assert_eq!(limits.max_payload_bytes, Some(4096));
        assert_eq!(limits.max_memory_bytes, None);
    }
}
