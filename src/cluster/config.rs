//! Cluster Configuration
//!
//! This module defines configuration structures for the distributed cluster,
//! including gossip protocol settings, health check parameters, and replication options.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::Duration;
use serde::{Deserialize, Serialize};

use crate::cluster::{NodeId, GossipConfig};

/// Main cluster configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterConfig {
    /// Unique identifier for this node
    pub node_id: NodeId,
    
    /// Address to bind the cluster communication port
    pub bind_addr: SocketAddr,
    
    /// Address to advertise to other nodes (if different from bind_addr)
    pub advertise_addr: Option<SocketAddr>,
    
    /// List of seed nodes to join when starting
    pub seed_nodes: Vec<SocketAddr>,
    
    /// Interval between gossip rounds
    pub gossip_interval: Duration,
    
    /// Interval between health probes
    pub probe_interval: Duration,
    
    /// Timeout for health probe responses
    pub probe_timeout: Duration,
    
    /// Time before marking a node as suspect
    pub suspect_timeout: Duration,
    
    /// Time before marking a suspect node as dead
    pub dead_timeout: Duration,
    
    /// Maximum size of gossip packets
    pub max_gossip_packet_size: usize,
    
    /// Number of nodes to gossip to in each round
    pub gossip_fanout: usize,
    
    /// Enable compression for gossip messages
    pub enable_compression: bool,
    
    /// Enable encryption for cluster communication
    pub enable_encryption: bool,
    
    /// Encryption key for cluster communication
    pub encryption_key: Option<Vec<u8>>,
    
    /// Node metadata (tags, labels, etc.)
    pub metadata: HashMap<String, String>,
}

impl ClusterConfig {
    /// Create a new cluster configuration with default values
    pub fn new() -> Self {
        Self::default()
    }
    
    /// Set the node ID
    pub fn with_node_id(mut self, node_id: NodeId) -> Self {
        self.node_id = node_id;
        self
    }
    
    /// Set the bind address
    pub fn with_bind_addr(mut self, addr: SocketAddr) -> Self {
        self.bind_addr = addr;
        self
    }
    
    /// Set the advertise address
    pub fn with_advertise_addr(mut self, addr: SocketAddr) -> Self {
        self.advertise_addr = Some(addr);
        self
    }
    
    /// Add a seed node
    pub fn with_seed_node(mut self, addr: SocketAddr) -> Self {
        self.seed_nodes.push(addr);
        self
    }
    
    /// Set multiple seed nodes
    pub fn with_seed_nodes(mut self, addrs: Vec<SocketAddr>) -> Self {
        self.seed_nodes = addrs;
        self
    }
    
    /// Set gossip interval
    pub fn with_gossip_interval(mut self, interval: Duration) -> Self {
        self.gossip_interval = interval;
        self
    }
    
    /// Set probe interval
    pub fn with_probe_interval(mut self, interval: Duration) -> Self {
        self.probe_interval = interval;
        self
    }
    
    /// Enable compression
    pub fn with_compression(mut self, enable: bool) -> Self {
        self.enable_compression = enable;
        self
    }
    
    /// Enable encryption with key
    pub fn with_encryption(mut self, key: Vec<u8>) -> Self {
        self.enable_encryption = true;
        self.encryption_key = Some(key);
        self
    }
    
    /// Add metadata
    pub fn with_metadata<K, V>(mut self, key: K, value: V) -> Self 
    where
        K: Into<String>,
        V: Into<String>,
    {
        self.metadata.insert(key.into(), value.into());
        self
    }
    
    /// Get gossip configuration
    pub fn gossip_config(&self) -> GossipConfig {
        GossipConfig {
            interval: self.gossip_interval,
            fanout: self.gossip_fanout,
            max_packet_size: self.max_gossip_packet_size,
            enable_compression: self.enable_compression,
            enable_encryption: self.enable_encryption,
            encryption_key: self.encryption_key.clone(),
        }
    }
    
    /// Get health check configuration
    pub fn health_config(&self) -> HealthConfig {
        HealthConfig {
            probe_interval: self.probe_interval,
            probe_timeout: self.probe_timeout,
            suspect_timeout: self.suspect_timeout,
            dead_timeout: self.dead_timeout,
        }
    }
    
    /// Get replication configuration
    pub fn replication_config(&self) -> ReplicationConfig {
        ReplicationConfig {
            enabled: self.enable_replication(),
            replication_factor: 3, // Default replication factor
            consistency_level: ConsistencyLevel::Quorum,
            sync_timeout: Duration::from_secs(5),
        }
    }
    
    /// Check if replication is enabled
    pub fn enable_replication(&self) -> bool {
        // Enable replication if we have more than one node or seed nodes
        !self.seed_nodes.is_empty() || self.metadata.contains_key("enable_replication")
    }
    
    /// Validate the configuration
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.gossip_interval.as_millis() < 50 {
            return Err(ConfigError::InvalidValue {
                field: "gossip_interval".to_string(),
                message: "Gossip interval must be at least 50ms".to_string(),
            });
        }
        
        if self.probe_timeout >= self.probe_interval {
            return Err(ConfigError::InvalidValue {
                field: "probe_timeout".to_string(),
                message: "Probe timeout must be less than probe interval".to_string(),
            });
        }
        
        if self.suspect_timeout <= self.probe_timeout {
            return Err(ConfigError::InvalidValue {
                field: "suspect_timeout".to_string(),
                message: "Suspect timeout must be greater than probe timeout".to_string(),
            });
        }
        
