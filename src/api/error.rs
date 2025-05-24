//! API Error Handling
//!
//! This module provides comprehensive error handling for the REST API,
//! including proper HTTP status codes and error response formatting.

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

/// API-specific errors
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("Disruptor not found: {id}")]
    DisruptorNotFound { id: String },

    #[error("Invalid request: {message}")]
    InvalidRequest { message: String },

    #[error("Disruptor error: {0}")]
    DisruptorError(#[from] crate::disruptor::DisruptorError),

    #[error("Serialization error: {0}")]
    SerializationError(#[from] serde_json::Error),

    #[error("Internal server error: {message}")]
    InternalError { message: String },

    #[error("Service unavailable: {message}")]
    ServiceUnavailable { message: String },

    #[error("Rate limit exceeded")]
    RateLimitExceeded,

    #[error("Authentication required")]
    Unauthorized,

    #[error("Access forbidden")]
    Forbidden,

    #[error("Request timeout")]
    Timeout,

    #[error("Payload too large")]
    PayloadTooLarge,

    #[error("Unsupported media type")]
    UnsupportedMediaType,

    #[error("Method not allowed")]
    MethodNotAllowed,
}

impl ApiError {
    /// Get the appropriate HTTP status code for this error
    pub fn status_code(&self) -> StatusCode {
        match self {
            ApiError::DisruptorNotFound { .. } => StatusCode::NOT_FOUND,
            ApiError::InvalidRequest { .. } => StatusCode::BAD_REQUEST,
            ApiError::DisruptorError(_) => StatusCode::UNPROCESSABLE_ENTITY,
            ApiError::SerializationError(_) => StatusCode::BAD_REQUEST,
            ApiError::InternalError { .. } => StatusCode::INTERNAL_SERVER_ERROR,
            ApiError::ServiceUnavailable { .. } => StatusCode::SERVICE_UNAVAILABLE,
            ApiError::RateLimitExceeded => StatusCode::TOO_MANY_REQUESTS,
            ApiError::Unauthorized => StatusCode::UNAUTHORIZED,
            ApiError::Forbidden => StatusCode::FORBIDDEN,
            ApiError::Timeout => StatusCode::REQUEST_TIMEOUT,
            ApiError::PayloadTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            ApiError::UnsupportedMediaType => StatusCode::UNSUPPORTED_MEDIA_TYPE,
            ApiError::MethodNotAllowed => StatusCode::METHOD_NOT_ALLOWED,
        }
    }

    /// Get the error code for this error
    pub fn error_code(&self) -> &'static str {
        match self {
            ApiError::DisruptorNotFound { .. } => "DISRUPTOR_NOT_FOUND",
            ApiError::InvalidRequest { .. } => "INVALID_REQUEST",
            ApiError::DisruptorError(_) => "DISRUPTOR_ERROR",
            ApiError::SerializationError(_) => "SERIALIZATION_ERROR",
            ApiError::InternalError { .. } => "INTERNAL_ERROR",
            ApiError::ServiceUnavailable { .. } => "SERVICE_UNAVAILABLE",
            ApiError::RateLimitExceeded => "RATE_LIMIT_EXCEEDED",
            ApiError::Unauthorized => "UNAUTHORIZED",
            ApiError::Forbidden => "FORBIDDEN",
            ApiError::Timeout => "TIMEOUT",
            ApiError::PayloadTooLarge => "PAYLOAD_TOO_LARGE",
            ApiError::UnsupportedMediaType => "UNSUPPORTED_MEDIA_TYPE",
            ApiError::MethodNotAllowed => "METHOD_NOT_ALLOWED",
        }
    }

    /// Create an invalid request error
    pub fn invalid_request<S: Into<String>>(message: S) -> Self {
        Self::InvalidRequest {
            message: message.into(),
        }
    }

    /// Create an internal error
    pub fn internal<S: Into<String>>(message: S) -> Self {
        Self::InternalError {
            message: message.into(),
        }
    }

    /// Create a service unavailable error
    pub fn service_unavailable<S: Into<String>>(message: S) -> Self {
        Self::ServiceUnavailable {
            message: message.into(),
        }
    }

    /// Create a disruptor not found error
    pub fn disruptor_not_found<S: Into<String>>(id: S) -> Self {
        Self::DisruptorNotFound { id: id.into() }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = self.status_code();
        let error_code = self.error_code();
        let message = self.to_string();

        let body = json!({
            "status": "error",
            "error": {
                "code": error_code,
                "message": message,
            },
            "timestamp": chrono::Utc::now(),
        });

        (status, Json(body)).into_response()
    }
}

/// Result type for API operations
pub type ApiResult<T> = Result<T, ApiError>;

/// Error response structure
#[derive(Debug, serde::Serialize)]
pub struct ErrorResponse {
    /// Error code
    pub code: String,
    /// Error message
    pub message: String,
    /// Additional error details
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
    /// Timestamp when error occurred
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

impl ErrorResponse {
    /// Create a new error response
    pub fn new<S: Into<String>>(code: S, message: S) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            details: None,
            timestamp: chrono::Utc::now(),
        }
    }

    /// Add details to the error response
    pub fn with_details(mut self, details: serde_json::Value) -> Self {
        self.details = Some(details);
        self
    }
}

/// Validation error details
#[derive(Debug, serde::Serialize)]
pub struct ValidationError {
    /// Field that failed validation
    pub field: String,
    /// Validation error message
    pub message: String,
    /// Rejected value
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rejected_value: Option<serde_json::Value>,
}

impl ValidationError {
    /// Create a new validation error
    pub fn new<S: Into<String>>(field: S, message: S) -> Self {
        Self {
            field: field.into(),
            message: message.into(),
            rejected_value: None,
        }
    }

    /// Add the rejected value
    pub fn with_value(mut self, value: serde_json::Value) -> Self {
        self.rejected_value = Some(value);
        self
    }
}

/// Helper function to create validation errors
pub fn validation_error<S: Into<String>>(field: S, message: S) -> ApiError {
    let error = ValidationError::new(field, message);
    ApiError::InvalidRequest {
        message: format!("Validation failed: {}", error.message),
    }
}

/// Helper function to create multiple validation errors
pub fn validation_errors(errors: Vec<ValidationError>) -> ApiError {
    let messages: Vec<String> = errors
        .iter()
        .map(|e| format!("{}: {}", e.field, e.message))
        .collect();

    ApiError::InvalidRequest {
        message: format!("Validation failed: {}", messages.join(", ")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_api_error_status_codes() {
        assert_eq!(
            ApiError::DisruptorNotFound { id: "test".to_string() }.status_code(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            ApiError::InvalidRequest { message: "test".to_string() }.status_code(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            ApiError::InternalError { message: "test".to_string() }.status_code(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    #[test]
    fn test_api_error_codes() {
        assert_eq!(
            ApiError::DisruptorNotFound { id: "test".to_string() }.error_code(),
            "DISRUPTOR_NOT_FOUND"
        );
        assert_eq!(
            ApiError::InvalidRequest { message: "test".to_string() }.error_code(),
            "INVALID_REQUEST"
        );
    }

    #[test]
    fn test_validation_error() {
        let error = ValidationError::new("field1", "is required");
        assert_eq!(error.field, "field1");
        assert_eq!(error.message, "is required");
        assert!(error.rejected_value.is_none());
    }

    #[test]
    fn test_error_response() {
        let response = ErrorResponse::new("TEST_ERROR", "Test message");
        assert_eq!(response.code, "TEST_ERROR");
        assert_eq!(response.message, "Test message");
        assert!(response.details.is_none());
    }
}
