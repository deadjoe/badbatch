//! API Middleware
//!
//! This module provides middleware components for the REST API,
//! including request logging, authentication, rate limiting, and metrics collection.

use axum::{
    extract::Request,
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};
use std::time::Instant;
use tracing::{info, warn, error};
use uuid::Uuid;

/// Request ID header name
pub const REQUEST_ID_HEADER: &str = "x-request-id";

/// Request logging middleware
pub async fn request_logging(request: Request, next: Next) -> Response {
    let start_time = Instant::now();
    let method = request.method().clone();
    let uri = request.uri().clone();
    let version = request.version();

    // Generate or extract request ID
    let request_id = request
        .headers()
        .get(REQUEST_ID_HEADER)
        .and_then(|h| h.to_str().ok())
        .map(|s| s.to_string())
        .unwrap_or_else(|| {
            // Generate a new request ID if not present
            Uuid::new_v4().to_string()
        });

    info!(
        request_id = %request_id,
        method = %method,
        uri = %uri,
        version = ?version,
        "Request started"
    );

    let response = next.run(request).await;
    let duration = start_time.elapsed();
    let status = response.status();

    let log_level = match status.as_u16() {
        200..=299 => tracing::Level::INFO,
        300..=399 => tracing::Level::INFO,
        400..=499 => tracing::Level::WARN,
        500..=599 => tracing::Level::ERROR,
        _ => tracing::Level::INFO,
    };

    match log_level {
        tracing::Level::INFO => info!(
            request_id = %request_id,
            method = %method,
            uri = %uri,
            status = %status,
            duration_ms = %duration.as_millis(),
            "Request completed"
        ),
        tracing::Level::WARN => warn!(
            request_id = %request_id,
            method = %method,
            uri = %uri,
            status = %status,
            duration_ms = %duration.as_millis(),
            "Request completed with warning"
        ),
        tracing::Level::ERROR => error!(
            request_id = %request_id,
            method = %method,
            uri = %uri,
            status = %status,
            duration_ms = %duration.as_millis(),
            "Request completed with error"
        ),
        _ => {}
    }

    response
}

/// Request metrics collection middleware
pub async fn metrics_collection(request: Request, next: Next) -> Response {
    let start_time = Instant::now();
    let method = request.method().clone();
    let uri = request.uri().path().to_string();

    let response = next.run(request).await;
    let duration = start_time.elapsed();
    let status = response.status();

    // Collect metrics (in a real implementation, this would update metrics storage)
    collect_request_metrics(&method.to_string(), &uri, status.as_u16(), duration);

    response
}

/// Security headers middleware
pub async fn security_headers(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;

    let headers = response.headers_mut();

    // Add security headers
    headers.insert("X-Content-Type-Options", "nosniff".parse().unwrap());
    headers.insert("X-Frame-Options", "DENY".parse().unwrap());
    headers.insert("X-XSS-Protection", "1; mode=block".parse().unwrap());
    headers.insert(
        "Strict-Transport-Security",
        "max-age=31536000; includeSubDomains".parse().unwrap(),
    );
    headers.insert("Referrer-Policy", "strict-origin-when-cross-origin".parse().unwrap());

    response
}

/// Rate limiting middleware (simplified implementation)
pub async fn rate_limiting(request: Request, next: Next) -> Response {
    // Extract client identifier (IP address, API key, etc.)
    let client_id = extract_client_id(&request);

    // Check rate limit (in a real implementation, this would use Redis or similar)
    if is_rate_limited(&client_id) {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            "Rate limit exceeded. Please try again later.",
        ).into_response();
    }

    // Record request for rate limiting
    record_request(&client_id);

    next.run(request).await
}

/// Authentication middleware with API key support
///
/// This middleware checks for API keys in the Authorization header or X-API-Key header.
/// If authentication is enabled in the configuration, it validates the API key.
/// If disabled, it allows all requests through (for development/testing).
pub async fn authentication(request: Request, next: Next) -> Response {
    // Check if authentication is enabled (can be configured via environment)
    let auth_enabled = std::env::var("BADBATCH_AUTH_ENABLED")
        .unwrap_or_else(|_| "false".to_string())
        .parse::<bool>()
        .unwrap_or(false);

    if !auth_enabled {
        // Authentication disabled - allow all requests
        return next.run(request).await;
    }

    // Extract API key from headers
    let headers = request.headers();
    let api_key = extract_api_key(headers);

    match api_key {
        Some(key) => {
            if validate_api_key(&key) {
                // Valid API key - proceed with request
                next.run(request).await
            } else {
                // Invalid API key
                tracing::warn!("Invalid API key attempted: {}", key);
                (StatusCode::UNAUTHORIZED, "Invalid API key").into_response()
            }
        }
        None => {
            // No API key provided
            tracing::warn!("Request without API key when authentication is enabled");
            (StatusCode::UNAUTHORIZED, "API key required").into_response()
        }
    }
}

/// Extract API key from request headers
///
/// Checks both Authorization header (Bearer token) and X-API-Key header
fn extract_api_key(headers: &axum::http::HeaderMap) -> Option<String> {
    // Check X-API-Key header first
    if let Some(api_key) = headers.get("X-API-Key") {
        if let Ok(key_str) = api_key.to_str() {
            return Some(key_str.to_string());
        }
    }

    // Check Authorization header for Bearer token
    if let Some(auth_header) = headers.get("Authorization") {
        if let Ok(auth_str) = auth_header.to_str() {
            if auth_str.starts_with("Bearer ") {
                return Some(auth_str[7..].to_string());
            }
        }
    }

    None
}

