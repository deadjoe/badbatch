//! API Data Models
//!
//! This module defines all the data structures used in the REST API,
//! including request and response models for Disruptor operations.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

/// Request to create a new Disruptor instance
#[derive(Debug, Clone, Deserialize)]
pub struct CreateDisruptorRequest {
    /// Buffer size (must be power of 2)
    pub buffer_size: usize,
    /// Producer type: "single" or "multi"
    pub producer_type: String,
    /// Wait strategy: "blocking", "yielding", "busy_spin", or "sleeping"
    pub wait_strategy: String,
    /// Optional name for the Disruptor instance
    pub name: Option<String>,
    /// Configuration options
    #[serde(default)]
    pub config: DisruptorConfig,
}

/// Disruptor configuration options
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DisruptorConfig {
    /// Enable metrics collection
    #[serde(default = "default_true")]
    pub enable_metrics: bool,
    /// Enable event logging
    #[serde(default)]
    pub enable_logging: bool,
    /// Maximum events per second (0 = unlimited)
    #[serde(default)]
    pub max_events_per_second: u64,
    /// Event retention time in seconds
    #[serde(default = "default_retention")]
    pub retention_seconds: u64,
}

impl Default for DisruptorConfig {
    fn default() -> Self {
        Self {
            enable_metrics: true,
            enable_logging: false,
            max_events_per_second: 0,
            retention_seconds: 3600, // 1 hour
        }
    }
}

fn default_true() -> bool {
    true
}

fn default_retention() -> u64 {
    3600
}

