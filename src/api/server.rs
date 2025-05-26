//! API Server
//!
//! This module provides the main HTTP server implementation for the BadBatch Disruptor API.
//! It handles server lifecycle, configuration, and provides a clean interface for starting
//! and stopping the API service.

use axum::Router;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;
use tracing::{error, info};

use crate::api::{manager::DisruptorManager, routes, ServerConfig};

/// Main API server
pub struct ApiServer {
    /// Server configuration
    config: ServerConfig,
    /// Server router
    router: Router,
}

impl ApiServer {
    /// Create a new API server with the given configuration
    pub fn new(config: ServerConfig) -> Self {
        let router = routes::create_router(&config);

        Self { config, router }
    }

    /// Create a new API server with state management
    pub fn with_state(config: ServerConfig, manager: Arc<Mutex<DisruptorManager>>) -> Self {
        let router = routes::create_router_with_state(&config, manager);

        Self { config, router }
    }

    /// Create a new API server with default configuration
    pub fn with_defaults() -> Self {
        Self::new(ServerConfig::default())
    }

    /// Start the API server
    pub async fn start(self) -> Result<(), ApiServerError> {
        // Parse the configured host and port into a SocketAddr
        let addr = format!("{}:{}", self.config.host, self.config.port)
            .parse::<SocketAddr>()
            .map_err(|e| ApiServerError::ConfigError {
                message: format!(
                    "Invalid host:port combination '{}:{}': {}",
                    self.config.host, self.config.port, e
                ),
            })?;

        info!(
            host = %self.config.host,
            port = %self.config.port,
            "Starting BadBatch Disruptor API server"
        );

        // Create TCP listener
        let listener = TcpListener::bind(addr)
            .await
            .map_err(|e| ApiServerError::BindError {
                addr: addr.to_string(),
                source: e,
            })?;

        info!(addr = %addr, "API server listening");

        // Start the server
        axum::serve(listener, self.router)
            .await
            .map_err(|e| ApiServerError::ServerError {
                message: e.to_string(),
            })?;

        Ok(())
    }

    /// Start the server with graceful shutdown
    pub async fn start_with_shutdown(
        self,
        shutdown_signal: impl std::future::Future<Output = ()> + Send + 'static,
    ) -> Result<(), ApiServerError> {
        // Parse the configured host and port into a SocketAddr
        let addr = format!("{}:{}", self.config.host, self.config.port)
            .parse::<SocketAddr>()
            .map_err(|e| ApiServerError::ConfigError {
                message: format!(
                    "Invalid host:port combination '{}:{}': {}",
                    self.config.host, self.config.port, e
                ),
            })?;

        info!(
            host = %self.config.host,
            port = %self.config.port,
            "Starting BadBatch Disruptor API server with graceful shutdown"
        );

        // Create TCP listener
        let listener = TcpListener::bind(addr)
            .await
            .map_err(|e| ApiServerError::BindError {
                addr: addr.to_string(),
                source: e,
            })?;

        info!(addr = %addr, "API server listening");

        // Start the server with graceful shutdown
        axum::serve(listener, self.router)
            .with_graceful_shutdown(shutdown_signal)
            .await
            .map_err(|e| ApiServerError::ServerError {
                message: e.to_string(),
            })?;

        info!("API server shut down gracefully");
        Ok(())
    }

    /// Get the server configuration
    pub fn config(&self) -> &ServerConfig {
        &self.config
    }

    /// Get the server router (for testing)
    pub fn router(&self) -> &Router {
        &self.router
    }
}

/// API server errors
#[derive(Debug, thiserror::Error)]
pub enum ApiServerError {
    #[error("Failed to bind to address {addr}: {source}")]
    BindError {
        addr: String,
        #[source]
        source: std::io::Error,
    },

    #[error("Server error: {message}")]
    ServerError { message: String },

    #[error("Configuration error: {message}")]
    ConfigError { message: String },
}

/// Server builder for fluent configuration
pub struct ApiServerBuilder {
    config: ServerConfig,
}

impl ApiServerBuilder {
    /// Create a new server builder
    pub fn new() -> Self {
        Self {
            config: ServerConfig::default(),
        }
    }

    /// Set the server host
    pub fn host<S: Into<String>>(mut self, host: S) -> Self {
        self.config.host = host.into();
        self
    }

    /// Set the server port
    pub fn port(mut self, port: u16) -> Self {
        self.config.port = port;
        self
    }

    /// Set the maximum request body size
    pub fn max_body_size(mut self, size: usize) -> Self {
        self.config.max_body_size = size;
        self
    }

    /// Set the request timeout
    pub fn timeout_seconds(mut self, timeout: u64) -> Self {
        self.config.timeout_seconds = timeout;
        self
    }

    /// Enable or disable CORS
    pub fn cors(mut self, enable: bool) -> Self {
        self.config.enable_cors = enable;
        self
    }

    /// Enable or disable request logging
    pub fn logging(mut self, enable: bool) -> Self {
        self.config.enable_logging = enable;
        self
    }

    /// Build the API server
    pub fn build(self) -> ApiServer {
        ApiServer::new(self.config)
    }
}

impl Default for ApiServerBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Utility functions for server management
///
/// Create a shutdown signal that triggers on CTRL+C
pub async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {
            info!("Received CTRL+C, shutting down");
        },
        _ = terminate => {
            info!("Received SIGTERM, shutting down");
        },
    }
}

