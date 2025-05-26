//! Cluster Error Handling
//!
//! This module provides comprehensive error handling for cluster operations,
//! including network errors, protocol errors, and configuration issues.

use std::net::SocketAddr;

/// Result type for cluster operations
pub type ClusterResult<T> = Result<T, ClusterError>;

/// Cluster-specific errors
#[derive(Debug, thiserror::Error)]
pub enum ClusterError {
    #[error("Network error: {message}")]
    NetworkError { message: String },

    #[error("Failed to bind to address {addr}: {source}")]
    BindError {
        addr: SocketAddr,
        #[source]
        source: std::io::Error,
    },

    #[error("Connection failed to {addr}: {source}")]
    ConnectionError {
        addr: SocketAddr,
        #[source]
        source: std::io::Error,
    },

    #[error("Protocol error: {message}")]
    ProtocolError { message: String },

    #[error("Gossip error: {message}")]
    GossipError { message: String },

    #[error("Node not found: {node_id}")]
    NodeNotFound { node_id: String },

    #[error("Service not found: {service_name}")]
    ServiceNotFound { service_name: String },

    #[error("Join failed: {message}")]
    JoinFailed { message: String },

    #[error("Health check failed for node {node_id}: {reason}")]
    HealthCheckFailed { node_id: String, reason: String },

    #[error("Replication error: {message}")]
    ReplicationError { message: String },

    #[error("Serialization error: {0}")]
    SerializationError(#[from] serde_json::Error),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("Timeout error: {operation}")]
    TimeoutError { operation: String },

    #[error("Configuration error: {0}")]
    ConfigError(#[from] crate::cluster::config::ConfigError),

    #[error("Encryption error: {message}")]
    EncryptionError { message: String },

    #[error("Authentication failed: {message}")]
    AuthenticationError { message: String },

    #[error("Cluster not ready: {message}")]
    NotReady { message: String },

    #[error("Operation not supported: {operation}")]
    NotSupported { operation: String },

    #[error("Resource exhausted: {resource}")]
    ResourceExhausted { resource: String },

    #[error("Internal error: {message}")]
    InternalError { message: String },
}

impl ClusterError {
    /// Create a network error
    pub fn network<S: Into<String>>(message: S) -> Self {
        Self::NetworkError {
            message: message.into(),
        }
    }

    /// Create a protocol error
    pub fn protocol<S: Into<String>>(message: S) -> Self {
        Self::ProtocolError {
            message: message.into(),
        }
    }

    /// Create a gossip error
    pub fn gossip<S: Into<String>>(message: S) -> Self {
        Self::GossipError {
            message: message.into(),
        }
    }

    /// Create a node not found error
    pub fn node_not_found<S: Into<String>>(node_id: S) -> Self {
        Self::NodeNotFound {
            node_id: node_id.into(),
        }
    }

    /// Create a service not found error
    pub fn service_not_found<S: Into<String>>(service_name: S) -> Self {
        Self::ServiceNotFound {
            service_name: service_name.into(),
        }
    }

    /// Create a join failed error
    pub fn join_failed<S: Into<String>>(message: S) -> Self {
        Self::JoinFailed {
            message: message.into(),
        }
    }

    /// Create a health check failed error
    pub fn health_check_failed<S: Into<String>>(node_id: S, reason: S) -> Self {
        Self::HealthCheckFailed {
            node_id: node_id.into(),
            reason: reason.into(),
        }
    }

    /// Create a replication error
    pub fn replication<S: Into<String>>(message: S) -> Self {
        Self::ReplicationError {
            message: message.into(),
        }
    }

    /// Create a timeout error
    pub fn timeout<S: Into<String>>(operation: S) -> Self {
        Self::TimeoutError {
            operation: operation.into(),
        }
    }

    /// Create an encryption error
    pub fn encryption<S: Into<String>>(message: S) -> Self {
        Self::EncryptionError {
            message: message.into(),
        }
    }

    /// Create an authentication error
    pub fn authentication<S: Into<String>>(message: S) -> Self {
        Self::AuthenticationError {
            message: message.into(),
        }
    }

    /// Create a not ready error
    pub fn not_ready<S: Into<String>>(message: S) -> Self {
        Self::NotReady {
            message: message.into(),
        }
    }

    /// Create a not supported error
    pub fn not_supported<S: Into<String>>(operation: S) -> Self {
        Self::NotSupported {
            operation: operation.into(),
        }
    }

    /// Create a resource exhausted error
    pub fn resource_exhausted<S: Into<String>>(resource: S) -> Self {
        Self::ResourceExhausted {
            resource: resource.into(),
        }
    }

    /// Create an internal error
    pub fn internal<S: Into<String>>(message: S) -> Self {
        Self::InternalError {
            message: message.into(),
        }
    }

    /// Check if the error is retryable
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            ClusterError::NetworkError { .. }
                | ClusterError::ConnectionError { .. }
                | ClusterError::TimeoutError { .. }
                | ClusterError::ResourceExhausted { .. }
                | ClusterError::NotReady { .. }
        )
    }

