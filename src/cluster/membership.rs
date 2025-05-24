//! Cluster Membership Management
//!
//! This module handles cluster membership, including member discovery,
//! failure detection, and membership change notifications.

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{RwLock, mpsc, broadcast};
use tokio::time::{interval, Duration};

use crate::cluster::{Node, NodeId, NodeInfo, NodeState, ClusterError, ClusterResult};

/// Membership events
#[derive(Debug, Clone)]
pub enum MembershipEvent {
    /// A node joined the cluster
    NodeJoined(NodeInfo),
    /// A node left the cluster
    NodeLeft(NodeInfo),
    /// A node failed
    NodeFailed(NodeInfo),
    /// A node recovered
    NodeRecovered(NodeInfo),
    /// A node was updated
    NodeUpdated(NodeInfo),
}

/// Cluster membership manager
pub struct ClusterMembership {
    /// Local node
    local_node: Arc<RwLock<Node>>,
    /// Current cluster members
    members: Arc<RwLock<HashMap<NodeId, NodeInfo>>>,
    /// Event broadcaster
    event_tx: broadcast::Sender<MembershipEvent>,
    /// Shutdown signal
    shutdown_tx: Arc<RwLock<Option<mpsc::Sender<()>>>>,
    /// Running state
    running: Arc<std::sync::atomic::AtomicBool>,
}

impl ClusterMembership {
    /// Create a new membership manager
    pub async fn new(local_node: Arc<RwLock<Node>>) -> ClusterResult<Self> {
        let (event_tx, _) = broadcast::channel(1000);

        let mut members = HashMap::new();
        {
            let node = local_node.read().await;
            members.insert(node.id().clone(), node.info().clone());
        }

        Ok(Self {
            local_node,
            members: Arc::new(RwLock::new(members)),
            event_tx,
            shutdown_tx: Arc::new(RwLock::new(None)),
            running: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        })
    }

    /// Start the membership manager
    pub async fn start(&self) -> ClusterResult<()> {
        if self.running.load(std::sync::atomic::Ordering::SeqCst) {
            return Ok(());
        }

        let (shutdown_tx, mut shutdown_rx) = mpsc::channel(1);
        *self.shutdown_tx.write().await = Some(shutdown_tx);
        self.running.store(true, std::sync::atomic::Ordering::SeqCst);

        // Start membership maintenance task
        let maintenance_task = {
            let members = self.members.clone();
            let event_tx = self.event_tx.clone();
            let running = self.running.clone();

            tokio::spawn(async move {
                let mut interval = interval(Duration::from_secs(5));

                while running.load(std::sync::atomic::Ordering::SeqCst) {
                    tokio::select! {
                        _ = interval.tick() => {
                            if let Err(e) = Self::maintenance_round(&members, &event_tx).await {
                                tracing::error!("Membership maintenance failed: {}", e);
                            }
                        }
                        _ = shutdown_rx.recv() => {
                            break;
                        }
                    }
                }
            })
        };

        tracing::info!("Cluster membership manager started");
        Ok(())
    }

    /// Stop the membership manager
    pub async fn stop(&self) -> ClusterResult<()> {
        if !self.running.load(std::sync::atomic::Ordering::SeqCst) {
            return Ok(());
        }

        self.running.store(false, std::sync::atomic::Ordering::SeqCst);

        if let Some(shutdown_tx) = self.shutdown_tx.write().await.take() {
            let _ = shutdown_tx.send(()).await;
        }

        tracing::info!("Cluster membership manager stopped");
        Ok(())
    }

    /// Add a member to the cluster
    pub async fn add_member(&self, node: NodeInfo) -> ClusterResult<()> {
        let mut members = self.members.write().await;
        let is_new = !members.contains_key(&node.id);

        members.insert(node.id.clone(), node.clone());

        if is_new {
            let _ = self.event_tx.send(MembershipEvent::NodeJoined(node));
        } else {
            let _ = self.event_tx.send(MembershipEvent::NodeUpdated(node));
        }

        Ok(())
    }