/// Health check for the server
pub async fn health_check(addr: SocketAddr) -> Result<bool, reqwest::Error> {
    let url = format!("http://{}/health", addr);
    let response = reqwest::get(&url).await?;
    Ok(response.status().is_success())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_server_builder() {
        let server = ApiServerBuilder::new()
            .host("127.0.0.1")
            .port(8081)
            .max_body_size(2048)
            .timeout_seconds(60)
            .cors(false)
            .logging(true)
            .build();

        assert_eq!(server.config.host, "127.0.0.1");
        assert_eq!(server.config.port, 8081);
        assert_eq!(server.config.max_body_size, 2048);
        assert_eq!(server.config.timeout_seconds, 60);
        assert!(!server.config.enable_cors);
        assert!(server.config.enable_logging);
    }

    #[test]
    fn test_server_with_defaults() {
        let server = ApiServer::with_defaults();
        assert_eq!(server.config.host, "0.0.0.0");
        assert_eq!(server.config.port, 8080);
        assert!(server.config.enable_cors);
        assert!(server.config.enable_logging);
    }

    #[tokio::test]
    async fn test_server_creation() {
        let config = ServerConfig {
            host: "127.0.0.1".to_string(),
            port: 0, // Use port 0 to let the OS choose
            max_body_size: 1024,
            timeout_seconds: 30,
            enable_cors: true,
            enable_logging: false,
        };

        let server = ApiServer::new(config);
        assert_eq!(server.config().host, "127.0.0.1");
        assert_eq!(server.config().port, 0);
    }

    #[tokio::test]
    async fn test_shutdown_signal_timeout() {
        // Test that shutdown signal can be created (but don't wait for it)
        let shutdown_future = shutdown_signal();

        // Use a timeout to avoid waiting indefinitely
        let result = tokio::time::timeout(Duration::from_millis(10), shutdown_future).await;
        assert!(result.is_err()); // Should timeout since no signal is sent
    }

    #[tokio::test]
    async fn test_server_respects_host_configuration() {
        // Test that server uses the configured host address, not hardcoded 0.0.0.0
        let config = ServerConfig {
            host: "127.0.0.1".to_string(),
            port: 0, // Use port 0 to let the OS choose
            max_body_size: 1024,
            timeout_seconds: 30,
            enable_cors: true,
            enable_logging: false,
        };

        let server = ApiServer::new(config);

        // Test that the configuration is stored correctly
        assert_eq!(server.config().host, "127.0.0.1");
        assert_eq!(server.config().port, 0);

        // Test that invalid host configurations are rejected
        let invalid_config = ServerConfig {
            host: "invalid-host-format".to_string(),
            port: 8080,
            max_body_size: 1024,
            timeout_seconds: 30,
            enable_cors: true,
            enable_logging: false,
        };

        let invalid_server = ApiServer::new(invalid_config);

        // This should fail when trying to start due to invalid host format
        let result = invalid_server.start().await;
        assert!(result.is_err());

        // Verify it's a configuration error
        match result.unwrap_err() {
            ApiServerError::ConfigError { message } => {
                assert!(message.contains("Invalid host:port combination"));
                assert!(message.contains("invalid-host-format"));
            }
            _ => panic!("Expected ConfigError"),
        }
    }

    #[tokio::test]
    async fn test_host_security_configurations() {
        // Test various host configurations to ensure security

        // Test localhost-only binding (secure)
        let localhost_config = ServerConfig {
            host: "127.0.0.1".to_string(),
            port: 0,
            max_body_size: 1024,
            timeout_seconds: 30,
            enable_cors: true,
            enable_logging: false,
        };

        let localhost_server = ApiServer::new(localhost_config);
        assert_eq!(localhost_server.config().host, "127.0.0.1");

        // Test IPv6 localhost binding
        let ipv6_localhost_config = ServerConfig {
            host: "::1".to_string(),
            port: 0,
            max_body_size: 1024,
            timeout_seconds: 30,
            enable_cors: true,
            enable_logging: false,
        };

        let ipv6_server = ApiServer::new(ipv6_localhost_config);
        assert_eq!(ipv6_server.config().host, "::1");

        // Test that 0.0.0.0 is only used when explicitly configured
        let all_interfaces_config = ServerConfig {
            host: "0.0.0.0".to_string(),
            port: 0,
            max_body_size: 1024,
            timeout_seconds: 30,
            enable_cors: true,
            enable_logging: false,
        };

        let all_interfaces_server = ApiServer::new(all_interfaces_config);
        assert_eq!(all_interfaces_server.config().host, "0.0.0.0");

        // Verify that the default configuration is explicit about binding to all interfaces
        let default_server = ApiServer::with_defaults();
        assert_eq!(default_server.config().host, "0.0.0.0");
    }

    #[tokio::test]
    async fn test_server_with_state_management() {
        use crate::api::manager::DisruptorManager;

        // Create a manager instance
        let manager = Arc::new(Mutex::new(DisruptorManager::new()));

        // Create server with state
        let config = ServerConfig {
            host: "127.0.0.1".to_string(),
            port: 0,
            max_body_size: 1024,
            timeout_seconds: 30,
            enable_cors: true,
            enable_logging: false,
        };

        let server = ApiServer::with_state(config, manager);

        // Verify configuration
        assert_eq!(server.config().host, "127.0.0.1");
        assert_eq!(server.config().port, 0);

        // Verify router is created (this tests that the state management routing works)
        let _router = server.router();
    }
}
