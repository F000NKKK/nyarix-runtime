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
        /// The path of the config file that failed to parse.
        path: String,
        /// The underlying parse error.
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    /// Missing required configuration key.
    #[error("missing required key: {key}")]
    MissingKey {
        /// The missing key's name.
        key: String,
    },
    /// Invalid configuration value.
    #[error("invalid value for {key}: {reason}")]
    InvalidValue {
        /// The offending key's name.
        key: String,
        /// Why the value is invalid.
        reason: String,
    },
    /// Incompatible configuration between modules.
    #[error("incompatible config: {message}")]
    Incompatible {
        /// Description of the incompatibility.
        message: String,
    },
}

/// Package-related errors.
#[derive(Debug, Error)]
pub enum PackageError {
    /// Failed to read a package file.
    #[error("failed to read package at {path}: {source}")]
    Io {
        /// The path that failed to read.
        path: String,
        /// The underlying I/O error.
        source: std::io::Error,
    },
    /// Invalid package manifest.
    #[error("invalid manifest: {reason}")]
    InvalidManifest {
        /// Why the manifest is invalid.
        reason: String,
    },
    /// Package signature verification failed.
    #[error("signature verification failed for {package}")]
    SignatureFailure {
        /// The package whose signature failed to verify.
        package: String,
    },
    /// Unsupported package format version.
    #[error("unsupported package format version: {version}")]
    UnsupportedVersion {
        /// The unsupported version string.
        version: String,
    },
    /// Package not found in registry or cache.
    #[error("package not found: {name}")]
    NotFound {
        /// The package name that couldn't be found.
        name: String,
    },
    /// A required top-level member (see #58's `.nyp` layout) is missing.
    #[error("package is missing required member: {path}")]
    MissingMember {
        /// The path of the missing required member.
        path: String,
    },
}

/// Module-related errors.
#[derive(Debug, Error)]
pub enum ModuleError {
    /// Module initialization failed.
    #[error("module '{name}' failed to initialize: {reason}")]
    InitFailed {
        /// The module's name.
        name: String,
        /// Why initialization failed.
        reason: String,
    },
    /// Module panicked or crashed during processing.
    #[error("module '{name}' crashed: {reason}")]
    Crashed {
        /// The module's name.
        name: String,
        /// Why the module crashed.
        reason: String,
    },
    /// Module is not compatible with the current Runtime API.
    #[error("module '{name}' requires API {required}, but Runtime provides {actual}")]
    ApiMismatch {
        /// The module's name.
        name: String,
        /// The API version the module requires.
        required: String,
        /// The API version the Runtime actually provides.
        actual: String,
    },
    /// Module dependency is missing.
    #[error("module '{name}' depends on '{dependency}' which is not available")]
    MissingDependency {
        /// The module that declared the dependency.
        name: String,
        /// The missing dependency's name.
        dependency: String,
    },
    /// Circular dependency detected.
    #[error("circular dependency detected involving: {chain}")]
    CircularDependency {
        /// The dependency chain forming the cycle.
        chain: String,
    },
    /// Module exceeded its resource quota.
    #[error("module '{name}' exceeded {resource} limit")]
    QuotaExceeded {
        /// The module's name.
        name: String,
        /// The resource whose limit was exceeded.
        resource: String,
    },
}

/// Graph-related errors.
#[derive(Debug, Error)]
pub enum GraphError {
    /// Cycle detected in the flow graph.
    #[error("cycle detected in graph: {cycle}")]
    Cycle {
        /// A description of the cycle.
        cycle: String,
    },
    /// Missing node required by the graph.
    #[error("missing node: {node_id}")]
    MissingNode {
        /// The missing node's identifier.
        node_id: String,
    },
    /// Incompatible edge between nodes.
    #[error("incompatible edge: {from} -> {to}: {reason}")]
    IncompatibleEdge {
        /// The edge's source node.
        from: String,
        /// The edge's destination node.
        to: String,
        /// Why the edge is incompatible.
        reason: String,
    },
    /// Graph is disconnected.
    #[error("graph is disconnected")]
    Disconnected,
    /// Failed to build graph from configuration.
    #[error("graph build failed: {reason}")]
    BuildFailed {
        /// Why building the graph failed.
        reason: String,
    },
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
    Scheduler {
        /// Why the scheduler failed.
        reason: String,
    },
    /// Event bus error.
    #[error("event bus error: {reason}")]
    EventBus {
        /// Why the event bus failed.
        reason: String,
    },
    /// Resource exhaustion.
    #[error("resource exhausted: {resource}")]
    ResourceExhausted {
        /// The exhausted resource's name.
        resource: String,
    },
}

