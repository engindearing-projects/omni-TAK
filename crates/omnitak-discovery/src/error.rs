//! Error types for the discovery service

use thiserror::Error;

/// Result type alias for discovery operations
pub type Result<T> = std::result::Result<T, DiscoveryError>;

/// Errors that can occur during service discovery
#[derive(Debug, Error)]
pub enum DiscoveryError {
    /// mDNS service daemon failed to initialize
    #[error("Failed to initialize mDNS daemon: {0}")]
    MdnsInitFailed(String),

    /// Failed to browse for services
    #[error("Failed to browse for service type '{service_type}': {reason}")]
    BrowseFailed {
        service_type: String,
        reason: String,
    },

    /// Failed to register/announce a service
    #[error("Failed to register service '{service_name}': {reason}")]
    RegisterFailed {
        service_name: String,
        reason: String,
    },

    /// Service resolution failed
    #[error("Failed to resolve service '{service_name}': {reason}")]
    ResolutionFailed {
        service_name: String,
        reason: String,
    },

    /// Invalid service configuration
    #[error("Invalid service configuration: {0}")]
    InvalidConfig(String),

    /// Service not found
    #[error("Service not found: {0}")]
    ServiceNotFound(String),

    /// Discovery service already started
    #[error("Discovery service is already running")]
    AlreadyStarted,

    /// Discovery service not started
    #[error("Discovery service has not been started")]
    NotStarted,

    /// Internal error
    #[error("Internal discovery error: {0}")]
    Internal(String),

    /// IO error
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// Generic error
    #[error("{0}")]
    Other(#[from] anyhow::Error),
}