    /// Remove a member from the cluster
    pub async fn remove_member(&self, node_id: &NodeId) -> ClusterResult<()> {
        let mut members = self.members.write().await;

        if let Some(node) = members.remove(node_id) {
            let _ = self.event_tx.send(MembershipEvent::NodeLeft(node));
        }

        Ok(())
    }

    /// Mark a member as failed
    pub async fn mark_failed(&self, node_id: &NodeId) -> ClusterResult<()> {
        let mut members = self.members.write().await;

        if let Some(node) = members.get_mut(node_id) {
            if node.state != NodeState::Dead {
                node.state = NodeState::Dead;
                let _ = self.event_tx.send(MembershipEvent::NodeFailed(node.clone()));
            }
        }

        Ok(())
    }

    /// Mark a member as recovered
    pub async fn mark_recovered(&self, node_id: &NodeId) -> ClusterResult<()> {
        let mut members = self.members.write().await;

        if let Some(node) = members.get_mut(node_id) {
            if node.state != NodeState::Alive {
                node.state = NodeState::Alive;
                node.last_seen = chrono::Utc::now();
                let _ = self.event_tx.send(MembershipEvent::NodeRecovered(node.clone()));
            }
        }

        Ok(())
    }

    /// Get all cluster members
    pub async fn get_members(&self) -> Vec<Node> {
        let members = self.members.read().await;
        members.values().map(|info| Node::from_info(info.clone())).collect()
    }

    /// Get healthy members
    pub async fn get_healthy_members(&self) -> Vec<Node> {
        let members = self.members.read().await;
        members
            .values()
            .filter(|info| info.is_healthy())
            .map(|info| Node::from_info(info.clone()))
            .collect()
    }

    /// Get member by ID
    pub async fn get_member(&self, node_id: &NodeId) -> Option<Node> {
        let members = self.members.read().await;
        members.get(node_id).map(|info| Node::from_info(info.clone()))
    }

    /// Get member count
    pub async fn member_count(&self) -> usize {
        let members = self.members.read().await;
        members.len()
    }

    /// Get healthy member count
    pub async fn healthy_member_count(&self) -> usize {
        let members = self.members.read().await;
        members.values().filter(|info| info.is_healthy()).count()
    }

    /// Subscribe to membership events
    pub fn subscribe(&self) -> broadcast::Receiver<MembershipEvent> {
        self.event_tx.subscribe()
    }

    /// Leave the cluster gracefully
    pub async fn leave(&self) -> ClusterResult<()> {
        let local_node_id = {
            let node = self.local_node.read().await;
            node.id().clone()
        };

        self.remove_member(&local_node_id).await?;
        Ok(())
    }

    /// Update local node information
    pub async fn update_local_node(&self) -> ClusterResult<()> {
        let node_info = {
            let node = self.local_node.read().await;
            node.info().clone()
        };

        self.add_member(node_info).await?;
        Ok(())
    }

    /// Perform maintenance round
    async fn maintenance_round(
        members: &Arc<RwLock<HashMap<NodeId, NodeInfo>>>,
        event_tx: &broadcast::Sender<MembershipEvent>,
    ) -> ClusterResult<()> {
        let now = chrono::Utc::now();
        let mut to_remove = Vec::new();

        {
            let members_guard = members.read().await;
            for (node_id, node_info) in members_guard.iter() {
                // Check if node has been inactive for too long
                let inactive_duration = now - node_info.last_seen;

                if inactive_duration > chrono::Duration::seconds(60) && node_info.state == NodeState::Alive {
                    // Mark as suspect first
                    // In a real implementation, this would trigger failure detection
                    tracing::warn!("Node {} has been inactive for {:?}", node_id, inactive_duration);
                }

                if inactive_duration > chrono::Duration::seconds(300) {
                    // Remove nodes that have been inactive for 5 minutes
                    to_remove.push((node_id.clone(), node_info.clone()));
                }
            }
        }

        // Remove inactive nodes
        if !to_remove.is_empty() {
            let mut members_guard = members.write().await;
            for (node_id, node_info) in to_remove {
                members_guard.remove(&node_id);
                let _ = event_tx.send(MembershipEvent::NodeLeft(node_info));
                tracing::info!("Removed inactive node: {}", node_id);
            }
        }

        Ok(())
    }
}