    /// Check if the error is permanent
    pub fn is_permanent(&self) -> bool {
        matches!(
            self,
            ClusterError::ConfigError(_)
                | ClusterError::NotSupported { .. }
                | ClusterError::AuthenticationError { .. }
                | ClusterError::SerializationError(_)
        )
    }

    /// Get error category
    pub fn category(&self) -> ErrorCategory {
        match self {
            ClusterError::NetworkError { .. } => ErrorCategory::Network,
            ClusterError::BindError { .. } => ErrorCategory::Network,
            ClusterError::ConnectionError { .. } => ErrorCategory::Network,
            ClusterError::ProtocolError { .. } => ErrorCategory::Protocol,
            ClusterError::GossipError { .. } => ErrorCategory::Protocol,
            ClusterError::NodeNotFound { .. } => ErrorCategory::NotFound,
            ClusterError::ServiceNotFound { .. } => ErrorCategory::NotFound,
            ClusterError::JoinFailed { .. } => ErrorCategory::Operation,
            ClusterError::HealthCheckFailed { .. } => ErrorCategory::Health,
            ClusterError::ReplicationError { .. } => ErrorCategory::Replication,
            ClusterError::SerializationError(_) => ErrorCategory::Serialization,
            ClusterError::IoError(_) => ErrorCategory::IO,
            ClusterError::TimeoutError { .. } => ErrorCategory::Timeout,
            ClusterError::ConfigError(_) => ErrorCategory::Configuration,
            ClusterError::EncryptionError { .. } => ErrorCategory::Security,
            ClusterError::AuthenticationError { .. } => ErrorCategory::Security,
            ClusterError::NotReady { .. } => ErrorCategory::State,
            ClusterError::NotSupported { .. } => ErrorCategory::Operation,
            ClusterError::ResourceExhausted { .. } => ErrorCategory::Resource,
            ClusterError::InternalError { .. } => ErrorCategory::Internal,
        }
    }
}

/// Error categories for classification
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCategory {
    Network,
    Protocol,
    NotFound,
    Operation,
    Health,
    Replication,
    Serialization,
    IO,
    Timeout,
    Configuration,
    Security,
    State,
    Resource,
    Internal,
}

impl std::fmt::Display for ErrorCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ErrorCategory::Network => write!(f, "network"),
            ErrorCategory::Protocol => write!(f, "protocol"),
            ErrorCategory::NotFound => write!(f, "not_found"),
            ErrorCategory::Operation => write!(f, "operation"),
            ErrorCategory::Health => write!(f, "health"),
            ErrorCategory::Replication => write!(f, "replication"),
            ErrorCategory::Serialization => write!(f, "serialization"),
            ErrorCategory::IO => write!(f, "io"),
            ErrorCategory::Timeout => write!(f, "timeout"),
            ErrorCategory::Configuration => write!(f, "configuration"),
            ErrorCategory::Security => write!(f, "security"),
            ErrorCategory::State => write!(f, "state"),
            ErrorCategory::Resource => write!(f, "resource"),
            ErrorCategory::Internal => write!(f, "internal"),
        }
    }
}

