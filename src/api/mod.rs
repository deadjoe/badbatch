//! REST API Module
//!
//! This module provides a complete REST API interface for the BadBatch Disruptor engine.
//! It allows remote clients to interact with the Disruptor through HTTP endpoints,
//! enabling distributed usage and monitoring capabilities.
//!
//! ## API Endpoints
//!
//! ### Disruptor Management
//! - `POST /api/v1/disruptor` - Create a new Disruptor instance
//! - `GET /api/v1/disruptor/{id}` - Get Disruptor status
//! - `DELETE /api/v1/disruptor/{id}` - Shutdown a Disruptor instance
//!
//! ### Event Publishing
//! - `POST /api/v1/disruptor/{id}/events` - Publish events
//! - `POST /api/v1/disruptor/{id}/events/batch` - Publish multiple events
//!
//! ### Monitoring
//! - `GET /api/v1/disruptor/{id}/metrics` - Get performance metrics
//! - `GET /api/v1/disruptor/{id}/health` - Health check
//!
//! ### System
//! - `GET /api/v1/health` - System health check
//! - `GET /api/v1/metrics` - System-wide metrics

pub mod handlers;
pub mod manager;
pub mod models;
pub mod routes;
pub mod server;
pub mod middleware;
pub mod error;

// Re-export main types
pub use server::ApiServer;
pub use models::*;
pub use error::ApiError;

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};


/// API version
pub const API_VERSION: &str = "v1";

/// Default server configuration
#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// Server host address
    pub host: String,
    /// Server port
    pub port: u16,
    /// Maximum request body size in bytes
    pub max_body_size: usize,
    /// Request timeout in seconds
    pub timeout_seconds: u64,
    /// Enable CORS
    pub enable_cors: bool,
    /// Enable request logging
    pub enable_logging: bool,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: "0.0.0.0".to_string(),
            port: 8080,
            max_body_size: 1024 * 1024, // 1MB
            timeout_seconds: 30,
            enable_cors: true,
            enable_logging: true,
        }
    }
}

/// Standard API response wrapper
#[derive(Debug, serde::Serialize)]
pub struct ApiResponse<T> {
    /// Response status
    pub status: String,
    /// Response data
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<T>,
    /// Error message if any
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Request timestamp
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

impl<T> ApiResponse<T>
where
    T: serde::Serialize,
{
    /// Create a successful response
    pub fn success(data: T) -> Self {
        Self {
            status: "success".to_string(),
            data: Some(data),
            error: None,
            timestamp: chrono::Utc::now(),
        }
    }

    /// Create an error response
    pub fn error(message: String) -> ApiResponse<()> {
        ApiResponse {
            status: "error".to_string(),
            data: None,
            error: Some(message),
            timestamp: chrono::Utc::now(),
        }
    }
}

impl<T> IntoResponse for ApiResponse<T>
where
    T: serde::Serialize,
{
    fn into_response(self) -> Response {
        let status = if self.error.is_some() {
            StatusCode::BAD_REQUEST
        } else {
            StatusCode::OK
        };

        (status, Json(self)).into_response()
    }
}

/// Health check response
#[derive(Debug, serde::Serialize)]
pub struct HealthResponse {
    /// Service status
    pub status: String,
    /// Service version
    pub version: String,
    /// Uptime in seconds
    pub uptime_seconds: u64,
    /// Number of active Disruptor instances
    pub active_disruptors: usize,
    /// System memory usage
    pub memory_usage: MemoryUsage,
}

/// Memory usage information
#[derive(Debug, serde::Serialize)]
pub struct MemoryUsage {
    /// Used memory in bytes
    pub used_bytes: u64,
    /// Available memory in bytes
    pub available_bytes: u64,
    /// Total memory in bytes
    pub total_bytes: u64,
}

/// Metrics response
#[derive(Debug, serde::Serialize)]
pub struct MetricsResponse {
    /// Total events processed
    pub total_events_processed: u64,
    /// Events per second
    pub events_per_second: f64,
    /// Average latency in microseconds
    pub avg_latency_micros: f64,
    /// 95th percentile latency in microseconds
    pub p95_latency_micros: f64,
    /// 99th percentile latency in microseconds
    pub p99_latency_micros: f64,
    /// Number of errors
    pub error_count: u64,
    /// Timestamp of metrics collection
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_server_config_default() {
        let config = ServerConfig::default();
        assert_eq!(config.host, "0.0.0.0");
        assert_eq!(config.port, 8080);
        assert_eq!(config.max_body_size, 1024 * 1024);
        assert_eq!(config.timeout_seconds, 30);
        assert!(config.enable_cors);
        assert!(config.enable_logging);
    }

    #[test]
    fn test_api_response_success() {
        let response = ApiResponse::success("test data");
        assert_eq!(response.status, "success");
        assert_eq!(response.data, Some("test data"));
        assert!(response.error.is_none());
    }

    #[test]
    fn test_api_response_error() {
        let response = ApiResponse::<()>::error("test error".to_string());
        assert_eq!(response.status, "error");
        assert!(response.data.is_none());
        assert_eq!(response.error, Some("test error".to_string()));
    }
}
