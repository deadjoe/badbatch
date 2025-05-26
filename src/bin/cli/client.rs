//! BadBatch HTTP Client
//!
//! HTTP client for communicating with BadBatch server API.

use reqwest::{Client, Response};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Duration;
use url::Url;

use crate::cli::{CliError, CliResult};

/// BadBatch HTTP client
#[derive(Debug)]
pub struct BadBatchClient {
    client: Client,
    base_url: Url,
}

impl BadBatchClient {
    /// Create a new client
    pub fn new(endpoint: &str) -> CliResult<Self> {
        let base_url = Url::parse(endpoint)
            .map_err(|e| CliError::invalid_input(format!("Invalid endpoint URL: {}", e)))?;

        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent(format!("badbatch-cli/{}", env!("CARGO_PKG_VERSION")))
            .build()?;

        Ok(Self { client, base_url })
    }

    /// Handle HTTP response and convert errors
    async fn handle_response<T>(response: Response) -> CliResult<T>
    where
        T: for<'de> Deserialize<'de>,
    {
        let status = response.status();

        if status.is_success() {
            response.json().await.map_err(CliError::from)
        } else {
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            Err(CliError::server(status.as_u16(), error_text))
        }
    }

    /// Get system metrics
    pub async fn get_system_metrics(&self) -> CliResult<SystemMetrics> {
        let url = self.base_url.join("/metrics")?;
        let response = self.client.get(url).send().await?;
        Self::handle_response(response).await
    }

    /// Get API version
    #[allow(dead_code)]
    pub async fn get_version(&self) -> CliResult<VersionResponse> {
        let url = self.base_url.join("/api/v1/version")?;
        let response = self.client.get(url).send().await?;
        Self::handle_response(response).await
    }

    /// Create a disruptor
    pub async fn create_disruptor(
        &self,
        request: CreateDisruptorRequest,
    ) -> CliResult<DisruptorResponse> {
        let url = self.base_url.join("/api/v1/disruptor")?;
        let response = self.client.post(url).json(&request).send().await?;
        Self::handle_response(response).await
    }

    /// List all disruptors
    pub async fn list_disruptors(&self) -> CliResult<Vec<DisruptorInfo>> {
        let url = self.base_url.join("/api/v1/disruptor")?;
        let response = self.client.get(url).send().await?;
        Self::handle_response(response).await
    }

    /// Get disruptor by ID
    pub async fn get_disruptor(&self, id: &str) -> CliResult<DisruptorInfo> {
        let url = self.base_url.join(&format!("/api/v1/disruptor/{}", id))?;
        let response = self.client.get(url).send().await?;
        Self::handle_response(response).await
    }

    /// Delete disruptor
    pub async fn delete_disruptor(&self, id: &str) -> CliResult<()> {
        let url = self.base_url.join(&format!("/api/v1/disruptor/{}", id))?;
        let response = self.client.delete(url).send().await?;

        let status = response.status();
        if status.is_success() {
            Ok(())
        } else {
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            Err(CliError::server(status.as_u16(), error_text))
        }
    }

    /// Start disruptor
    pub async fn start_disruptor(&self, id: &str) -> CliResult<()> {
        let url = self
            .base_url
            .join(&format!("/api/v1/disruptor/{}/start", id))?;
        let response = self.client.post(url).send().await?;

        let status = response.status();
        if status.is_success() {
            Ok(())
        } else {
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            Err(CliError::server(status.as_u16(), error_text))
        }
    }

    /// Stop disruptor
    pub async fn stop_disruptor(&self, id: &str) -> CliResult<()> {
        let url = self
            .base_url
            .join(&format!("/api/v1/disruptor/{}/stop", id))?;
        let response = self.client.post(url).send().await?;

        let status = response.status();
        if status.is_success() {
            Ok(())
        } else {
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            Err(CliError::server(status.as_u16(), error_text))
        }
    }

    /// Pause disruptor
    pub async fn pause_disruptor(&self, id: &str) -> CliResult<()> {
        let url = self
            .base_url
            .join(&format!("/api/v1/disruptor/{}/pause", id))?;
        let response = self.client.post(url).send().await?;

        let status = response.status();
        if status.is_success() {
            Ok(())
        } else {
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            Err(CliError::server(status.as_u16(), error_text))
        }
    }