/// Membership statistics
#[derive(Debug, Clone, serde::Serialize)]
pub struct MembershipStats {
    /// Total number of members
    pub total_members: usize,
    /// Number of healthy members
    pub healthy_members: usize,
    /// Number of suspect members
    pub suspect_members: usize,
    /// Number of dead members
    pub dead_members: usize,
    /// Membership generation/version
    pub generation: u64,
    /// Last update timestamp
    pub last_updated: chrono::DateTime<chrono::Utc>,
}

impl ClusterMembership {
    /// Get membership statistics
    pub async fn get_stats(&self) -> MembershipStats {
        let members = self.members.read().await;
        let mut healthy = 0;
        let mut suspect = 0;
        let mut dead = 0;

        for member in members.values() {
            match member.state {
                NodeState::Alive => healthy += 1,
                NodeState::Suspect => suspect += 1,
                NodeState::Dead => dead += 1,
                NodeState::Left => {} // Don't count left nodes
            }
        }

        MembershipStats {
            total_members: members.len(),
            healthy_members: healthy,
            suspect_members: suspect,
            dead_members: dead,
            generation: 1, // Placeholder
            last_updated: chrono::Utc::now(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cluster::NodeId;

    #[tokio::test]
    async fn test_membership_creation() {
        let node_id = NodeId::generate();
        let addr = "127.0.0.1:7946".parse().unwrap();
        let node = Arc::new(RwLock::new(Node::new(node_id.clone(), addr, addr)));

        let membership = ClusterMembership::new(node).await;
        assert!(membership.is_ok());

        let membership = membership.unwrap();
        assert_eq!(membership.member_count().await, 1);
    }

    #[tokio::test]
    async fn test_add_remove_member() {
        let node_id = NodeId::generate();
        let addr = "127.0.0.1:7946".parse().unwrap();
        let node = Arc::new(RwLock::new(Node::new(node_id.clone(), addr, addr)));

        let membership = ClusterMembership::new(node).await.unwrap();

        // Add a new member
        let new_node_id = NodeId::generate();
        let new_addr = "127.0.0.1:7947".parse().unwrap();
        let new_node_info = NodeInfo::new(new_node_id.clone(), new_addr);

        membership.add_member(new_node_info).await.unwrap();
        assert_eq!(membership.member_count().await, 2);

        // Remove the member
        membership.remove_member(&new_node_id).await.unwrap();
        assert_eq!(membership.member_count().await, 1);
    }

    #[tokio::test]
    async fn test_membership_events() {
        let node_id = NodeId::generate();
        let addr = "127.0.0.1:7946".parse().unwrap();
        let node = Arc::new(RwLock::new(Node::new(node_id.clone(), addr, addr)));

        let membership = ClusterMembership::new(node).await.unwrap();
        let mut event_rx = membership.subscribe();

        // Add a new member
        let new_node_id = NodeId::generate();
        let new_addr = "127.0.0.1:7947".parse().unwrap();
        let new_node_info = NodeInfo::new(new_node_id.clone(), new_addr);

        membership.add_member(new_node_info).await.unwrap();

        // Check for event
        let event = event_rx.recv().await.unwrap();
        match event {
            MembershipEvent::NodeJoined(node_info) => {
                assert_eq!(node_info.id, new_node_id);
            }
            _ => panic!("Expected NodeJoined event"),
        }
    }

    #[tokio::test]
    async fn test_membership_stats() {
        let node_id = NodeId::generate();
        let addr = "127.0.0.1:7946".parse().unwrap();
        let node = Arc::new(RwLock::new(Node::new(node_id.clone(), addr, addr)));

        let membership = ClusterMembership::new(node).await.unwrap();
        let stats = membership.get_stats().await;

        assert_eq!(stats.total_members, 1);
        assert_eq!(stats.healthy_members, 1);
        assert_eq!(stats.suspect_members, 0);
        assert_eq!(stats.dead_members, 0);
    }
}