/// Response when creating a Disruptor
#[derive(Debug, Serialize)]
pub struct CreateDisruptorResponse {
    /// Unique identifier for the Disruptor instance
    pub id: String,
    /// Disruptor configuration
    pub config: DisruptorInfo,
    /// Creation timestamp
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Disruptor information
#[derive(Debug, Clone, Serialize)]
pub struct DisruptorInfo {
    /// Disruptor ID
    pub id: String,
    /// Optional name
    pub name: Option<String>,
    /// Buffer size
    pub buffer_size: usize,
    /// Producer type
    pub producer_type: String,
    /// Wait strategy
    pub wait_strategy: String,
    /// Current status
    pub status: DisruptorStatus,
    /// Configuration
    pub config: DisruptorConfig,
    /// Creation timestamp
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// Last activity timestamp
    pub last_activity: chrono::DateTime<chrono::Utc>,
}

/// Disruptor status
#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "lowercase")]
pub enum DisruptorStatus {
    /// Disruptor is created but not started
    Created,
    /// Disruptor is running and processing events
    Running,
    /// Disruptor is paused
    Paused,
    /// Disruptor is shutting down
    Stopping,
    /// Disruptor has been stopped
    Stopped,
    /// Disruptor encountered an error
    Error,
}

/// Request to publish a single event
#[derive(Debug, Deserialize)]
pub struct PublishEventRequest {
    /// Event data (JSON object)
    pub data: serde_json::Value,
    /// Optional event metadata
    #[serde(default)]
    pub metadata: HashMap<String, String>,
    /// Optional correlation ID for tracking
    pub correlation_id: Option<String>,
}

/// Request to publish multiple events
#[derive(Debug, Deserialize)]
pub struct PublishBatchRequest {
    /// List of events to publish
    pub events: Vec<PublishEventRequest>,
    /// Batch metadata
    #[serde(default)]
    pub metadata: HashMap<String, String>,
}

/// Response when publishing events
#[derive(Debug, Serialize)]
pub struct PublishEventResponse {
    /// Event sequence number
    pub sequence: i64,
    /// Event ID
    pub event_id: String,
    /// Correlation ID if provided
    pub correlation_id: Option<String>,
    /// Publish timestamp
    pub published_at: chrono::DateTime<chrono::Utc>,
}

/// Response when publishing a batch of events
#[derive(Debug, Serialize)]
pub struct PublishBatchResponse {
    /// Number of events published
    pub published_count: usize,
    /// List of published event responses
    pub events: Vec<PublishEventResponse>,
    /// Batch ID
    pub batch_id: String,
    /// Publish timestamp
    pub published_at: chrono::DateTime<chrono::Utc>,
}

/// Disruptor metrics
#[derive(Debug, Serialize)]
pub struct DisruptorMetrics {
    /// Disruptor ID
    pub disruptor_id: String,
    /// Total events published
    pub total_events_published: u64,
    /// Total events processed
    pub total_events_processed: u64,
    /// Events per second (current rate)
    pub events_per_second: f64,
    /// Average processing latency in microseconds
    pub avg_latency_micros: f64,
    /// 95th percentile latency
    pub p95_latency_micros: f64,
    /// 99th percentile latency
    pub p99_latency_micros: f64,
    /// Current buffer utilization (0.0 to 1.0)
    pub buffer_utilization: f64,
    /// Number of processing errors
    pub error_count: u64,
    /// Metrics collection timestamp
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

/// Health check response for a specific Disruptor
#[derive(Debug, Serialize)]
pub struct DisruptorHealth {
    /// Disruptor ID
    pub disruptor_id: String,
    /// Health status
    pub status: HealthStatus,
    /// Uptime in seconds
    pub uptime_seconds: u64,
    /// Last heartbeat timestamp
    pub last_heartbeat: chrono::DateTime<chrono::Utc>,
    /// Health check details
    pub details: HashMap<String, serde_json::Value>,
}

/// Health status enumeration
#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "lowercase")]
pub enum HealthStatus {
    /// Service is healthy
    Healthy,
    /// Service is degraded but functional
    Degraded,
    /// Service is unhealthy
    Unhealthy,
    /// Service status is unknown
    Unknown,
}

/// List of Disruptor instances
#[derive(Debug, Serialize)]
pub struct DisruptorList {
    /// List of Disruptor instances
    pub disruptors: Vec<DisruptorInfo>,
    /// Total count
    pub total_count: usize,
    /// Pagination offset
    pub offset: usize,
    /// Pagination limit
    pub limit: usize,
}

/// Query parameters for listing Disruptors
#[derive(Debug, Deserialize)]
pub struct ListDisruptorsQuery {
    /// Pagination offset
    #[serde(default)]
    pub offset: usize,
    /// Pagination limit
    #[serde(default = "default_limit")]
    pub limit: usize,
    /// Filter by status
    pub status: Option<String>,
    /// Filter by name pattern
    pub name: Option<String>,
}

fn default_limit() -> usize {
    50
}

/// Event data structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventData {
    /// Event ID
    pub id: String,
    /// Event sequence number
    pub sequence: i64,
    /// Event data payload
    pub data: serde_json::Value,
    /// Event metadata
    pub metadata: HashMap<String, String>,
    /// Correlation ID
    pub correlation_id: Option<String>,
    /// Event creation timestamp
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// Event processing timestamp
    pub processed_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl EventData {
    /// Create a new event
    pub fn new(sequence: i64, data: serde_json::Value) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            sequence,
            data,
            metadata: HashMap::new(),
            correlation_id: None,
            created_at: chrono::Utc::now(),
            processed_at: None,
        }
    }

    /// Mark event as processed
    pub fn mark_processed(&mut self) {
        self.processed_at = Some(chrono::Utc::now());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_create_disruptor_request_deserialization() {
        let json = json!({
            "buffer_size": 1024,
            "producer_type": "single",
            "wait_strategy": "blocking",
            "name": "test-disruptor"
        });

        let request: CreateDisruptorRequest = serde_json::from_value(json).unwrap();
        assert_eq!(request.buffer_size, 1024);
        assert_eq!(request.producer_type, "single");
        assert_eq!(request.wait_strategy, "blocking");
        assert_eq!(request.name, Some("test-disruptor".to_string()));
    }

    #[test]
    fn test_disruptor_config_default() {
        let config = DisruptorConfig::default();
        assert!(config.enable_metrics);
        assert!(!config.enable_logging);
        assert_eq!(config.max_events_per_second, 0);
        assert_eq!(config.retention_seconds, 3600);
    }

    #[test]
    fn test_event_data_creation() {
        let data = json!({"message": "test"});
        let event = EventData::new(42, data.clone());

        assert_eq!(event.sequence, 42);
        assert_eq!(event.data, data);
        assert!(event.processed_at.is_none());
        assert!(!event.id.is_empty());
    }

    #[test]
    fn test_event_data_mark_processed() {
        let mut event = EventData::new(1, json!({}));
        assert!(event.processed_at.is_none());

        event.mark_processed();
        assert!(event.processed_at.is_some());
    }
}
