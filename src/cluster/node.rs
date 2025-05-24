//! Cluster Node Management
//!
//! This module defines the core node abstraction for the cluster,
//! including node identity, state management, and metadata handling.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Unique identifier for a cluster node
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NodeId(String);

impl NodeId {
    /// Generate a new random node ID
    pub fn generate() -> Self {
        Self(Uuid::new_v4().to_string())
    }

    /// Create a node ID from a string
    pub fn from_string<S: Into<String>>(id: S) -> Self {
        Self(id.into())
    }

    /// Get the string representation
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for NodeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<String> for NodeId {
    fn from(id: String) -> Self {
        Self(id)
    }
}

impl From<&str> for NodeId {
    fn from(id: &str) -> Self {
        Self(id.to_string())
    }
}

/// Node state in the cluster
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeState {
    /// Node is alive and healthy
    Alive,
    /// Node is suspected to be down
    Suspect,
    /// Node is confirmed dead
    Dead,
    /// Node has left the cluster gracefully
    Left,
}

impl std::fmt::Display for NodeState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NodeState::Alive => write!(f, "alive"),
            NodeState::Suspect => write!(f, "suspect"),
            NodeState::Dead => write!(f, "dead"),
            NodeState::Left => write!(f, "left"),
        }
    }
}

/// Node information structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeInfo {
    /// Unique node identifier
    pub id: NodeId,
    /// Node name (human-readable)
    pub name: String,
    /// Address for cluster communication
    pub addr: SocketAddr,
    /// Current node state
    pub state: NodeState,
    /// Node metadata (tags, labels, etc.)
    pub metadata: HashMap<String, String>,
    /// Protocol version
    pub protocol_version: u32,
    /// Node startup timestamp
    pub started_at: chrono::DateTime<chrono::Utc>,
    /// Last seen timestamp
    pub last_seen: chrono::DateTime<chrono::Utc>,
    /// Incarnation number (for conflict resolution)
    pub incarnation: u64,
}

impl NodeInfo {
    /// Create a new node info
    pub fn new(id: NodeId, addr: SocketAddr) -> Self {
        let now = chrono::Utc::now();
        Self {
            name: format!("node-{}", &id.as_str()[..8]),
            id,
            addr,
            state: NodeState::Alive,
            metadata: HashMap::new(),
            protocol_version: 1,
            started_at: now,
            last_seen: now,
            incarnation: 0,
        }
    }

    /// Update the last seen timestamp
    pub fn update_last_seen(&mut self) {
        self.last_seen = chrono::Utc::now();
    }

    /// Increment incarnation number
    pub fn increment_incarnation(&mut self) {
        self.incarnation += 1;
    }

    /// Check if node is healthy
    pub fn is_healthy(&self) -> bool {
        matches!(self.state, NodeState::Alive)
    }

    /// Check if node is available
    pub fn is_available(&self) -> bool {
        matches!(self.state, NodeState::Alive | NodeState::Suspect)
    }

    /// Get age since startup
    pub fn age(&self) -> chrono::Duration {
        chrono::Utc::now() - self.started_at
    }

    /// Get time since last seen
    pub fn time_since_last_seen(&self) -> chrono::Duration {
        chrono::Utc::now() - self.last_seen
    }
}

/// Main node structure
#[derive(Debug)]
pub struct Node {
    /// Node information
    info: NodeInfo,
    /// Local sequence number for ordering
    sequence: AtomicU64,
}

impl Clone for Node {
    fn clone(&self) -> Self {
        Self {
            info: self.info.clone(),
            sequence: AtomicU64::new(self.sequence.load(std::sync::atomic::Ordering::SeqCst)),
        }
    }
}

impl Node {
    /// Create a new node
    pub fn new(id: NodeId, bind_addr: SocketAddr, advertise_addr: SocketAddr) -> Self {
        let mut info = NodeInfo::new(id, advertise_addr);
        info.metadata.insert("bind_addr".to_string(), bind_addr.to_string());

        Self {
            info,
            sequence: AtomicU64::new(0),
        }
    }

    /// Get node ID
    pub fn id(&self) -> &NodeId {
        &self.info.id
    }

    /// Get node name
    pub fn name(&self) -> &str {
        &self.info.name
    }

    /// Set node name
    pub fn set_name<S: Into<String>>(&mut self, name: S) {
        self.info.name = name.into();
    }

    /// Get node address
    pub fn addr(&self) -> SocketAddr {
        self.info.addr
    }

    /// Get bind address
    pub fn bind_addr(&self) -> SocketAddr {
        self.info.metadata
            .get("bind_addr")
            .and_then(|s| s.parse().ok())
            .unwrap_or(self.info.addr)
    }

    /// Get node state
    pub fn state(&self) -> NodeState {
        self.info.state
    }

    /// Set node state
    pub fn set_state(&mut self, state: NodeState) {
        self.info.state = state;
        self.info.update_last_seen();
    }

    /// Get node metadata
    pub fn metadata(&self) -> &HashMap<String, String> {
        &self.info.metadata
    }

    /// Add metadata
    pub fn add_metadata<K, V>(&mut self, key: K, value: V)
    where
        K: Into<String>,
        V: Into<String>,
    {
        self.info.metadata.insert(key.into(), value.into());
    }

    /// Remove metadata
    pub fn remove_metadata(&mut self, key: &str) -> Option<String> {
        self.info.metadata.remove(key)
    }

    /// Get protocol version
    pub fn protocol_version(&self) -> u32 {
        self.info.protocol_version
    }