    /// Resume disruptor
    pub async fn resume_disruptor(&self, id: &str) -> CliResult<()> {
        let url = self
            .base_url
            .join(&format!("/api/v1/disruptor/{}/resume", id))?;
        let response = self.client.post(url).send().await?;

        let status = response.status();
        if status.is_success() {
            Ok(())
        } else {
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            Err(CliError::server(status.as_u16(), error_text))
        }
    }

    /// Publish single event
    pub async fn publish_event(
        &self,
        disruptor_id: &str,
        request: PublishEventRequest,
    ) -> CliResult<PublishResponse> {
        let url = self
            .base_url
            .join(&format!("/api/v1/disruptor/{}/events", disruptor_id))?;
        let response = self.client.post(url).json(&request).send().await?;
        Self::handle_response(response).await
    }

    /// Publish batch events
    pub async fn publish_batch(
        &self,
        disruptor_id: &str,
        request: PublishBatchRequest,
    ) -> CliResult<PublishBatchResponse> {
        let url = self
            .base_url
            .join(&format!("/api/v1/disruptor/{}/events/batch", disruptor_id))?;
        let response = self.client.post(url).json(&request).send().await?;
        Self::handle_response(response).await
    }

    /// Get disruptor metrics
    pub async fn get_disruptor_metrics(&self, id: &str) -> CliResult<DisruptorMetrics> {
        let url = self
            .base_url
            .join(&format!("/api/v1/disruptor/{}/metrics", id))?;
        let response = self.client.get(url).send().await?;
        Self::handle_response(response).await
    }

    /// Get disruptor health
    #[allow(dead_code)]
    pub async fn get_disruptor_health(&self, id: &str) -> CliResult<DisruptorHealth> {
        let url = self
            .base_url
            .join(&format!("/api/v1/disruptor/{}/health", id))?;
        let response = self.client.get(url).send().await?;
        Self::handle_response(response).await
    }
}

