//! Health Checking
//!
//! This module provides health checking capabilities for cluster nodes,
//! including failure detection and recovery monitoring.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use serde::{Deserialize, Serialize};

use crate::cluster::{NodeId, ClusterResult};
use crate::cluster::config::HealthConfig;

/// Health status for cluster nodes
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HealthStatus {
    /// Node is healthy
    Healthy,
    /// Node is degraded but functional
    Degraded,
    /// Node is unhealthy
    Unhealthy,
    /// Node status is unknown
    Unknown,
}

impl std::fmt::Display for HealthStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HealthStatus::Healthy => write!(f, "healthy"),
            HealthStatus::Degraded => write!(f, "degraded"),
            HealthStatus::Unhealthy => write!(f, "unhealthy"),
            HealthStatus::Unknown => write!(f, "unknown"),
        }
    }
}

/// Health check result
#[derive(Debug, Clone)]
pub struct HealthCheckResult {
    /// Node ID
    pub node_id: NodeId,
    /// Health status
    pub status: HealthStatus,
    /// Response time
    pub response_time: Duration,
    /// Error message if any
    pub error: Option<String>,
    /// Check timestamp
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

/// Health checker
pub struct HealthChecker {
    /// Configuration
    #[allow(dead_code)]
    config: HealthConfig,
    /// Node health status
    node_health: Arc<RwLock<HashMap<NodeId, HealthCheckResult>>>,
    /// Running state
    running: Arc<std::sync::atomic::AtomicBool>,
}

impl HealthChecker {
    /// Create a new health checker
    pub async fn new(config: HealthConfig) -> ClusterResult<Self> {
        Ok(Self {
            config,
            node_health: Arc::new(RwLock::new(HashMap::new())),
            running: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        })
    }

    /// Start the health checker
    pub async fn start(&self) -> ClusterResult<()> {
        if self.running.load(std::sync::atomic::Ordering::SeqCst) {
            return Ok(());
        }

        self.running.store(true, std::sync::atomic::Ordering::SeqCst);
        tracing::info!("Health checker started");
        Ok(())
    }

    /// Stop the health checker
    pub async fn stop(&self) -> ClusterResult<()> {
        if !self.running.load(std::sync::atomic::Ordering::SeqCst) {
            return Ok(());
        }

        self.running.store(false, std::sync::atomic::Ordering::SeqCst);
        tracing::info!("Health checker stopped");
        Ok(())
    }

    /// Check health of a node
    pub async fn check_node_health(&self, node_id: &NodeId) -> ClusterResult<HealthCheckResult> {
        let start_time = std::time::Instant::now();

        // Placeholder health check - in real implementation would ping the node
        let status = HealthStatus::Healthy;
        let response_time = start_time.elapsed();

        let result = HealthCheckResult {
            node_id: node_id.clone(),
            status,
            response_time,
            error: None,
            timestamp: chrono::Utc::now(),
        };

        // Store the result
        {
            let mut health = self.node_health.write().await;
            health.insert(node_id.clone(), result.clone());
        }

        Ok(result)
    }

    /// Get health status for a node
    pub async fn get_node_health(&self, node_id: &NodeId) -> Option<HealthCheckResult> {
        let health = self.node_health.read().await;
        health.get(node_id).cloned()
    }

    /// Get all healthy nodes
    pub async fn get_healthy_nodes(&self) -> ClusterResult<Vec<NodeId>> {
        let health = self.node_health.read().await;
        Ok(health
            .iter()
            .filter(|(_, result)| result.status == HealthStatus::Healthy)
            .map(|(node_id, _)| node_id.clone())
            .collect())
    }

    /// Get all unhealthy nodes
    pub async fn get_unhealthy_nodes(&self) -> ClusterResult<Vec<NodeId>> {
        let health = self.node_health.read().await;
        Ok(health
            .iter()
            .filter(|(_, result)| result.status == HealthStatus::Unhealthy)
            .map(|(node_id, _)| node_id.clone())
            .collect())
    }

    /// Remove health status for a node
    pub async fn remove_node(&self, node_id: &NodeId) {
        let mut health = self.node_health.write().await;
        health.remove(node_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cluster::NodeId;

    #[tokio::test]
    async fn test_health_checker() {
        let config = HealthConfig {
            probe_interval: Duration::from_secs(1),
            probe_timeout: Duration::from_millis(500),
            suspect_timeout: Duration::from_secs(5),
            dead_timeout: Duration::from_secs(30),
        };

        let checker = HealthChecker::new(config).await.unwrap();
        let node_id = NodeId::generate();

        let result = checker.check_node_health(&node_id).await.unwrap();
        assert_eq!(result.node_id, node_id);
        assert_eq!(result.status, HealthStatus::Healthy);

        let stored_result = checker.get_node_health(&node_id).await;
        assert!(stored_result.is_some());
    }
}