/// Error context for additional debugging information
#[derive(Debug, Clone)]
pub struct ErrorContext {
    /// Operation that was being performed
    pub operation: String,
    /// Node ID if applicable
    pub node_id: Option<String>,
    /// Service name if applicable
    pub service_name: Option<String>,
    /// Additional metadata
    pub metadata: std::collections::HashMap<String, String>,
}

impl ErrorContext {
    /// Create a new error context
    pub fn new<S: Into<String>>(operation: S) -> Self {
        Self {
            operation: operation.into(),
            node_id: None,
            service_name: None,
            metadata: std::collections::HashMap::new(),
        }
    }

    /// Add node ID to context
    pub fn with_node_id<S: Into<String>>(mut self, node_id: S) -> Self {
        self.node_id = Some(node_id.into());
        self
    }

    /// Add service name to context
    pub fn with_service_name<S: Into<String>>(mut self, service_name: S) -> Self {
        self.service_name = Some(service_name.into());
        self
    }

    /// Add metadata to context
    pub fn with_metadata<K, V>(mut self, key: K, value: V) -> Self
    where
        K: Into<String>,
        V: Into<String>,
    {
        self.metadata.insert(key.into(), value.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_creation() {
        let error = ClusterError::network("Connection refused");
        assert!(matches!(error, ClusterError::NetworkError { .. }));
        assert_eq!(error.category(), ErrorCategory::Network);
        assert!(error.is_retryable());
        assert!(!error.is_permanent());
    }

    #[test]
    fn test_error_categories() {
        let network_error = ClusterError::network("test");
        assert_eq!(network_error.category(), ErrorCategory::Network);

        let config_error =
            ClusterError::ConfigError(crate::cluster::config::ConfigError::InvalidValue {
                field: "test".to_string(),
                message: "test".to_string(),
            });
        assert_eq!(config_error.category(), ErrorCategory::Configuration);
        assert!(config_error.is_permanent());
    }

    #[test]
    fn test_error_context() {
        let context = ErrorContext::new("join_cluster")
            .with_node_id("node-123")
            .with_service_name("disruptor-service")
            .with_metadata("attempt", "1");

        assert_eq!(context.operation, "join_cluster");
        assert_eq!(context.node_id, Some("node-123".to_string()));
        assert_eq!(context.service_name, Some("disruptor-service".to_string()));
        assert_eq!(context.metadata.get("attempt"), Some(&"1".to_string()));
    }

    #[test]
    fn test_retryable_errors() {
        assert!(ClusterError::network("test").is_retryable());
        assert!(ClusterError::timeout("test").is_retryable());
        assert!(!ClusterError::not_supported("test").is_retryable());
        assert!(!ClusterError::authentication("test").is_retryable());
    }

    #[test]
    fn test_all_error_constructors() {
        // Test all error constructor methods
        let _ = ClusterError::network("network error");
        let _ = ClusterError::protocol("protocol error");
        let _ = ClusterError::gossip("gossip error");
        let _ = ClusterError::node_not_found("node-123");
        let _ = ClusterError::service_not_found("my-service");
        let _ = ClusterError::join_failed("join failed");
        let _ = ClusterError::health_check_failed("node-456", "timeout");
        let _ = ClusterError::replication("replication error");
        let _ = ClusterError::timeout("operation timeout");
        let _ = ClusterError::encryption("encryption error");
        let _ = ClusterError::authentication("auth failed");
        let _ = ClusterError::not_ready("not ready");
        let _ = ClusterError::not_supported("unsupported operation");
        let _ = ClusterError::resource_exhausted("memory");
        let _ = ClusterError::internal("internal error");
    }

    #[test]
    fn test_error_category_display() {
        assert_eq!(ErrorCategory::Network.to_string(), "network");
        assert_eq!(ErrorCategory::Protocol.to_string(), "protocol");
        assert_eq!(ErrorCategory::NotFound.to_string(), "not_found");
        assert_eq!(ErrorCategory::Operation.to_string(), "operation");
        assert_eq!(ErrorCategory::Health.to_string(), "health");
        assert_eq!(ErrorCategory::Replication.to_string(), "replication");
        assert_eq!(ErrorCategory::Serialization.to_string(), "serialization");
        assert_eq!(ErrorCategory::IO.to_string(), "io");
        assert_eq!(ErrorCategory::Timeout.to_string(), "timeout");
        assert_eq!(ErrorCategory::Configuration.to_string(), "configuration");
        assert_eq!(ErrorCategory::Security.to_string(), "security");
        assert_eq!(ErrorCategory::State.to_string(), "state");
        assert_eq!(ErrorCategory::Resource.to_string(), "resource");
        assert_eq!(ErrorCategory::Internal.to_string(), "internal");
    }

    #[test]
    fn test_permanent_errors() {
        assert!(ClusterError::not_supported("test").is_permanent());
        assert!(ClusterError::authentication("test").is_permanent());
        assert!(!ClusterError::network("test").is_permanent());
        assert!(!ClusterError::timeout("test").is_permanent());
    }

    #[test]
    fn test_error_context_builder() {
        let context = ErrorContext::new("test_operation");
        assert_eq!(context.operation, "test_operation");
        assert!(context.node_id.is_none());
        assert!(context.service_name.is_none());
        assert!(context.metadata.is_empty());

        let context = context
            .with_node_id("test-node")
            .with_service_name("test-service")
            .with_metadata("key1", "value1")
            .with_metadata("key2", "value2");

        assert_eq!(context.node_id, Some("test-node".to_string()));
        assert_eq!(context.service_name, Some("test-service".to_string()));
        assert_eq!(context.metadata.len(), 2);
        assert_eq!(context.metadata.get("key1"), Some(&"value1".to_string()));
        assert_eq!(context.metadata.get("key2"), Some(&"value2".to_string()));
    }

    #[test]
    fn test_error_from_conversions() {
        // Test From implementations
        let io_error = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let cluster_error: ClusterError = io_error.into();
        assert!(matches!(cluster_error, ClusterError::IoError(_)));
        assert_eq!(cluster_error.category(), ErrorCategory::IO);

        // Create a serde_json error by trying to parse invalid JSON
        let invalid_json = "{invalid json";
        let serde_error = serde_json::from_str::<serde_json::Value>(invalid_json).unwrap_err();
        let cluster_error: ClusterError = serde_error.into();
        assert!(matches!(cluster_error, ClusterError::SerializationError(_)));
        assert_eq!(cluster_error.category(), ErrorCategory::Serialization);
    }

    #[test]
    fn test_specific_error_variants() {
        // Test specific error variants with their expected categories
        let addr: SocketAddr = "127.0.0.1:8080".parse().unwrap();
        let io_error = std::io::Error::new(std::io::ErrorKind::AddrInUse, "Address already in use");
        let bind_error = ClusterError::BindError {
            addr,
            source: io_error,
        };
        assert_eq!(bind_error.category(), ErrorCategory::Network);
        assert!(!bind_error.is_retryable());

        let addr2: SocketAddr = "192.168.1.1:9090".parse().unwrap();
        let io_error2 =
            std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "Connection refused");
        let connection_error = ClusterError::ConnectionError {
            addr: addr2,
            source: io_error2,
        };
        assert_eq!(connection_error.category(), ErrorCategory::Network);
        assert!(connection_error.is_retryable());

        let health_check_error = ClusterError::HealthCheckFailed {
            node_id: "node-456".to_string(),
            reason: "Timeout after 5s".to_string(),
        };
        assert_eq!(health_check_error.category(), ErrorCategory::Health);
        assert!(!health_check_error.is_retryable());
    }
}
