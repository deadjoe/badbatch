//! Service Discovery
//!
//! This module provides service discovery capabilities for the cluster,
//! allowing services to register themselves and discover other services.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::cluster::{ClusterMembership, ClusterResult, NodeId};

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
#[derive(Default)]
pub struct ServiceRegistry {
    /// Registered services
    services: Arc<RwLock<HashMap<String, ServiceInfo>>>,
    /// Services by name index
    services_by_name: Arc<RwLock<HashMap<String, Vec<String>>>>,
}

impl ServiceRegistry {
    /// Create a new service registry
    pub fn new() -> Self {
        Self::default()
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
            by_name
                .entry(service_name)
                .or_insert_with(Vec::new)
                .push(service_id);
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
    #[allow(dead_code)]
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

    fn create_test_service(id: &str, name: &str, port: u16) -> ServiceInfo {
        ServiceInfo {
            id: id.to_string(),
            name: name.to_string(),
            address: "127.0.0.1".to_string(),
            port,
            tags: vec!["web".to_string()],
            metadata: HashMap::new(),
            node_id: NodeId::generate(),
            health_check: Some("/health".to_string()),
            registered_at: chrono::Utc::now(),
        }
    }

    #[tokio::test]
    async fn test_service_registry() {
        let registry = ServiceRegistry::new();
        let service = create_test_service("service-1", "test-service", 8080);

        registry.register(service.clone()).await.unwrap();

        let services = registry.get_services("test-service").await;
        assert_eq!(services.len(), 1);
        assert_eq!(services[0].id, "service-1");
    }

    #[tokio::test]
    async fn test_service_registry_multiple_services() {
        let registry = ServiceRegistry::new();

        let service1 = create_test_service("service-1", "web-service", 8080);
        let service2 = create_test_service("service-2", "web-service", 8081);
        let service3 = create_test_service("service-3", "api-service", 9090);

        registry.register(service1).await.unwrap();
        registry.register(service2).await.unwrap();
        registry.register(service3).await.unwrap();

        // Test getting services by name
        let web_services = registry.get_services("web-service").await;
        assert_eq!(web_services.len(), 2);

        let api_services = registry.get_services("api-service").await;
        assert_eq!(api_services.len(), 1);

        // Test getting all services
        let all_services = registry.get_all_services().await;
        assert_eq!(all_services.len(), 3);
    }

    #[tokio::test]
    async fn test_service_registry_unregister() {
        let registry = ServiceRegistry::new();
        let service = create_test_service("service-1", "test-service", 8080);

        registry.register(service.clone()).await.unwrap();
        assert_eq!(registry.get_services("test-service").await.len(), 1);

        registry.unregister("service-1").await.unwrap();
        assert_eq!(registry.get_services("test-service").await.len(), 0);
    }

    #[tokio::test]
    async fn test_service_registry_unregister_nonexistent() {
        let registry = ServiceRegistry::new();

        // Should not error when unregistering non-existent service
        let result = registry.unregister("non-existent").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_service_registry_get_nonexistent() {
        let registry = ServiceRegistry::new();

        let services = registry.get_services("non-existent").await;
        assert_eq!(services.len(), 0);
    }

    #[tokio::test]
    async fn test_service_discovery() {
        use crate::cluster::membership::ClusterMembership;
        use crate::cluster::node::Node;
        use std::sync::Arc;
        use tokio::sync::RwLock;

        use crate::cluster::node::NodeId;
        let node_id = NodeId::generate();
        let addr = "127.0.0.1:8080".parse().unwrap();
        let node = Node::new(node_id, addr, addr);
        let local_node = Arc::new(RwLock::new(node));
        let membership = Arc::new(ClusterMembership::new(local_node).await.unwrap());
        let discovery = ServiceDiscovery::new(membership).await.unwrap();
        let service = create_test_service("service-1", "test-service", 8080);

        // Test registration
        discovery.register_service(service.clone()).await.unwrap();

        // Test discovery
        let discovered = discovery.discover_services("test-service").await.unwrap();
        assert_eq!(discovered.len(), 1);
        assert_eq!(discovered[0].id, "service-1");

        // Test get all services
        let all_services = discovery.get_all_services().await.unwrap();
        assert_eq!(all_services.len(), 1);

        // Test unregistration
        discovery.unregister_service("service-1").await.unwrap();
        let discovered_after = discovery.discover_services("test-service").await.unwrap();
        assert_eq!(discovered_after.len(), 0);
    }

    #[tokio::test]
    async fn test_service_info_creation() {
        let service = create_test_service("test-id", "test-name", 3000);

        assert_eq!(service.id, "test-id");
        assert_eq!(service.name, "test-name");
        assert_eq!(service.address, "127.0.0.1");
        assert_eq!(service.port, 3000);
        assert_eq!(service.tags, vec!["web"]);
        assert!(service.metadata.is_empty());
        assert_eq!(service.health_check, Some("/health".to_string()));
    }

    #[tokio::test]
    async fn test_service_registry_concurrent_access() {
        let registry = ServiceRegistry::new();
        let registry = std::sync::Arc::new(registry);

        let mut handles = vec![];

        // Spawn multiple tasks to register services concurrently
        for i in 0..10 {
            let registry_clone = registry.clone();
            let handle = tokio::spawn(async move {
                let service = create_test_service(
                    &format!("service-{}", i),
                    "concurrent-service",
                    8000 + i as u16,
                );
                registry_clone.register(service).await.unwrap();
            });
            handles.push(handle);
        }

        // Wait for all registrations to complete
        for handle in handles {
            handle.await.unwrap();
        }

        // Verify all services were registered
        let services = registry.get_services("concurrent-service").await;
        assert_eq!(services.len(), 10);
    }
}
