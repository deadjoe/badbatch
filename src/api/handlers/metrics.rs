//! Metrics Handlers
//!
//! This module provides handlers for retrieving performance metrics and health
//! information for Disruptor instances and the overall system.

use axum::{
    extract::Path,
    response::Json,
};
use std::collections::HashMap;

use crate::api::{
    ApiResponse,
    models::{DisruptorMetrics, DisruptorHealth, HealthStatus},
    handlers::{ApiResult, ApiError},
};

/// Get performance metrics for a specific Disruptor
pub async fn get_disruptor_metrics(
    Path(disruptor_id): Path<String>,
) -> ApiResult<Json<ApiResponse<DisruptorMetrics>>> {
    // Validate that the Disruptor exists
    validate_disruptor_exists(&disruptor_id)?;

    // Collect metrics from the Disruptor
    let metrics = collect_disruptor_metrics(&disruptor_id)?;

    Ok(Json(ApiResponse::success(metrics)))
}

/// Get health information for a specific Disruptor
pub async fn get_disruptor_health(
    Path(disruptor_id): Path<String>,
) -> ApiResult<Json<ApiResponse<DisruptorHealth>>> {
    // Validate that the Disruptor exists
    validate_disruptor_exists(&disruptor_id)?;

    // Check Disruptor health
    let health = check_disruptor_health(&disruptor_id)?;

    Ok(Json(ApiResponse::success(health)))
}

/// Get aggregated metrics for all Disruptors
pub async fn get_system_metrics() -> ApiResult<Json<ApiResponse<SystemMetrics>>> {
    let metrics = collect_system_metrics()?;
    Ok(Json(ApiResponse::success(metrics)))
}

/// Get health status for all Disruptors
pub async fn get_system_health() -> ApiResult<Json<ApiResponse<SystemHealth>>> {
    let health = check_system_health()?;
    Ok(Json(ApiResponse::success(health)))
}

// Data structures for metrics and health

#[derive(Debug, serde::Serialize)]
pub struct SystemMetrics {
    /// Total number of Disruptor instances
    pub total_disruptors: usize,
    /// Number of running Disruptors
    pub running_disruptors: usize,
    /// Aggregated metrics across all Disruptors
    pub aggregated: AggregatedMetrics,
    /// Per-Disruptor metrics
    pub per_disruptor: HashMap<String, DisruptorMetrics>,
    /// Collection timestamp
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, serde::Serialize)]
pub struct AggregatedMetrics {
    /// Total events published across all Disruptors
    pub total_events_published: u64,
    /// Total events processed across all Disruptors
    pub total_events_processed: u64,
    /// Average events per second across all Disruptors
    pub avg_events_per_second: f64,
    /// Average latency across all Disruptors
    pub avg_latency_micros: f64,
    /// Maximum latency across all Disruptors
    pub max_latency_micros: f64,
    /// Total error count across all Disruptors
    pub total_errors: u64,
    /// Average buffer utilization across all Disruptors
    pub avg_buffer_utilization: f64,
}