        if self.dead_timeout <= self.suspect_timeout {
            return Err(ConfigError::InvalidValue {
                field: "dead_timeout".to_string(),
                message: "Dead timeout must be greater than suspect timeout".to_string(),
            });
        }
        
        if self.gossip_fanout == 0 {
            return Err(ConfigError::InvalidValue {
                field: "gossip_fanout".to_string(),
                message: "Gossip fanout must be greater than 0".to_string(),
            });
        }
        
        if self.max_gossip_packet_size < 512 {
            return Err(ConfigError::InvalidValue {
                field: "max_gossip_packet_size".to_string(),
                message: "Maximum gossip packet size must be at least 512 bytes".to_string(),
            });
        }
        
        if self.enable_encryption && self.encryption_key.is_none() {
            return Err(ConfigError::InvalidValue {
                field: "encryption_key".to_string(),
                message: "Encryption key is required when encryption is enabled".to_string(),
            });
        }
        
        Ok(())
    }
}

/// Health check configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthConfig {
    /// Interval between health probes
    pub probe_interval: Duration,
    /// Timeout for health probe responses
    pub probe_timeout: Duration,
    /// Time before marking a node as suspect
    pub suspect_timeout: Duration,
    /// Time before marking a suspect node as dead
    pub dead_timeout: Duration,
}

/// Replication configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplicationConfig {
    /// Whether replication is enabled
    pub enabled: bool,
    /// Number of replicas to maintain
    pub replication_factor: usize,
    /// Consistency level for operations
    pub consistency_level: ConsistencyLevel,
    /// Timeout for synchronous replication
    pub sync_timeout: Duration,
}

/// Consistency levels for replication
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConsistencyLevel {
    /// Write to any single node
    Any,
    /// Write to at least one node
    One,
    /// Write to a quorum of nodes
    Quorum,
    /// Write to all nodes
    All,
}

/// Configuration errors
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("Invalid configuration value for {field}: {message}")]
    InvalidValue { field: String, message: String },
    
    #[error("Missing required configuration: {field}")]
    MissingRequired { field: String },
    
    #[error("Configuration conflict: {message}")]
    Conflict { message: String },
}

/// Configuration builder for fluent API
pub struct ClusterConfigBuilder {
    config: ClusterConfig,
}

impl ClusterConfigBuilder {
    /// Create a new configuration builder
    pub fn new() -> Self {
        Self {
            config: ClusterConfig::default(),
        }
    }
    
    /// Set node ID
    pub fn node_id(mut self, id: NodeId) -> Self {
        self.config.node_id = id;
        self
    }
    
    /// Set bind address
    pub fn bind_addr(mut self, addr: SocketAddr) -> Self {
        self.config.bind_addr = addr;
        self
    }
    
    /// Add seed node
    pub fn seed_node(mut self, addr: SocketAddr) -> Self {
        self.config.seed_nodes.push(addr);
        self
    }
    
    /// Set gossip interval
    pub fn gossip_interval(mut self, interval: Duration) -> Self {
        self.config.gossip_interval = interval;
        self
    }
    
    /// Enable compression
    pub fn compression(mut self, enable: bool) -> Self {
        self.config.enable_compression = enable;
        self
    }
    
    /// Build the configuration
    pub fn build(self) -> Result<ClusterConfig, ConfigError> {
        self.config.validate()?;
        Ok(self.config)
    }
}

impl Default for ClusterConfigBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn test_default_config() {
        let config = ClusterConfig::default();
        assert!(config.validate().is_ok());
        assert_eq!(config.gossip_fanout, 3);
        assert_eq!(config.max_gossip_packet_size, 1400);
        assert!(config.enable_compression);
        assert!(!config.enable_encryption);
    }

    #[test]
    fn test_config_builder() {
        let config = ClusterConfigBuilder::new()
            .bind_addr("127.0.0.1:7946".parse().unwrap())
            .seed_node("127.0.0.1:7947".parse().unwrap())
            .gossip_interval(Duration::from_millis(100))
            .compression(true)
            .build();
        
        assert!(config.is_ok());
        let config = config.unwrap();
        assert_eq!(config.seed_nodes.len(), 1);
        assert_eq!(config.gossip_interval, Duration::from_millis(100));
    }

    #[test]
    fn test_config_validation() {
        let mut config = ClusterConfig::default();
        
        // Test invalid gossip interval
        config.gossip_interval = Duration::from_millis(10);
        assert!(config.validate().is_err());
        
        // Test invalid probe timeout
        config.gossip_interval = Duration::from_millis(200);
        config.probe_timeout = config.probe_interval;
        assert!(config.validate().is_err());
        
        // Test encryption without key
        config.probe_timeout = Duration::from_millis(100);
        config.enable_encryption = true;
        config.encryption_key = None;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_fluent_api() {
        let config = ClusterConfig::new()
            .with_bind_addr("0.0.0.0:8000".parse().unwrap())
            .with_seed_node("127.0.0.1:8001".parse().unwrap())
            .with_gossip_interval(Duration::from_millis(150))
            .with_compression(false)
            .with_metadata("datacenter", "us-west-1")
            .with_metadata("rack", "rack-1");
        
        assert_eq!(config.bind_addr.port(), 8000);
        assert_eq!(config.seed_nodes.len(), 1);
        assert_eq!(config.gossip_interval, Duration::from_millis(150));
        assert!(!config.enable_compression);
        assert_eq!(config.metadata.get("datacenter"), Some(&"us-west-1".to_string()));
    }
}
