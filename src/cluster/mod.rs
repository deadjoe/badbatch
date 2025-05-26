//! Cluster Module
//!
//! This module provides distributed clustering capabilities for BadBatch Disruptor Engine.
//! It implements a Gossip-based discovery protocol similar to Consul, enabling automatic
//! node discovery, health monitoring, and distributed coordination.
//!
//! ## Features
//!
//! - **Gossip Protocol**: Efficient peer-to-peer node discovery and state propagation
//! - **Health Monitoring**: Automatic detection of node failures and recoveries
//! - **Service Discovery**: Registration and discovery of Disruptor services
//! - **Cluster Membership**: Dynamic cluster membership management
//! - **Event Replication**: Optional event replication across cluster nodes
//! - **Load Balancing**: Intelligent routing of requests across healthy nodes
//!
//! ## Architecture
//!
//! The cluster implementation follows a decentralized architecture where each node
//! maintains a partial view of the cluster state and uses gossip to propagate
//! information to other nodes.
//!
//! ```text
//! ┌─────────────┐    ┌─────────────┐    ┌─────────────┐
//! │   Node A    │◄──►│   Node B    │◄──►│   Node C    │
//! │             │    │             │    │             │
//! │ Disruptor 1 │    │ Disruptor 2 │    │ Disruptor 3 │
//! │ Disruptor 4 │    │ Disruptor 5 │    │ Disruptor 6 │
//! └─────────────┘    └─────────────┘    └─────────────┘
//!        ▲                  ▲                  ▲
//!        │                  │                  │
//!        └──────────────────┼──────────────────┘
//!                           │
//!                    Gossip Protocol
//! ```

pub mod node;
pub mod gossip;
pub mod discovery;
pub mod membership;
pub mod health;
pub mod replication;
pub mod config;
pub mod error;

// Re-export main types
pub use node::{Node, NodeId, NodeInfo, NodeState};
pub use gossip::{GossipProtocol, GossipMessage, GossipConfig};
pub use discovery::{ServiceDiscovery, ServiceRegistry, ServiceInfo};
pub use membership::{ClusterMembership, MembershipEvent};
pub use health::{HealthChecker, HealthStatus as ClusterHealthStatus};
pub use replication::EventReplicator;
pub use config::ReplicationConfig;
pub use config::ClusterConfig;
pub use error::{ClusterError, ClusterResult};

use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;

/// Default cluster configuration
impl Default for ClusterConfig {
    fn default() -> Self {
        Self {
            node_id: NodeId::generate(),
            bind_addr: "0.0.0.0:7946".parse().unwrap(),
            advertise_addr: None,
            seed_nodes: Vec::new(),
            gossip_interval: Duration::from_millis(200),
            probe_interval: Duration::from_secs(1),
            probe_timeout: Duration::from_millis(500),
            suspect_timeout: Duration::from_secs(5),
            dead_timeout: Duration::from_secs(30),
            max_gossip_packet_size: 1400,
            gossip_fanout: 3,
            enable_compression: true,
            enable_encryption: false,
            encryption_key: None,
            metadata: std::collections::HashMap::new(),
        }
    }
}

/// Main cluster manager
pub struct Cluster {
    /// Cluster configuration
    config: ClusterConfig,
    /// Local node information
    local_node: Arc<RwLock<Node>>,
    /// Cluster membership manager
    membership: Arc<ClusterMembership>,
    /// Gossip protocol handler
    gossip: Arc<GossipProtocol>,
    /// Service discovery
    discovery: Arc<ServiceDiscovery>,
    /// Health checker
    health_checker: Arc<HealthChecker>,
    /// Event replicator (optional)
    replicator: Option<Arc<EventReplicator>>,
    /// Cluster state
    state: Arc<RwLock<ClusterState>>,
}

/// Cluster state
#[derive(Debug, Clone, serde::Serialize)]
pub struct ClusterState {
    /// Cluster formation timestamp
    pub formed_at: chrono::DateTime<chrono::Utc>,
    /// Total number of nodes
    pub total_nodes: usize,
    /// Number of healthy nodes
    pub healthy_nodes: usize,
    /// Cluster leader (if any)
    pub leader: Option<NodeId>,
    /// Cluster generation/epoch
    pub generation: u64,
}