#[derive(Debug, serde::Serialize)]
pub struct SystemHealth {
    /// Overall system health status
    pub status: HealthStatus,
    /// Number of healthy Disruptors
    pub healthy_disruptors: usize,
    /// Number of degraded Disruptors
    pub degraded_disruptors: usize,
    /// Number of unhealthy Disruptors
    pub unhealthy_disruptors: usize,
    /// Per-Disruptor health status
    pub per_disruptor: HashMap<String, DisruptorHealth>,
    /// System uptime in seconds
    pub system_uptime_seconds: u64,
    /// Health check timestamp
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

// Implementation functions

fn validate_disruptor_exists(disruptor_id: &str) -> ApiResult<()> {
    // Placeholder: Check if Disruptor exists
    if disruptor_id == "test-id" {
        Ok(())
    } else {
        Err(ApiError::disruptor_not_found(disruptor_id))
    }
}

fn collect_disruptor_metrics(disruptor_id: &str) -> ApiResult<DisruptorMetrics> {
    // Placeholder: Collect actual metrics from the Disruptor
    // In a real implementation, this would:
    // 1. Get the Disruptor instance from the manager
    // 2. Query its internal metrics collectors
    // 3. Calculate latency percentiles
    // 4. Get buffer utilization from the ring buffer

    Ok(DisruptorMetrics {
        disruptor_id: disruptor_id.to_string(),
        total_events_published: get_events_published(disruptor_id),
        total_events_processed: get_events_processed(disruptor_id),
        events_per_second: calculate_events_per_second(disruptor_id),
        avg_latency_micros: calculate_avg_latency(disruptor_id),
        p95_latency_micros: calculate_p95_latency(disruptor_id),
        p99_latency_micros: calculate_p99_latency(disruptor_id),
        buffer_utilization: calculate_buffer_utilization(disruptor_id),
        error_count: get_error_count(disruptor_id),
        timestamp: chrono::Utc::now(),
    })
}

fn check_disruptor_health(disruptor_id: &str) -> ApiResult<DisruptorHealth> {
    // Placeholder: Check actual health of the Disruptor
    // In a real implementation, this would:
    // 1. Check if the Disruptor is responsive
    // 2. Verify event processing is working
    // 3. Check for error rates
    // 4. Validate buffer state

    let status = determine_health_status(disruptor_id);
    let mut details = HashMap::new();

    details.insert("buffer_status".to_string(), serde_json::json!("healthy"));
    details.insert("processor_status".to_string(), serde_json::json!("running"));
    details.insert("error_rate".to_string(), serde_json::json!(0.0));

    Ok(DisruptorHealth {
        disruptor_id: disruptor_id.to_string(),
        status,
        uptime_seconds: get_disruptor_uptime(disruptor_id),
        last_heartbeat: chrono::Utc::now(),
        details,
    })
}

fn collect_system_metrics() -> ApiResult<SystemMetrics> {
    // Placeholder: Collect metrics from all Disruptors
    let all_disruptor_ids = get_all_disruptor_ids();
    let mut per_disruptor = HashMap::new();

    let mut total_published = 0;
    let mut total_processed = 0;
    let mut total_errors = 0;
    let mut total_eps = 0.0;
    let mut total_latency = 0.0;
    let mut max_latency = 0.0;
    let mut total_utilization = 0.0;
    let running_count = all_disruptor_ids.len();

    for disruptor_id in &all_disruptor_ids {
        if let Ok(metrics) = collect_disruptor_metrics(disruptor_id) {
            total_published += metrics.total_events_published;
            total_processed += metrics.total_events_processed;
            total_errors += metrics.error_count;
            total_eps += metrics.events_per_second;
            total_latency += metrics.avg_latency_micros;
            total_utilization += metrics.buffer_utilization;

            if metrics.p99_latency_micros > max_latency {
                max_latency = metrics.p99_latency_micros;
            }

            per_disruptor.insert(disruptor_id.clone(), metrics);
        }
    }

    let count = running_count as f64;
    let aggregated = AggregatedMetrics {
        total_events_published: total_published,
        total_events_processed: total_processed,
        avg_events_per_second: if count > 0.0 { total_eps / count } else { 0.0 },
        avg_latency_micros: if count > 0.0 { total_latency / count } else { 0.0 },
        max_latency_micros: max_latency,
        total_errors,
        avg_buffer_utilization: if count > 0.0 { total_utilization / count } else { 0.0 },
    };

    Ok(SystemMetrics {
        total_disruptors: all_disruptor_ids.len(),
        running_disruptors: running_count,
        aggregated,
        per_disruptor,
        timestamp: chrono::Utc::now(),
    })
}

fn check_system_health() -> ApiResult<SystemHealth> {
    // Placeholder: Check health of all Disruptors
    let all_disruptor_ids = get_all_disruptor_ids();
    let mut per_disruptor = HashMap::new();

    let mut healthy_count = 0;
    let mut degraded_count = 0;
    let mut unhealthy_count = 0;

    for disruptor_id in &all_disruptor_ids {
        if let Ok(health) = check_disruptor_health(disruptor_id) {
            match health.status {
                HealthStatus::Healthy => healthy_count += 1,
                HealthStatus::Degraded => degraded_count += 1,
                HealthStatus::Unhealthy => unhealthy_count += 1,
                HealthStatus::Unknown => unhealthy_count += 1,
            }
            per_disruptor.insert(disruptor_id.clone(), health);
        }
    }

    // Determine overall system health
    let overall_status = if unhealthy_count > 0 {
        HealthStatus::Unhealthy
    } else if degraded_count > 0 {
        HealthStatus::Degraded
    } else if healthy_count > 0 {
        HealthStatus::Healthy
    } else {
        HealthStatus::Unknown
    };

    Ok(SystemHealth {
        status: overall_status,
        healthy_disruptors: healthy_count,
        degraded_disruptors: degraded_count,
        unhealthy_disruptors: unhealthy_count,
        per_disruptor,
        system_uptime_seconds: get_system_uptime(),
        timestamp: chrono::Utc::now(),
    })
}

// Helper functions (placeholder implementations)

fn get_all_disruptor_ids() -> Vec<String> {
    // Placeholder: Get all Disruptor IDs from the manager
    vec!["test-id".to_string()]
}

fn get_events_published(_disruptor_id: &str) -> u64 {
    // Placeholder: Get actual published event count
    1000
}

fn get_events_processed(_disruptor_id: &str) -> u64 {
    // Placeholder: Get actual processed event count
    950
}

fn calculate_events_per_second(_disruptor_id: &str) -> f64 {
    // Placeholder: Calculate actual EPS
    100.0
}

fn calculate_avg_latency(_disruptor_id: &str) -> f64 {
    // Placeholder: Calculate actual average latency
    50.0
}

fn calculate_p95_latency(_disruptor_id: &str) -> f64 {
    // Placeholder: Calculate actual P95 latency
    75.0
}

fn calculate_p99_latency(_disruptor_id: &str) -> f64 {
    // Placeholder: Calculate actual P99 latency
    100.0
}

fn calculate_buffer_utilization(_disruptor_id: &str) -> f64 {
    // Placeholder: Calculate actual buffer utilization
    0.25
}

fn get_error_count(_disruptor_id: &str) -> u64 {
    // Placeholder: Get actual error count
    5
}

fn determine_health_status(_disruptor_id: &str) -> HealthStatus {
    // Placeholder: Determine actual health status
    HealthStatus::Healthy
}

fn get_disruptor_uptime(_disruptor_id: &str) -> u64 {
    // Placeholder: Get actual uptime
    3600
}

fn get_system_uptime() -> u64 {
    // Placeholder: Get actual system uptime
    7200
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_get_disruptor_metrics() {
        let result = get_disruptor_metrics(axum::extract::Path("test-id".to_string())).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_get_disruptor_health() {
        let result = get_disruptor_health(axum::extract::Path("test-id".to_string())).await;
        assert!(result.is_ok());
    }

    #[test]
    fn test_collect_disruptor_metrics() {
        let result = collect_disruptor_metrics("test-id");
        assert!(result.is_ok());

        let metrics = result.unwrap();
        assert_eq!(metrics.disruptor_id, "test-id");
        assert!(metrics.total_events_published > 0);
    }

    #[test]
    fn test_check_disruptor_health() {
        let result = check_disruptor_health("test-id");
        assert!(result.is_ok());

        let health = result.unwrap();
        assert_eq!(health.disruptor_id, "test-id");
        assert!(matches!(health.status, HealthStatus::Healthy));
    }
}
