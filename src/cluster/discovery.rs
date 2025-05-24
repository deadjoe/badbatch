//! Service Discovery
//!
//! This module provides service discovery capabilities for the cluster,
//! allowing services to register themselves and discover other services.

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use serde::{Deserialize, Serialize};

use crate::cluster::{ClusterMembership, ClusterError, ClusterResult, NodeId};

/// Service information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceInfo {
    /// Unique service identifier
    pub id: String,
    /// Service name
    pub name: String,
    /// Service address
    pub address: String,
    /// Service port
    pub port: u16,
    /// Service tags
    pub tags: Vec<String>,
    /// Service metadata
    pub metadata: HashMap<String, String>,
    /// Node hosting the service
    pub node_id: NodeId,
    /// Health check endpoint
    pub health_check: Option<String>,
    /// Registration timestamp
    pub registered_at: chrono::DateTime<chrono::Utc>,
}

/// Service registry
pub struct ServiceRegistry {
    /// Registered services
    services: Arc<RwLock<HashMap<String, ServiceInfo>>>,
    /// Services by name index
    services_by_name: Arc<RwLock<HashMap<String, Vec<String>>>>,
}

impl ServiceRegistry {
    /// Create a new service registry
    pub fn new() -> Self {
        Self {
            services: Arc::new(RwLock::new(HashMap::new())),
            services_by_name: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Register a service
    pub async fn register(&self, service: ServiceInfo) -> ClusterResult<()> {
        let service_id = service.id.clone();
        let service_name = service.name.clone();

        {
            let mut services = self.services.write().await;
            services.insert(service_id.clone(), service);
        }

        {
            let mut by_name = self.services_by_name.write().await;
            by_name.entry(service_name).or_insert_with(Vec::new).push(service_id);
        }

        Ok(())
    }

    /// Unregister a service
    pub async fn unregister(&self, service_id: &str) -> ClusterResult<()> {
        let service_name = {
            let mut services = self.services.write().await;
            services.remove(service_id).map(|s| s.name)
        };

        if let Some(name) = service_name {
            let mut by_name = self.services_by_name.write().await;
            if let Some(service_ids) = by_name.get_mut(&name) {
                service_ids.retain(|id| id != service_id);
                if service_ids.is_empty() {
                    by_name.remove(&name);
                }
            }
        }

        Ok(())
    }

    /// Get services by name
    pub async fn get_services(&self, name: &str) -> Vec<ServiceInfo> {
        let service_ids = {
            let by_name = self.services_by_name.read().await;
            by_name.get(name).cloned().unwrap_or_default()
        };

        let services = self.services.read().await;
        service_ids
            .iter()
            .filter_map(|id| services.get(id).cloned())
            .collect()
    }

    /// Get all services
    pub async fn get_all_services(&self) -> Vec<ServiceInfo> {
        let services = self.services.read().await;
        services.values().cloned().collect()
    }
}

/// Service discovery
pub struct ServiceDiscovery {
    /// Service registry
    registry: ServiceRegistry,
    /// Cluster membership
    membership: Arc<ClusterMembership>,
}

impl ServiceDiscovery {
    /// Create a new service discovery
    pub async fn new(membership: Arc<ClusterMembership>) -> ClusterResult<Self> {
        Ok(Self {
            registry: ServiceRegistry::new(),
            membership,
        })
    }

    /// Start service discovery
    pub async fn start(&self) -> ClusterResult<()> {
        tracing::info!("Service discovery started");
        Ok(())
    }

    /// Stop service discovery
    pub async fn stop(&self) -> ClusterResult<()> {
        tracing::info!("Service discovery stopped");
        Ok(())
    }

    /// Register a service
    pub async fn register_service(&self, service: ServiceInfo) -> ClusterResult<()> {
        self.registry.register(service).await
    }

    /// Unregister a service
    pub async fn unregister_service(&self, service_id: &str) -> ClusterResult<()> {
        self.registry.unregister(service_id).await
    }

    /// Discover services by name
    pub async fn discover_services(&self, service_name: &str) -> ClusterResult<Vec<ServiceInfo>> {
        Ok(self.registry.get_services(service_name).await)
    }

    /// Get all services
    pub async fn get_all_services(&self) -> ClusterResult<Vec<ServiceInfo>> {
        Ok(self.registry.get_all_services().await)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cluster::{Node, NodeId};
    use std::sync::Arc;
    use tokio::sync::RwLock;

    #[tokio::test]
    async fn test_service_registry() {
        let registry = ServiceRegistry::new();
        
        let service = ServiceInfo {
            id: "service-1".to_string(),
            name: "test-service".to_string(),
            address: "127.0.0.1".to_string(),
            port: 8080,
            tags: vec!["web".to_string()],
            metadata: HashMap::new(),
            node_id: NodeId::generate(),
            health_check: Some("/health".to_string()),
            registered_at: chrono::Utc::now(),
        };

        registry.register(service.clone()).await.unwrap();
        
        let services = registry.get_services("test-service").await;
        assert_eq!(services.len(), 1);
        assert_eq!(services[0].id, "service-1");
    }
}