/// Validate API key against configured keys
///
/// In a production system, this would check against a database or key management service.
/// For now, it checks against environment variables or a default development key.
fn validate_api_key(api_key: &str) -> bool {
    // Get valid API keys from environment (comma-separated)
    let valid_keys = std::env::var("BADBATCH_API_KEYS")
        .unwrap_or_else(|_| "dev-key-12345,admin-key-67890".to_string());

    valid_keys
        .split(',')
        .map(|key| key.trim())
        .any(|valid_key| valid_key == api_key)
}

/// CORS middleware (custom implementation)
pub async fn cors_middleware(request: Request, next: Next) -> Response {
    let origin = request.headers().get("Origin").cloned();
    let _method = request.method().clone();

    let mut response = next.run(request).await;
    let headers = response.headers_mut();

    // Add CORS headers
    if let Some(origin_value) = origin {
        headers.insert("Access-Control-Allow-Origin", origin_value.clone());
    } else {
        headers.insert("Access-Control-Allow-Origin", "*".parse().unwrap());
    }

    headers.insert(
        "Access-Control-Allow-Methods",
        "GET, POST, PUT, DELETE, OPTIONS".parse().unwrap(),
    );
    headers.insert(
        "Access-Control-Allow-Headers",
        "Content-Type, Authorization, X-Request-ID".parse().unwrap(),
    );
    headers.insert("Access-Control-Max-Age", "86400".parse().unwrap());

    response
}

// Helper functions

/// Extract client identifier for rate limiting
fn extract_client_id(request: &Request) -> String {
    // Try to get client IP from headers (considering proxies)
    if let Some(forwarded_for) = request.headers().get("X-Forwarded-For") {
        if let Ok(ip_str) = forwarded_for.to_str() {
            return ip_str.split(',').next().unwrap_or("unknown").trim().to_string();
        }
    }

    if let Some(real_ip) = request.headers().get("X-Real-IP") {
        if let Ok(ip_str) = real_ip.to_str() {
            return ip_str.to_string();
        }
    }

    // Fallback to connection info (this is a placeholder)
    "unknown".to_string()
}

/// Check if client is rate limited (placeholder implementation)
fn is_rate_limited(_client_id: &str) -> bool {
    // In a real implementation, this would check against a rate limiting store
    // For now, always allow requests
    false
}

/// Record request for rate limiting (placeholder implementation)
fn record_request(_client_id: &str) {
    // In a real implementation, this would update rate limiting counters
}



/// Collect request metrics (placeholder implementation)
fn collect_request_metrics(
    _method: &str,
    _path: &str,
    _status_code: u16,
    _duration: std::time::Duration,
) {
    // In a real implementation, this would update metrics storage
    // Could use Prometheus metrics, InfluxDB, or similar
}

/// Middleware configuration
#[derive(Debug, Clone)]
pub struct MiddlewareConfig {
    /// Enable request logging
    pub enable_logging: bool,
    /// Enable metrics collection
    pub enable_metrics: bool,
    /// Enable security headers
    pub enable_security_headers: bool,
    /// Enable rate limiting
    pub enable_rate_limiting: bool,
    /// Enable authentication
    pub enable_authentication: bool,
    /// Enable CORS
    pub enable_cors: bool,
}

impl Default for MiddlewareConfig {
    fn default() -> Self {
        Self {
            enable_logging: true,
            enable_metrics: true,
            enable_security_headers: true,
            enable_rate_limiting: false, // Disabled by default for simplicity
            enable_authentication: false, // Disabled by default for simplicity
            enable_cors: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_middleware_config_default() {
        let config = MiddlewareConfig::default();
        assert!(config.enable_logging);
        assert!(config.enable_metrics);
        assert!(config.enable_security_headers);
        assert!(!config.enable_rate_limiting);
        assert!(!config.enable_authentication);
        assert!(config.enable_cors);
    }

    #[test]
    fn test_extract_client_id_unknown() {
        use axum::http::Request;
        use axum::body::Body;
        let request = Request::builder().body(Body::empty()).unwrap();
        let client_id = extract_client_id(&request);
        assert_eq!(client_id, "unknown");
    }

    #[test]
    fn test_validate_api_key() {
        // Test with default development keys
        assert!(validate_api_key("dev-key-12345"));
        assert!(validate_api_key("admin-key-67890"));
        assert!(!validate_api_key("invalid-key"));
    }

    #[test]
    fn test_extract_api_key() {
        use axum::http::{HeaderMap, HeaderValue};

        // Test X-API-Key header
        let mut headers = HeaderMap::new();
        headers.insert("X-API-Key", HeaderValue::from_static("test-key"));
        assert_eq!(extract_api_key(&headers), Some("test-key".to_string()));

        // Test Authorization Bearer header
        let mut headers = HeaderMap::new();
        headers.insert("Authorization", HeaderValue::from_static("Bearer bearer-token"));
        assert_eq!(extract_api_key(&headers), Some("bearer-token".to_string()));

        // Test no headers
        let headers = HeaderMap::new();
        assert_eq!(extract_api_key(&headers), None);
    }

    #[test]
    fn test_is_rate_limited_placeholder() {
        assert!(!is_rate_limited("any_client"));
    }
}
