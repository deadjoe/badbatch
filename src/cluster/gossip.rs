//! Gossip Protocol Implementation
//!
//! This module implements the gossip protocol for efficient information dissemination
//! across the cluster. It follows the SWIM (Scalable Weakly-consistent Infection-style
//! Process Group Membership) protocol with optimizations for performance and reliability.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, RwLock};
use tokio::time::interval;

use crate::cluster::{ClusterError, ClusterResult, Node, NodeId, NodeInfo};

/// Gossip protocol configuration
#[derive(Debug, Clone)]
pub struct GossipConfig {
    /// Interval between gossip rounds
    pub interval: Duration,
    /// Number of nodes to gossip to in each round
    pub fanout: usize,
    /// Maximum size of gossip packets
    pub max_packet_size: usize,
    /// Enable compression for gossip messages
    pub enable_compression: bool,
    /// Enable encryption for gossip messages
    pub enable_encryption: bool,
    /// Encryption key
    pub encryption_key: Option<Vec<u8>>,
}

/// Types of gossip messages
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GossipMessage {
    /// Ping message for failure detection
    Ping {
        from: NodeId,
        sequence: u64,
        timestamp: u64,
    },
    /// Ack response to ping
    Ack {
        from: NodeId,
        to: NodeId,
        sequence: u64,
        timestamp: u64,
    },
    /// Indirect ping request
    IndirectPing {
        from: NodeId,
        target: NodeId,
        sequence: u64,
    },
    /// Node state update
    StateUpdate { node: NodeInfo, incarnation: u64 },
    /// Join request
    JoinRequest { node: NodeInfo },
    /// Join response
    JoinResponse {
        nodes: Vec<NodeInfo>,
        accepted: bool,
    },
    /// Leave notification
    Leave { node_id: NodeId, incarnation: u64 },
    /// Suspect notification
    Suspect {
        node_id: NodeId,
        incarnation: u64,
        from: NodeId,
    },
    /// Alive notification (refutes suspect)
    Alive { node_id: NodeId, incarnation: u64 },
    /// Dead notification
    Dead {
        node_id: NodeId,
        incarnation: u64,
        from: NodeId,
    },
}

/// Gossip protocol handler
pub struct GossipProtocol {
    /// Configuration
    config: GossipConfig,
    /// Local node
    local_node: Arc<RwLock<Node>>,
    /// UDP socket for communication
    socket: Arc<UdpSocket>,
    /// Known nodes in the cluster
    nodes: Arc<RwLock<HashMap<NodeId, NodeInfo>>>,
    /// Pending acks
    pending_acks: Arc<RwLock<HashMap<u64, PendingAck>>>,
    /// Message sequence counter
    sequence_counter: Arc<std::sync::atomic::AtomicU64>,
    /// Shutdown signal
    shutdown_tx: Arc<RwLock<Option<mpsc::Sender<()>>>>,
    /// Running state
    running: Arc<std::sync::atomic::AtomicBool>,
    /// Background task handles
    task_handles: Arc<RwLock<Vec<tokio::task::JoinHandle<()>>>>,
}

/// Pending acknowledgment
#[derive(Debug)]
#[allow(dead_code)]
struct PendingAck {
    target: NodeId,
    sent_at: Instant,
    timeout: Duration,
}

impl GossipProtocol {
    /// Create a new gossip protocol instance
    pub async fn new(config: GossipConfig, local_node: Arc<RwLock<Node>>) -> ClusterResult<Self> {
        let bind_addr = {
            let node = local_node.read().await;
            node.bind_addr()
        };

        let socket = UdpSocket::bind(bind_addr)
            .await
            .map_err(|e| ClusterError::BindError {
                addr: bind_addr,
                source: e,
            })?;

        Ok(Self {
            config,
            local_node,
            socket: Arc::new(socket),
            nodes: Arc::new(RwLock::new(HashMap::new())),
            pending_acks: Arc::new(RwLock::new(HashMap::new())),
            sequence_counter: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            shutdown_tx: Arc::new(RwLock::new(None)),
            running: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            task_handles: Arc::new(RwLock::new(Vec::new())),
        })
    }