    /// Get incarnation number
    pub fn incarnation(&self) -> u64 {
        self.info.incarnation
    }

    /// Increment incarnation
    pub fn increment_incarnation(&mut self) {
        self.info.increment_incarnation();
    }

    /// Get next sequence number
    pub fn next_sequence(&self) -> u64 {
        self.sequence.fetch_add(1, Ordering::SeqCst)
    }

    /// Get current sequence number
    pub fn current_sequence(&self) -> u64 {
        self.sequence.load(Ordering::SeqCst)
    }

    /// Update last seen timestamp
    pub fn update_last_seen(&mut self) {
        self.info.update_last_seen();
    }

    /// Check if node is healthy
    pub fn is_healthy(&self) -> bool {
        self.info.is_healthy()
    }

    /// Check if node is available
    pub fn is_available(&self) -> bool {
        self.info.is_available()
    }

    /// Get node info (immutable reference)
    pub fn info(&self) -> &NodeInfo {
        &self.info
    }

    /// Get node info (mutable reference)
    pub fn info_mut(&mut self) -> &mut NodeInfo {
        &mut self.info
    }

    /// Convert to node info
    pub fn into_info(self) -> NodeInfo {
        self.info
    }

    /// Create from node info
    pub fn from_info(info: NodeInfo) -> Self {
        Self {
            info,
            sequence: AtomicU64::new(0),
        }
    }

    /// Check if this node is newer than another (for conflict resolution)
    pub fn is_newer_than(&self, other: &Node) -> bool {
        self.info.incarnation > other.info.incarnation
    }

    /// Merge with another node (keeping the newer information)
    pub fn merge(&mut self, other: &Node) {
        if other.is_newer_than(self) {
            self.info = other.info.clone();
        }
    }
}

impl PartialEq for Node {
    fn eq(&self, other: &Self) -> bool {
        self.info.id == other.info.id
    }
}

impl Eq for Node {}

impl std::hash::Hash for Node {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.info.id.hash(state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_node_id_generation() {
        let id1 = NodeId::generate();
        let id2 = NodeId::generate();
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_node_id_from_string() {
        let id = NodeId::from_string("test-node-123");
        assert_eq!(id.as_str(), "test-node-123");
        assert_eq!(id.to_string(), "test-node-123");
    }

    #[test]
    fn test_node_creation() {
        let id = NodeId::generate();
        let bind_addr = "127.0.0.1:7946".parse().unwrap();
        let advertise_addr = "10.0.0.1:7946".parse().unwrap();

        let node = Node::new(id.clone(), bind_addr, advertise_addr);

        assert_eq!(node.id(), &id);
        assert_eq!(node.addr(), advertise_addr);
        assert_eq!(node.bind_addr(), bind_addr);
        assert_eq!(node.state(), NodeState::Alive);
        assert!(node.is_healthy());
        assert!(node.is_available());
    }

    #[test]
    fn test_node_state_transitions() {
        let id = NodeId::generate();
        let addr = "127.0.0.1:7946".parse().unwrap();
        let mut node = Node::new(id, addr, addr);

        assert_eq!(node.state(), NodeState::Alive);

        node.set_state(NodeState::Suspect);
        assert_eq!(node.state(), NodeState::Suspect);
        assert!(!node.is_healthy());
        assert!(node.is_available());

        node.set_state(NodeState::Dead);
        assert_eq!(node.state(), NodeState::Dead);
        assert!(!node.is_healthy());
        assert!(!node.is_available());
    }

    #[test]
    fn test_node_metadata() {
        let id = NodeId::generate();
        let addr = "127.0.0.1:7946".parse().unwrap();
        let mut node = Node::new(id, addr, addr);

        node.add_metadata("datacenter", "us-west-1");
        node.add_metadata("rack", "rack-1");

        assert_eq!(node.metadata().get("datacenter"), Some(&"us-west-1".to_string()));
        assert_eq!(node.metadata().get("rack"), Some(&"rack-1".to_string()));

        let removed = node.remove_metadata("rack");
        assert_eq!(removed, Some("rack-1".to_string()));
        assert!(node.metadata().get("rack").is_none());
    }

    #[test]
    fn test_node_sequence() {
        let id = NodeId::generate();
        let addr = "127.0.0.1:7946".parse().unwrap();
        let node = Node::new(id, addr, addr);

        assert_eq!(node.current_sequence(), 0);
        assert_eq!(node.next_sequence(), 0);
        assert_eq!(node.current_sequence(), 1);
        assert_eq!(node.next_sequence(), 1);
        assert_eq!(node.current_sequence(), 2);
    }

    #[test]
    fn test_node_incarnation() {
        let id = NodeId::generate();
        let addr = "127.0.0.1:7946".parse().unwrap();
        let mut node = Node::new(id, addr, addr);

        assert_eq!(node.incarnation(), 0);
        node.increment_incarnation();
        assert_eq!(node.incarnation(), 1);
    }

    #[test]
    fn test_node_comparison() {
        let id = NodeId::generate();
        let addr = "127.0.0.1:7946".parse().unwrap();
        let mut node1 = Node::new(id.clone(), addr, addr);
        let mut node2 = Node::new(id, addr, addr);

        assert_eq!(node1, node2);
        assert!(!node1.is_newer_than(&node2));

        node2.increment_incarnation();
        assert!(node2.is_newer_than(&node1));

        node1.merge(&node2);
        assert_eq!(node1.incarnation(), 1);
    }
}