// API Response Types
#[derive(Debug, Serialize, Deserialize)]
pub struct HealthResponse {
    pub status: String,
    pub timestamp: String,
    pub version: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SystemMetrics {
    pub uptime_seconds: u64,
    pub memory_usage_bytes: u64,
    pub cpu_usage_percent: f64,
    pub active_disruptors: usize,
    pub total_events_processed: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct VersionResponse {
    pub version: String,
    pub build_date: String,
    pub git_commit: String,
}

#[derive(Debug, Serialize)]
pub struct CreateDisruptorRequest {
    pub name: String,
    pub buffer_size: usize,
    pub producer_type: String,
    pub wait_strategy: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DisruptorResponse {
    pub id: String,
    pub message: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DisruptorInfo {
    pub id: String,
    pub name: String,
    pub buffer_size: usize,
    pub producer_type: String,
    pub wait_strategy: String,
    pub status: String,
    pub created_at: String,
}

#[derive(Debug, Serialize)]
pub struct PublishEventRequest {
    pub data: serde_json::Value,
    pub metadata: Option<HashMap<String, String>>,
}

#[derive(Debug, Serialize)]
pub struct PublishBatchRequest {
    pub events: Vec<EventData>,
}

#[derive(Debug, Serialize)]
pub struct EventData {
    pub data: serde_json::Value,
    pub metadata: Option<HashMap<String, String>>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PublishResponse {
    pub sequence: u64,
    pub timestamp: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PublishBatchResponse {
    pub sequences: Vec<u64>,
    pub count: usize,
    pub timestamp: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DisruptorMetrics {
    pub events_processed: u64,
    pub events_per_second: f64,
    pub buffer_utilization: f64,
    pub producer_count: usize,
    pub consumer_count: usize,
    pub last_sequence: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DisruptorHealth {
    pub status: String,
    pub healthy: bool,
    pub last_check: String,
    pub issues: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_client_creation() {
        // Test valid URL
        let client = BadBatchClient::new("http://localhost:8080");
        assert!(client.is_ok());

        // Test invalid URL
        let client = BadBatchClient::new("invalid-url");
        assert!(client.is_err());
        assert!(matches!(client.unwrap_err(), CliError::InvalidInput { .. }));
    }

    #[test]
    fn test_request_structs_serialization() {
        // Test CreateDisruptorRequest
        let create_req = CreateDisruptorRequest {
            name: "test-disruptor".to_string(),
            buffer_size: 1024,
            producer_type: "single".to_string(),
            wait_strategy: "blocking".to_string(),
        };
        let json = serde_json::to_string(&create_req).unwrap();
        assert!(json.contains("test-disruptor"));
        assert!(json.contains("1024"));

        // Test PublishEventRequest
        let mut metadata = HashMap::new();
        metadata.insert("key".to_string(), "value".to_string());

        let publish_req = PublishEventRequest {
            data: serde_json::json!({"message": "test"}),
            metadata: Some(metadata),
        };
        let json = serde_json::to_string(&publish_req).unwrap();
        assert!(json.contains("test"));
        assert!(json.contains("key"));

        // Test PublishBatchRequest
        let event_data = EventData {
            data: serde_json::json!({"id": 1}),
            metadata: None,
        };
        let batch_req = PublishBatchRequest {
            events: vec![event_data],
        };
        let json = serde_json::to_string(&batch_req).unwrap();
        assert!(json.contains("events"));
    }

    #[test]
    fn test_response_structs_deserialization() {
        // Test HealthResponse
        let health_json = r#"{
            "status": "healthy",
            "timestamp": "2023-01-01T00:00:00Z",
            "version": "1.0.0"
        }"#;
        let health: HealthResponse = serde_json::from_str(health_json).unwrap();
        assert_eq!(health.status, "healthy");
        assert_eq!(health.version, "1.0.0");

        // Test SystemMetrics
        let metrics_json = r#"{
            "uptime_seconds": 3600,
            "memory_usage_bytes": 1048576,
            "cpu_usage_percent": 25.5,
            "active_disruptors": 2,
            "total_events_processed": 1000
        }"#;
        let metrics: SystemMetrics = serde_json::from_str(metrics_json).unwrap();
        assert_eq!(metrics.uptime_seconds, 3600);
        assert_eq!(metrics.active_disruptors, 2);

        // Test DisruptorInfo
        let disruptor_json = r#"{
            "id": "test-id",
            "name": "test-disruptor",
            "buffer_size": 1024,
            "producer_type": "single",
            "wait_strategy": "blocking",
            "status": "running",
            "created_at": "2023-01-01T00:00:00Z"
        }"#;
        let disruptor: DisruptorInfo = serde_json::from_str(disruptor_json).unwrap();
        assert_eq!(disruptor.id, "test-id");
        assert_eq!(disruptor.buffer_size, 1024);

        // Test PublishResponse
        let publish_json = r#"{
            "sequence": 42,
            "timestamp": "2023-01-01T00:00:00Z"
        }"#;
        let publish: PublishResponse = serde_json::from_str(publish_json).unwrap();
        assert_eq!(publish.sequence, 42);

        // Test DisruptorMetrics
        let metrics_json = r#"{
            "events_processed": 1000,
            "events_per_second": 100.5,
            "buffer_utilization": 0.75,
            "producer_count": 1,
            "consumer_count": 2,
            "last_sequence": 999
        }"#;
        let metrics: DisruptorMetrics = serde_json::from_str(metrics_json).unwrap();
        assert_eq!(metrics.events_processed, 1000);
        assert_eq!(metrics.producer_count, 1);
    }

    #[test]
    fn test_event_data_creation() {
        let mut metadata = HashMap::new();
        metadata.insert("source".to_string(), "test".to_string());

        let event = EventData {
            data: serde_json::json!({"message": "hello"}),
            metadata: Some(metadata),
        };

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("hello"));
        assert!(json.contains("source"));
    }

    #[test]
    fn test_disruptor_health_deserialization() {
        let health_json = r#"{
            "status": "healthy",
            "healthy": true,
            "last_check": "2023-01-01T00:00:00Z",
            "issues": []
        }"#;
        let health: DisruptorHealth = serde_json::from_str(health_json).unwrap();
        assert_eq!(health.status, "healthy");
        assert!(health.healthy);
        assert!(health.issues.is_empty());

        // Test with issues
        let health_with_issues_json = r#"{
            "status": "degraded",
            "healthy": false,
            "last_check": "2023-01-01T00:00:00Z",
            "issues": ["High latency", "Memory pressure"]
        }"#;
        let health: DisruptorHealth = serde_json::from_str(health_with_issues_json).unwrap();
        assert_eq!(health.status, "degraded");
        assert!(!health.healthy);
        assert_eq!(health.issues.len(), 2);
    }
}
