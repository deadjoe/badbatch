//! API Handlers
//!
//! This module contains all the HTTP request handlers for the BadBatch Disruptor API.
//! Handlers are organized by functionality and provide the business logic for each endpoint.

pub mod system;
pub mod disruptor;
pub mod events;
pub mod metrics;

// Re-export commonly used types
pub use crate::api::error::{ApiError, ApiResult};
pub use crate::api::ApiResponse;