impl Cluster {
    /// Create a new cluster instance
    pub async fn new(config: ClusterConfig) -> ClusterResult<Self> {
        let local_node = Arc::new(RwLock::new(Node::new(
            config.node_id.clone(),
            config.bind_addr,
            config.advertise_addr.unwrap_or(config.bind_addr),
        )));

        let membership = Arc::new(ClusterMembership::new(local_node.clone()).await?);
        let gossip = Arc::new(GossipProtocol::new(config.gossip_config(), local_node.clone()).await?);
        let discovery = Arc::new(ServiceDiscovery::new(membership.clone()).await?);
        let health_checker = Arc::new(HealthChecker::new(config.health_config()).await?);

        let replicator = if config.enable_replication() {
            Some(Arc::new(EventReplicator::new(config.replication_config()).await?))
        } else {
            None
        };

        let state = Arc::new(RwLock::new(ClusterState {
            formed_at: chrono::Utc::now(),
            total_nodes: 1,
            healthy_nodes: 1,
            leader: None,
            generation: 0,
        }));

        Ok(Self {
            config,
            local_node,
            membership,
            gossip,
            discovery,
            health_checker,
            replicator,
            state,
        })
    }

    /// Start the cluster
    pub async fn start(&self) -> ClusterResult<()> {
        tracing::info!(
            node_id = %self.config.node_id,
            bind_addr = %self.config.bind_addr,
            "Starting cluster node"
        );

        // Start gossip protocol
        self.gossip.start().await?;

        // Start health checker
        self.health_checker.start().await?;

        // Start membership manager
        self.membership.start().await?;

        // Start service discovery
        self.discovery.start().await?;

        // Start replicator if enabled
        if let Some(replicator) = &self.replicator {
            replicator.start().await?;
        }

        // Join seed nodes if specified
        if !self.config.seed_nodes.is_empty() {
            self.join_cluster().await?;
        }

        tracing::info!(
            node_id = %self.config.node_id,
            "Cluster node started successfully"
        );

        Ok(())
    }

    /// Stop the cluster
    pub async fn stop(&self) -> ClusterResult<()> {
        tracing::info!(
            node_id = %self.config.node_id,
            "Stopping cluster node"
        );

        // Stop components in reverse order
        if let Some(replicator) = &self.replicator {
            replicator.stop().await?;
        }

        self.discovery.stop().await?;
        self.membership.stop().await?;
        self.health_checker.stop().await?;
        self.gossip.stop().await?;

        tracing::info!(
            node_id = %self.config.node_id,
            "Cluster node stopped"
        );

        Ok(())
    }

    /// Join an existing cluster
    pub async fn join_cluster(&self) -> ClusterResult<()> {
        for seed_addr in &self.config.seed_nodes {
            match self.gossip.join_node(*seed_addr).await {
                Ok(_) => {
                    tracing::info!(
                        seed_addr = %seed_addr,
                        "Successfully joined cluster via seed node"
                    );
                    return Ok(());
                }
                Err(e) => {
                    tracing::warn!(
                        seed_addr = %seed_addr,
                        error = %e,
                        "Failed to join via seed node, trying next"
                    );
                }
            }
        }

        Err(ClusterError::JoinFailed {
            message: "Failed to join cluster via any seed node".to_string(),
        })
    }

    /// Leave the cluster gracefully
    pub async fn leave_cluster(&self) -> ClusterResult<()> {
        self.membership.leave().await?;
        Ok(())
    }

    /// Get cluster state
    pub async fn get_state(&self) -> ClusterState {
        self.state.read().await.clone()
    }

    /// Get local node information
    pub async fn get_local_node(&self) -> Node {
        self.local_node.read().await.clone()
    }

    /// Get all cluster members
    pub async fn get_members(&self) -> Vec<Node> {
        self.membership.get_members().await
    }

    /// Register a service
    pub async fn register_service(&self, service: ServiceInfo) -> ClusterResult<()> {
        self.discovery.register_service(service).await
    }

    /// Unregister a service
    pub async fn unregister_service(&self, service_id: &str) -> ClusterResult<()> {
        self.discovery.unregister_service(service_id).await
    }

    /// Discover services by name
    pub async fn discover_services(&self, service_name: &str) -> ClusterResult<Vec<ServiceInfo>> {
        self.discovery.discover_services(service_name).await
    }

