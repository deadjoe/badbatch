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
            let error_text = response.text().await.unwrap_or_else(|_| "Unknown error".to_string());
            Err(CliError::server(status.as_u16(), error_text))
        }
    }

    /// Get system health
    pub async fn get_health(&self) -> CliResult<HealthResponse> {
        let url = self.base_url.join("/health")?;
        let response = self.client.get(url).send().await?;
        Self::handle_response(response).await
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
    pub async fn create_disruptor(&self, request: CreateDisruptorRequest) -> CliResult<DisruptorResponse> {
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
            let error_text = response.text().await.unwrap_or_else(|_| "Unknown error".to_string());
            Err(CliError::server(status.as_u16(), error_text))
        }
    }

    /// Start disruptor
    pub async fn start_disruptor(&self, id: &str) -> CliResult<()> {
        let url = self.base_url.join(&format!("/api/v1/disruptor/{}/start", id))?;
        let response = self.client.post(url).send().await?;

        let status = response.status();
        if status.is_success() {
            Ok(())
        } else {
            let error_text = response.text().await.unwrap_or_else(|_| "Unknown error".to_string());
            Err(CliError::server(status.as_u16(), error_text))
        }
    }

    /// Stop disruptor
    pub async fn stop_disruptor(&self, id: &str) -> CliResult<()> {
        let url = self.base_url.join(&format!("/api/v1/disruptor/{}/stop", id))?;
        let response = self.client.post(url).send().await?;

        let status = response.status();
        if status.is_success() {
            Ok(())
        } else {
            let error_text = response.text().await.unwrap_or_else(|_| "Unknown error".to_string());
            Err(CliError::server(status.as_u16(), error_text))
        }
    }

    /// Pause disruptor
    pub async fn pause_disruptor(&self, id: &str) -> CliResult<()> {
        let url = self.base_url.join(&format!("/api/v1/disruptor/{}/pause", id))?;
        let response = self.client.post(url).send().await?;

        let status = response.status();
        if status.is_success() {
            Ok(())
        } else {
            let error_text = response.text().await.unwrap_or_else(|_| "Unknown error".to_string());
            Err(CliError::server(status.as_u16(), error_text))
        }
    }

    /// Resume disruptor
    pub async fn resume_disruptor(&self, id: &str) -> CliResult<()> {
        let url = self.base_url.join(&format!("/api/v1/disruptor/{}/resume", id))?;
        let response = self.client.post(url).send().await?;

        let status = response.status();
        if status.is_success() {
            Ok(())
        } else {
            let error_text = response.text().await.unwrap_or_else(|_| "Unknown error".to_string());
            Err(CliError::server(status.as_u16(), error_text))
        }
    }

    /// Publish single event
    pub async fn publish_event(&self, disruptor_id: &str, request: PublishEventRequest) -> CliResult<PublishResponse> {
        let url = self.base_url.join(&format!("/api/v1/disruptor/{}/events", disruptor_id))?;
        let response = self.client.post(url).json(&request).send().await?;
        Self::handle_response(response).await
    }

    /// Publish batch events
    pub async fn publish_batch(&self, disruptor_id: &str, request: PublishBatchRequest) -> CliResult<PublishBatchResponse> {
        let url = self.base_url.join(&format!("/api/v1/disruptor/{}/events/batch", disruptor_id))?;
        let response = self.client.post(url).json(&request).send().await?;
        Self::handle_response(response).await
    }

    /// Get disruptor metrics
    pub async fn get_disruptor_metrics(&self, id: &str) -> CliResult<DisruptorMetrics> {
        let url = self.base_url.join(&format!("/api/v1/disruptor/{}/metrics", id))?;
        let response = self.client.get(url).send().await?;
        Self::handle_response(response).await
    }

    /// Get disruptor health
    #[allow(dead_code)]
    pub async fn get_disruptor_health(&self, id: &str) -> CliResult<DisruptorHealth> {
        let url = self.base_url.join(&format!("/api/v1/disruptor/{}/health", id))?;
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
