//! Structured error types for the Nyarix platform.
//!
//! Errors are organized by layer and carry context for diagnostics.
//! No bare strings — every error variant preserves enough information
//! for the UI and logs to render meaningful messages.

use thiserror::Error;

/// The top-level error type for Nyarix.
#[derive(Debug, Error)]
pub enum NyarixError {
    /// Configuration-related errors.
    #[error("configuration error: {0}")]
    Config(#[from] ConfigError),

    /// Package-related errors.
    #[error("package error: {0}")]
    Package(#[from] PackageError),

    /// Module-related errors.
    #[error("module error: {0}")]
    Module(#[from] ModuleError),

    /// Graph-related errors.
    #[error("graph error: {0}")]
    Graph(#[from] GraphError),

    /// Runtime execution errors.
    #[error("runtime error: {0}")]
    Runtime(#[from] RuntimeError),

    /// Transport errors (I/O, network).
    #[error("transport error: {0}")]
    Transport(#[from] TransportError),

    /// Cryptographic errors.
    #[error("crypto error: {0}")]
    Crypto(#[from] CryptoError),

    /// Security/sandbox errors.
    #[error("security error: {0}")]
    Security(#[from] SecurityError),

    /// Internal errors that should not happen.
    #[error("internal error: {0}")]
    Internal(#[from] InternalError),
}

/// Configuration errors.
#[derive(Debug, Error)]
pub enum ConfigError {
    /// Failed to parse a configuration file.
    #[error("failed to parse config at {path}: {source}")]
    Parse {
        path: String,
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    /// Missing required configuration key.
    #[error("missing required key: {key}")]
    MissingKey { key: String },
    /// Invalid configuration value.
    #[error("invalid value for {key}: {reason}")]
    InvalidValue { key: String, reason: String },
    /// Incompatible configuration between modules.
    #[error("incompatible config: {message}")]
    Incompatible { message: String },
}

/// Package-related errors.
#[derive(Debug, Error)]
pub enum PackageError {
    /// Failed to read a package file.
    #[error("failed to read package at {path}: {source}")]
    Io {
        path: String,
        source: std::io::Error,
    },
    /// Invalid package manifest.
    #[error("invalid manifest: {reason}")]
    InvalidManifest { reason: String },
    /// Package signature verification failed.
    #[error("signature verification failed for {package}")]
    SignatureFailure { package: String },
    /// Unsupported package format version.
    #[error("unsupported package format version: {version}")]
    UnsupportedVersion { version: String },
    /// Package not found in registry or cache.
    #[error("package not found: {name}")]
    NotFound { name: String },
    /// A required top-level member (see #58's `.nyp` layout) is missing.
    #[error("package is missing required member: {path}")]
    MissingMember { path: String },
}

/// Module-related errors.
#[derive(Debug, Error)]
pub enum ModuleError {
    /// Module initialization failed.
    #[error("module '{name}' failed to initialize: {reason}")]
    InitFailed { name: String, reason: String },
    /// Module panicked or crashed during processing.
    #[error("module '{name}' crashed: {reason}")]
    Crashed { name: String, reason: String },
    /// Module is not compatible with the current Runtime API.
    #[error("module '{name}' requires API {required}, but Runtime provides {actual}")]
    ApiMismatch {
        name: String,
        required: String,
        actual: String,
    },
    /// Module dependency is missing.
    #[error("module '{name}' depends on '{dependency}' which is not available")]
    MissingDependency { name: String, dependency: String },
    /// Circular dependency detected.
    #[error("circular dependency detected involving: {chain}")]
    CircularDependency { chain: String },
    /// Module exceeded its resource quota.
    #[error("module '{name}' exceeded {resource} limit")]
    QuotaExceeded { name: String, resource: String },
}

/// Graph-related errors.
#[derive(Debug, Error)]
pub enum GraphError {
    /// Cycle detected in the flow graph.
    #[error("cycle detected in graph: {cycle}")]
    Cycle { cycle: String },
    /// Missing node required by the graph.
    #[error("missing node: {node_id}")]
    MissingNode { node_id: String },
    /// Incompatible edge between nodes.
    #[error("incompatible edge: {from} -> {to}: {reason}")]
    IncompatibleEdge {
        from: String,
        to: String,
        reason: String,
    },
    /// Graph is disconnected.
    #[error("graph is disconnected")]
    Disconnected,
    /// Failed to build graph from configuration.
    #[error("graph build failed: {reason}")]
    BuildFailed { reason: String },
}

/// Runtime execution errors.
#[derive(Debug, Error)]
pub enum RuntimeError {
    /// Runtime is already running.
    #[error("runtime is already running")]
    AlreadyRunning,
    /// Runtime is shutting down.
    #[error("runtime is shutting down")]
    ShuttingDown,
    /// Scheduler error.
    #[error("scheduler error: {reason}")]
    Scheduler { reason: String },
    /// Event bus error.
    #[error("event bus error: {reason}")]
    EventBus { reason: String },
    /// Resource exhaustion.
    #[error("resource exhausted: {resource}")]
    ResourceExhausted { resource: String },
}

/// Transport errors (network I/O).
#[derive(Debug, Error)]
pub enum TransportError {
    /// Connection failed.
    #[error("connection to {addr} failed: {source}")]
    ConnectionFailed {
        addr: String,
        source: std::io::Error,
    },
    /// Connection was reset.
    #[error("connection reset: {reason}")]
    ConnectionReset { reason: String },
    /// Timeout.
    #[error("transport timeout after {duration_ms}ms")]
    Timeout { duration_ms: u64 },
    /// Address resolution failed.
    #[error("failed to resolve address: {addr}")]
    ResolutionFailed { addr: String },
    /// Transport is not supported on this platform.
    #[error("transport not supported: {transport}")]
    NotSupported { transport: String },
}

/// Cryptographic errors.
#[derive(Debug, Error)]
pub enum CryptoError {
    /// Key exchange failed.
    #[error("key exchange failed: {reason}")]
    KeyExchange { reason: String },
    /// Encryption/decryption failed.
    #[error("crypto operation failed: {reason}")]
    OperationFailed { reason: String },
    /// Invalid key material.
    #[error("invalid key: {reason}")]
    InvalidKey { reason: String },
    /// Algorithm not supported.
    #[error("crypto algorithm not supported: {algorithm}")]
    UnsupportedAlgorithm { algorithm: String },
}

/// Security and sandbox errors.
#[derive(Debug, Error)]
pub enum SecurityError {
    /// Module requested a capability it was not granted.
    #[error("module '{module}' denied capability '{capability}'")]
    CapabilityDenied { module: String, capability: String },
    /// Signing verification failed.
    #[error("signature verification failed: {reason}")]
    SignatureFailed { reason: String },
    /// Trust level insufficient.
    #[error("module '{module}' has trust level {level}, requires {required}")]
    InsufficientTrust {
        module: String,
        level: String,
        required: String,
    },
    /// Sandbox violation.
    #[error("sandbox violation by '{module}': {violation}")]
    SandboxViolation { module: String, violation: String },
}

/// Internal errors — these should not happen in normal operation.
#[derive(Debug, Error)]
pub enum InternalError {
    /// An invariant was violated.
    #[error("invariant violated: {message}")]
    InvariantViolation { message: String },
    /// Unexpected state.
    #[error("unexpected state: {message}")]
    UnexpectedState { message: String },
    /// A bug was detected.
    #[error("bug: {message}")]
    Bug { message: String },
}