    /// Get cluster health
    pub async fn get_cluster_health(&self) -> ClusterResult<ClusterHealth> {
        let members = self.get_members().await;
        let healthy_count = self.health_checker.get_healthy_nodes().await?.len();

        Ok(ClusterHealth {
            total_nodes: members.len(),
            healthy_nodes: healthy_count,
            unhealthy_nodes: members.len() - healthy_count,
            cluster_state: self.get_state().await,
            last_updated: chrono::Utc::now(),
        })
    }
}

/// Cluster health information
#[derive(Debug, Clone, serde::Serialize)]
pub struct ClusterHealth {
    /// Total number of nodes in cluster
    pub total_nodes: usize,
    /// Number of healthy nodes
    pub healthy_nodes: usize,
    /// Number of unhealthy nodes
    pub unhealthy_nodes: usize,
    /// Current cluster state
    pub cluster_state: ClusterState,
    /// Last health check timestamp
    pub last_updated: chrono::DateTime<chrono::Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{TcpListener, SocketAddr};

    /// 获取一个可用的端口
    fn get_available_port() -> u16 {
        TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    /// 创建测试用的集群配置，使用动态端口
    fn create_test_config() -> ClusterConfig {
        use std::sync::atomic::{AtomicU16, Ordering};
        static PORT_COUNTER: AtomicU16 = AtomicU16::new(50000);

        // Use a unique port for each test to avoid conflicts
        let port = PORT_COUNTER.fetch_add(1, Ordering::SeqCst);
        let bind_addr: SocketAddr = format!("127.0.0.1:{}", port).parse().unwrap();

        ClusterConfig {
            bind_addr,
            ..ClusterConfig::default()
        }
    }

    #[tokio::test]
    async fn test_cluster_creation() {
        let config = create_test_config();
        let cluster = Cluster::new(config).await;
        assert!(cluster.is_ok());
    }

    #[tokio::test]
    async fn test_cluster_state() {
        let config = create_test_config();
        let cluster = Cluster::new(config).await.unwrap();
        let state = cluster.get_state().await;

        assert_eq!(state.total_nodes, 1);
        assert_eq!(state.healthy_nodes, 1);
        assert!(state.leader.is_none());
    }

    #[tokio::test]
    async fn test_local_node() {
        let config = create_test_config();
        let cluster = Cluster::new(config.clone()).await.unwrap();
        let node = cluster.get_local_node().await;

        assert_eq!(node.id(), &config.node_id);
        assert_eq!(node.bind_addr(), config.bind_addr);
    }

    #[tokio::test]
    async fn test_cluster_basic_functionality() {
        let config = create_test_config();
        let cluster = Cluster::new(config).await.unwrap();

        // Test that we can get cluster state without starting
        let state = cluster.get_state().await;
        assert_eq!(state.total_nodes, 1);
        assert_eq!(state.healthy_nodes, 1);

        // Test that we can get local node info
        let local_node = cluster.get_local_node().await;
        assert_eq!(local_node.id(), &cluster.config.node_id);

        // Test that we can get members (should be just local node)
        let members = cluster.get_members().await;
        assert_eq!(members.len(), 1);
        assert_eq!(members[0].id(), &cluster.config.node_id);
    }

    #[tokio::test]
    async fn test_cluster_configuration() {
        // Test cluster configuration with seed nodes
        let config1 = create_test_config();
        let config2 = {
            let mut config = create_test_config();
            config.seed_nodes = vec![config1.bind_addr];
            config
        };

        // Create clusters without starting them
        let cluster1 = Cluster::new(config1.clone()).await.unwrap();
        let cluster2 = Cluster::new(config2.clone()).await.unwrap();

        // Verify configurations
        assert_eq!(cluster1.config.node_id, config1.node_id);
        assert_eq!(cluster1.config.bind_addr, config1.bind_addr);
        assert!(cluster1.config.seed_nodes.is_empty());

        assert_eq!(cluster2.config.node_id, config2.node_id);
        assert_eq!(cluster2.config.bind_addr, config2.bind_addr);
        assert_eq!(cluster2.config.seed_nodes.len(), 1);
        assert_eq!(cluster2.config.seed_nodes[0], config1.bind_addr);

        // Both clusters should have valid initial state
        let state1 = cluster1.get_state().await;
        let state2 = cluster2.get_state().await;

        assert_eq!(state1.total_nodes, 1);
        assert_eq!(state1.healthy_nodes, 1);
        assert_eq!(state2.total_nodes, 1);
        assert_eq!(state2.healthy_nodes, 1);
    }
}