/// Transport errors (network I/O).
#[derive(Debug, Error)]
pub enum TransportError {
    /// Connection failed.
    #[error("connection to {addr} failed: {source}")]
    ConnectionFailed {
        /// The address that couldn't be connected to.
        addr: String,
        /// The underlying I/O error.
        source: std::io::Error,
    },
    /// Connection was reset.
    #[error("connection reset: {reason}")]
    ConnectionReset {
        /// Why the connection was reset.
        reason: String,
    },
    /// Timeout.
    #[error("transport timeout after {duration_ms}ms")]
    Timeout {
        /// How long the Runtime waited before timing out, in milliseconds.
        duration_ms: u64,
    },
    /// Address resolution failed.
    #[error("failed to resolve address: {addr}")]
    ResolutionFailed {
        /// The address that failed to resolve.
        addr: String,
    },
    /// Transport is not supported on this platform.
    #[error("transport not supported: {transport}")]
    NotSupported {
        /// The unsupported transport's name.
        transport: String,
    },
}

/// Cryptographic errors.
#[derive(Debug, Error)]
pub enum CryptoError {
    /// Key exchange failed.
    #[error("key exchange failed: {reason}")]
    KeyExchange {
        /// Why the key exchange failed.
        reason: String,
    },
    /// Encryption/decryption failed.
    #[error("crypto operation failed: {reason}")]
    OperationFailed {
        /// Why the operation failed.
        reason: String,
    },
    /// Invalid key material.
    #[error("invalid key: {reason}")]
    InvalidKey {
        /// Why the key is invalid.
        reason: String,
    },
    /// Algorithm not supported.
    #[error("crypto algorithm not supported: {algorithm}")]
    UnsupportedAlgorithm {
        /// The unsupported algorithm's name.
        algorithm: String,
    },
}

/// Security and sandbox errors.
#[derive(Debug, Error)]
pub enum SecurityError {
    /// Module requested a capability it was not granted.
    #[error("module '{module}' denied capability '{capability}'")]
    CapabilityDenied {
        /// The module that was denied.
        module: String,
        /// The capability it was denied.
        capability: String,
    },
    /// Signing verification failed.
    #[error("signature verification failed: {reason}")]
    SignatureFailed {
        /// Why signature verification failed.
        reason: String,
    },
    /// Trust level insufficient.
    #[error("module '{module}' has trust level {level}, requires {required}")]
    InsufficientTrust {
        /// The module with insufficient trust.
        module: String,
        /// The module's actual trust level.
        level: String,
        /// The trust level required.
        required: String,
    },
    /// Sandbox violation.
    #[error("sandbox violation by '{module}': {violation}")]
    SandboxViolation {
        /// The module that violated its sandbox.
        module: String,
        /// A description of the violation.
        violation: String,
    },
}

/// Internal errors — these should not happen in normal operation.
#[derive(Debug, Error)]
pub enum InternalError {
    /// An invariant was violated.
    #[error("invariant violated: {message}")]
    InvariantViolation {
        /// A description of the violated invariant.
        message: String,
    },
    /// Unexpected state.
    #[error("unexpected state: {message}")]
    UnexpectedState {
        /// A description of the unexpected state.
        message: String,
    },
    /// A bug was detected.
    #[error("bug: {message}")]
    Bug {
        /// A description of the bug.
        message: String,
    },
}