    /// Start the gossip protocol
    pub async fn start(&self) -> ClusterResult<()> {
        if self.running.load(std::sync::atomic::Ordering::SeqCst) {
            return Ok(());
        }

        let (shutdown_tx, mut shutdown_rx) = mpsc::channel(1);
        *self.shutdown_tx.write().await = Some(shutdown_tx);
        self.running
            .store(true, std::sync::atomic::Ordering::SeqCst);

        // Start gossip timer
        let gossip_task = {
            let config = self.config.clone();
            let local_node = self.local_node.clone();
            let nodes = self.nodes.clone();
            let socket = self.socket.clone();
            let sequence_counter = self.sequence_counter.clone();
            let running = self.running.clone();

            tokio::spawn(async move {
                let mut interval = interval(config.interval);

                while running.load(std::sync::atomic::Ordering::SeqCst) {
                    tokio::select! {
                        _ = interval.tick() => {
                            if let Err(e) = Self::gossip_round(
                                &config,
                                &local_node,
                                &nodes,
                                &socket,
                                &sequence_counter,
                            ).await {
                                tracing::error!("Gossip round failed: {}", e);
                            }
                        }
                        _ = shutdown_rx.recv() => {
                            break;
                        }
                    }
                }
            })
        };

        // Start message receiver
        let receiver_task = {
            let socket = self.socket.clone();
            let local_node = self.local_node.clone();
            let nodes = self.nodes.clone();
            let pending_acks = self.pending_acks.clone();
            let running = self.running.clone();

            tokio::spawn(async move {
                let mut buffer = vec![0u8; 65536];

                while running.load(std::sync::atomic::Ordering::SeqCst) {
                    match socket.recv_from(&mut buffer).await {
                        Ok((size, addr)) => {
                            if let Err(e) = Self::handle_message(
                                &buffer[..size],
                                addr,
                                &local_node,
                                &nodes,
                                &pending_acks,
                                &socket,
                            )
                            .await
                            {
                                tracing::warn!("Failed to handle message from {}: {}", addr, e);
                            }
                        }
                        Err(e) => {
                            tracing::error!("Failed to receive gossip message: {}", e);
                        }
                    }
                }
            })
        };

        // Store task handles for proper cleanup
        {
            let mut handles = self.task_handles.write().await;
            handles.push(gossip_task);
            handles.push(receiver_task);
        }

        tracing::info!("Gossip protocol started");
        Ok(())
    }

    /// Stop the gossip protocol
    pub async fn stop(&self) -> ClusterResult<()> {
        if !self.running.load(std::sync::atomic::Ordering::SeqCst) {
            return Ok(());
        }

        self.running
            .store(false, std::sync::atomic::Ordering::SeqCst);

        // Send shutdown signal
        if let Some(shutdown_tx) = self.shutdown_tx.write().await.take() {
            let _ = shutdown_tx.send(()).await;
        }

        // Wait for all background tasks to complete
        let mut handles = self.task_handles.write().await;
        while let Some(handle) = handles.pop() {
            if let Err(e) = handle.await {
                tracing::warn!("Background task failed to complete cleanly: {}", e);
            }
        }

        tracing::info!("Gossip protocol stopped");
        Ok(())
    }

    /// Join a node
    pub async fn join_node(&self, addr: SocketAddr) -> ClusterResult<()> {
        let local_node = self.local_node.read().await;
        let join_request = GossipMessage::JoinRequest {
            node: local_node.info().clone(),
        };

        self.send_message(&join_request, addr).await?;
        Ok(())
    }

    /// Add a node to the known nodes
    pub async fn add_node(&self, node: NodeInfo) {
        let mut nodes = self.nodes.write().await;
        nodes.insert(node.id.clone(), node);
    }

    /// Remove a node from known nodes
    pub async fn remove_node(&self, node_id: &NodeId) {
        let mut nodes = self.nodes.write().await;
        nodes.remove(node_id);
    }

    /// Get all known nodes
    pub async fn get_nodes(&self) -> Vec<NodeInfo> {
        let nodes = self.nodes.read().await;
        nodes.values().cloned().collect()
    }

    /// Perform a gossip round
    async fn gossip_round(
        config: &GossipConfig,
        local_node: &Arc<RwLock<Node>>,
        nodes: &Arc<RwLock<HashMap<NodeId, NodeInfo>>>,
        socket: &UdpSocket,
        sequence_counter: &std::sync::atomic::AtomicU64,
    ) -> ClusterResult<()> {
        let nodes_snapshot = {
            let nodes = nodes.read().await;
            nodes.values().cloned().collect::<Vec<_>>()
        };

        if nodes_snapshot.is_empty() {
            return Ok(());
        }

        // Select random nodes to gossip to
        let mut targets = Vec::new();
        let fanout = config.fanout.min(nodes_snapshot.len());

        // Simple random selection (could be improved with better algorithms)
        for _ in 0..fanout {
            if let Some(node) = nodes_snapshot.get(rand::random::<usize>() % nodes_snapshot.len()) {
                if !targets.iter().any(|n: &NodeInfo| n.id == node.id) {
                    targets.push(node.clone());
                }
            }
        }

        // Send ping messages
        for target in targets {
            let sequence = sequence_counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let ping = GossipMessage::Ping {
                from: local_node.read().await.id().clone(),
                sequence,
                timestamp: chrono::Utc::now().timestamp_millis() as u64,
            };

            if let Err(e) = Self::send_message_to(&ping, target.addr, socket).await {
                tracing::warn!("Failed to send ping to {}: {}", target.addr, e);
            }
        }

        Ok(())
    }

