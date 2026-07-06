//! The `Module` trait — the contract every Nyarix module implements
//! (see issue #16).

use nyarix_error::ModuleError;
use nyarix_packet::Packet;

use crate::context::RuntimeContext;
use crate::health::Health;
use crate::metadata::ModuleMetadata;

/// Result type for module lifecycle and processing operations.
pub type Result<T> = std::result::Result<T, ModuleError>;

/// The contract every Nyarix module implements.
///
/// Lifecycle guarantee from the Runtime (#22): `initialize` is always
/// called before the first `process`, and `shutdown` is always called
/// after the last `process`.
pub trait Module: Send + Sync {
    /// Static metadata describing this module (name, version, type, ...).
    fn metadata(&self) -> &ModuleMetadata;

    /// Allocate resources, validate configuration, subscribe to events.
    /// Called exactly once before the first `process` call.
    ///
    /// # Errors
    /// Returns an error if the module cannot be brought up (e.g. invalid
    /// configuration, resource allocation failure).
    fn initialize(&mut self, ctx: &RuntimeContext) -> Result<()>;

    /// Process a single packet.
    ///
    /// Returns `Ok(Some(packet))` to pass the (possibly transformed) packet
    /// downstream, `Ok(None)` if this module intentionally absorbed or
    /// dropped it (see #19), or `Err` if processing failed.
    ///
    /// # Errors
    /// Returns [`ModuleError`] if the packet could not be processed.
    fn process(&mut self, packet: Packet) -> Result<Option<Packet>>;

    /// Graceful shutdown: release resources. Called exactly once, after the
    /// last `process` call.
    ///
    /// # Errors
    /// Returns an error if shutdown could not complete cleanly.
    fn shutdown(&mut self, ctx: &RuntimeContext) -> Result<()>;

    /// Report current health. Polled periodically by the Runtime (#24).
    fn health(&self) -> Health {
        Health::Healthy
    }

    /// Migrate to a new version/configuration without leaving the graph.
    /// Default is a no-op for modules that don't support live migration.
    ///
    /// # Errors
    /// Returns an error if migration was attempted but failed.
    fn migrate(&mut self, _ctx: &RuntimeContext) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::ModuleType;

    /// A pipeline passthrough module: forwards every packet unchanged.
    struct PassthroughModule {
        metadata: ModuleMetadata,
        initialized: bool,
    }

    impl PassthroughModule {
        fn new() -> Self {
            Self {
                metadata: ModuleMetadata::new("passthrough", "0.1.0", ModuleType::Observability),
                initialized: false,
            }
        }
    }

    impl Module for PassthroughModule {
        fn metadata(&self) -> &ModuleMetadata {
            &self.metadata
        }

        fn initialize(&mut self, _ctx: &RuntimeContext) -> Result<()> {
            self.initialized = true;
            Ok(())
        }

        fn process(&mut self, packet: Packet) -> Result<Option<Packet>> {
            Ok(Some(packet))
        }

        fn shutdown(&mut self, _ctx: &RuntimeContext) -> Result<()> {
            self.initialized = false;
            Ok(())
        }
    }

    /// A module that absorbs every packet (e.g. a terminal sink node).
    struct SinkModule {
        metadata: ModuleMetadata,
    }

    impl Module for SinkModule {
        fn metadata(&self) -> &ModuleMetadata {
            &self.metadata
        }

        fn initialize(&mut self, _ctx: &RuntimeContext) -> Result<()> {
            Ok(())
        }

        fn process(&mut self, _packet: Packet) -> Result<Option<Packet>> {
            Ok(None)
        }

        fn shutdown(&mut self, _ctx: &RuntimeContext) -> Result<()> {
            Ok(())
        }
    }

    #[test]
    fn lifecycle_runs_in_order() {
        let ctx = RuntimeContext::empty();
        let mut module = PassthroughModule::new();

        assert!(!module.initialized);
        module.initialize(&ctx).unwrap();
        assert!(module.initialized);

        let pkt = Packet::new(b"hello".as_slice());
        let result = module.process(pkt).unwrap();
        assert!(result.is_some());

        module.shutdown(&ctx).unwrap();
        assert!(!module.initialized);
    }

    #[test]
    fn passthrough_forwards_packet_unchanged() {
        let mut module = PassthroughModule::new();
        let pkt = Packet::new(b"payload".as_slice());
        let id = pkt.id();

        let forwarded = module.process(pkt).unwrap().unwrap();
        assert_eq!(forwarded.id(), id);
    }

    #[test]
    fn sink_absorbs_packet() {
        let mut module = SinkModule {
            metadata: ModuleMetadata::new("sink", "0.1.0", ModuleType::Flow),
        };
        let pkt = Packet::new(b"data".as_slice());

        assert!(module.process(pkt).unwrap().is_none());
    }

    #[test]
    fn default_health_is_healthy() {
        let module = PassthroughModule::new();
        assert_eq!(module.health(), Health::Healthy);
    }

    #[test]
    fn default_migrate_is_noop() {
        let mut module = PassthroughModule::new();
        let ctx = RuntimeContext::empty();
        assert!(module.migrate(&ctx).is_ok());
    }
}