    /// Handle incoming message
    async fn handle_message(
        data: &[u8],
        from_addr: SocketAddr,
        local_node: &Arc<RwLock<Node>>,
        nodes: &Arc<RwLock<HashMap<NodeId, NodeInfo>>>,
        pending_acks: &Arc<RwLock<HashMap<u64, PendingAck>>>,
        socket: &UdpSocket,
    ) -> ClusterResult<()> {
        let message: GossipMessage = serde_json::from_slice(data)
            .map_err(|e| ClusterError::protocol(format!("Invalid message format: {}", e)))?;

        match message {
            GossipMessage::Ping {
                from,
                sequence,
                timestamp: _,
            } => {
                let ack = GossipMessage::Ack {
                    from: local_node.read().await.id().clone(),
                    to: from,
                    sequence,
                    timestamp: chrono::Utc::now().timestamp_millis() as u64,
                };
                Self::send_message_to(&ack, from_addr, socket).await?;
            }

            GossipMessage::Ack {
                from: _,
                to: _,
                sequence,
                timestamp: _,
            } => {
                let mut pending = pending_acks.write().await;
                pending.remove(&sequence);
            }

            GossipMessage::JoinRequest { node } => {
                // Add the joining node
                {
                    let mut nodes_map = nodes.write().await;
                    nodes_map.insert(node.id.clone(), node.clone());
                }

                // Send join response with current node list
                let nodes_list = {
                    let nodes_map = nodes.read().await;
                    nodes_map.values().cloned().collect()
                };

                let response = GossipMessage::JoinResponse {
                    nodes: nodes_list,
                    accepted: true,
                };
                Self::send_message_to(&response, from_addr, socket).await?;
            }

            GossipMessage::JoinResponse {
                nodes: node_list,
                accepted,
            } => {
                if accepted {
                    let mut nodes_map = nodes.write().await;
                    for node in node_list {
                        nodes_map.insert(node.id.clone(), node);
                    }
                }
            }

            GossipMessage::StateUpdate {
                node,
                incarnation: _,
            } => {
                let mut nodes_map = nodes.write().await;
                nodes_map.insert(node.id.clone(), node);
            }

            _ => {
                // Handle other message types
                tracing::debug!("Received gossip message: {:?}", message);
            }
        }

        Ok(())
    }

    /// Send a message to a specific address
    async fn send_message_to(
        message: &GossipMessage,
        addr: SocketAddr,
        socket: &UdpSocket,
    ) -> ClusterResult<()> {
        let data = serde_json::to_vec(message)
            .map_err(|e| ClusterError::protocol(format!("Failed to serialize message: {}", e)))?;

        socket
            .send_to(&data, addr)
            .await
            .map_err(|e| ClusterError::network(format!("Failed to send message: {}", e)))?;

        Ok(())
    }

    /// Send a message
    async fn send_message(&self, message: &GossipMessage, addr: SocketAddr) -> ClusterResult<()> {
        Self::send_message_to(message, addr, &self.socket).await
    }
}

// Placeholder for rand functionality
mod rand {
    pub fn random<T>() -> T
    where
        T: From<u8>,
    {
        // Simple placeholder - in real implementation would use proper RNG
        T::from(42)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cluster::NodeId;

    #[test]
    fn test_gossip_config() {
        let config = GossipConfig {
            interval: Duration::from_millis(200),
            fanout: 3,
            max_packet_size: 1400,
            enable_compression: true,
            enable_encryption: false,
            encryption_key: None,
        };

        assert_eq!(config.interval, Duration::from_millis(200));
        assert_eq!(config.fanout, 3);
        assert!(config.enable_compression);
        assert!(!config.enable_encryption);
    }

    #[test]
    fn test_gossip_message_serialization() {
        let ping = GossipMessage::Ping {
            from: NodeId::generate(),
            sequence: 123,
            timestamp: 1234567890,
        };

        let serialized = serde_json::to_vec(&ping).unwrap();
        let deserialized: GossipMessage = serde_json::from_slice(&serialized).unwrap();

        match deserialized {
            GossipMessage::Ping {
                sequence,
                timestamp,
                ..
            } => {
                assert_eq!(sequence, 123);
                assert_eq!(timestamp, 1234567890);
            }
            _ => panic!("Wrong message type"),
        }
    }

    #[tokio::test]
    async fn test_gossip_protocol_creation() {
        let config = GossipConfig {
            interval: Duration::from_millis(200),
            fanout: 3,
            max_packet_size: 1400,
            enable_compression: false,
            enable_encryption: false,
            encryption_key: None,
        };

        let node_id = NodeId::generate();
        let addr = "127.0.0.1:0".parse().unwrap(); // Use port 0 for testing
        let node = Arc::new(RwLock::new(Node::new(node_id, addr, addr)));

        let result = GossipProtocol::new(config, node).await;
        assert!(result.is_ok());
    }
}
