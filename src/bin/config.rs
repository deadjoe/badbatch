//! Configuration Management
//!
//! Configuration loading and management for BadBatch CLI.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use tokio::fs;

use crate::cli::{CliError, CliResult};

/// Main configuration structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Server configuration
    pub server: Option<ServerConfig>,
    /// Cluster configuration
    pub cluster: Option<ClusterConfig>,
    /// Client configuration
    pub client: Option<ClientConfig>,
    /// Default settings
    pub defaults: Option<DefaultConfig>,
}

/// Server configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    /// Server bind address
    pub bind_addr: String,
    /// Enable cluster mode
    pub cluster_mode: bool,
    /// Cluster bind address
    pub cluster_bind_addr: Option<String>,
    /// Seed nodes for cluster
    pub seed_nodes: Vec<String>,
    /// Server metadata
    pub metadata: HashMap<String, String>,
}

/// Cluster configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterConfig {
    /// Node ID
    pub node_id: Option<String>,
    /// Bind address for cluster communication
    pub bind_addr: String,
    /// Advertise address (if different from bind)
    pub advertise_addr: Option<String>,
    /// Seed nodes to join
    pub seed_nodes: Vec<String>,
    /// Gossip interval in milliseconds
    pub gossip_interval_ms: u64,
    /// Probe interval in seconds
    pub probe_interval_secs: u64,
    /// Enable compression
    pub enable_compression: bool,
    /// Enable encryption
    pub enable_encryption: bool,
    /// Encryption key (base64 encoded)
    pub encryption_key: Option<String>,
    /// Node metadata
    pub metadata: HashMap<String, String>,
}

/// Client configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientConfig {
    /// Default server endpoint
    pub endpoint: String,
    /// Request timeout in seconds
    pub timeout_secs: u64,
    /// Default output format
    pub output_format: String,
    /// Enable verbose logging
    pub verbose: bool,
    /// Custom headers
    pub headers: HashMap<String, String>,
}

/// Default configuration values
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DefaultConfig {
    /// Default disruptor buffer size
    pub buffer_size: usize,
    /// Default producer type
    pub producer_type: String,
    /// Default wait strategy
    pub wait_strategy: String,
    /// Default batch size for operations
    pub batch_size: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            server: Some(ServerConfig::default()),
            cluster: Some(ClusterConfig::default()),
            client: Some(ClientConfig::default()),
            defaults: Some(DefaultConfig::default()),
        }
    }
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind_addr: "0.0.0.0:8080".to_string(),
            cluster_mode: false,
            cluster_bind_addr: Some("0.0.0.0:7946".to_string()),
            seed_nodes: Vec::new(),
            metadata: HashMap::new(),
        }
    }
}

impl Default for ClusterConfig {
    fn default() -> Self {
        Self {
            node_id: None,
            bind_addr: "0.0.0.0:7946".to_string(),
            advertise_addr: None,
            seed_nodes: Vec::new(),
            gossip_interval_ms: 200,
            probe_interval_secs: 1,
            enable_compression: true,
            enable_encryption: false,
            encryption_key: None,
            metadata: HashMap::new(),
        }
    }
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            endpoint: "http://localhost:8080".to_string(),
            timeout_secs: 30,
            output_format: "table".to_string(),
            verbose: false,
            headers: HashMap::new(),
        }
    }
}

impl Default for DefaultConfig {
    fn default() -> Self {
        Self {
            buffer_size: 1024,
            producer_type: "single".to_string(),
            wait_strategy: "blocking".to_string(),
            batch_size: 100,
        }
    }
}

/// Load configuration from file
pub async fn load_config<P: AsRef<Path>>(path: P) -> CliResult<Config> {
    let content = fs::read_to_string(path).await?;
    
    // Try to detect format from content
    if content.trim_start().starts_with('{') {
        // JSON format
        serde_json::from_str(&content).map_err(CliError::from)
    } else {
        // YAML format
        serde_yaml::from_str(&content).map_err(CliError::from)
    }
}

/// Save configuration to file
pub async fn save_config<P: AsRef<Path>>(config: &Config, path: P, format: &str) -> CliResult<()> {
    let content = match format.to_lowercase().as_str() {
        "json" => serde_json::to_string_pretty(config)?,
        "yaml" => serde_yaml::to_string(config)?,
        _ => return Err(CliError::invalid_input(format!("Unsupported format: {}", format))),
    };
    
    fs::write(path, content).await?;
    Ok(())
}

/// Validate configuration
pub fn validate_config(config: &Config) -> CliResult<()> {
    // Validate server config
    if let Some(server) = &config.server {
        validate_server_config(server)?;
    }
    
    // Validate cluster config
    if let Some(cluster) = &config.cluster {
        validate_cluster_config(cluster)?;
    }
    
    // Validate client config
    if let Some(client) = &config.client {
        validate_client_config(client)?;
    }
    
    // Validate defaults
    if let Some(defaults) = &config.defaults {
        validate_default_config(defaults)?;
    }
    
    Ok(())
}

fn validate_server_config(config: &ServerConfig) -> CliResult<()> {
    // Validate bind address
    if config.bind_addr.is_empty() {
        return Err(CliError::validation("Server bind address cannot be empty"));
    }
    
    // Try to parse the address
    config.bind_addr.parse::<std::net::SocketAddr>()
        .map_err(|e| CliError::validation(format!("Invalid server bind address: {}", e)))?;
    
    // Validate cluster bind address if provided
    if let Some(cluster_addr) = &config.cluster_bind_addr {
        cluster_addr.parse::<std::net::SocketAddr>()
            .map_err(|e| CliError::validation(format!("Invalid cluster bind address: {}", e)))?;
    }
    
    Ok(())
}

fn validate_cluster_config(config: &ClusterConfig) -> CliResult<()> {
    // Validate bind address
    config.bind_addr.parse::<std::net::SocketAddr>()
        .map_err(|e| CliError::validation(format!("Invalid cluster bind address: {}", e)))?;
    
    // Validate advertise address if provided
    if let Some(advertise_addr) = &config.advertise_addr {
        advertise_addr.parse::<std::net::SocketAddr>()
            .map_err(|e| CliError::validation(format!("Invalid advertise address: {}", e)))?;
    }
    
    // Validate seed nodes
    for seed in &config.seed_nodes {
        seed.parse::<std::net::SocketAddr>()
            .map_err(|e| CliError::validation(format!("Invalid seed node address '{}': {}", seed, e)))?;
    }
    
    // Validate intervals
    if config.gossip_interval_ms < 50 {
        return Err(CliError::validation("Gossip interval must be at least 50ms"));
    }
    
    if config.probe_interval_secs == 0 {
        return Err(CliError::validation("Probe interval must be greater than 0"));
    }
    
    Ok(())
}

fn validate_client_config(config: &ClientConfig) -> CliResult<()> {
    // Validate endpoint URL
    url::Url::parse(&config.endpoint)
        .map_err(|e| CliError::validation(format!("Invalid endpoint URL: {}", e)))?;
    
    // Validate timeout
    if config.timeout_secs == 0 {
        return Err(CliError::validation("Timeout must be greater than 0"));
    }
    
    // Validate output format
    match config.output_format.to_lowercase().as_str() {
        "json" | "yaml" | "table" => {}
        _ => return Err(CliError::validation(format!(
            "Invalid output format: {}. Supported formats: json, yaml, table",
            config.output_format
        ))),
    }
    
    Ok(())
}

fn validate_default_config(config: &DefaultConfig) -> CliResult<()> {
    // Validate buffer size
    if config.buffer_size == 0 {
        return Err(CliError::validation("Buffer size cannot be zero"));
    }
    
    if !config.buffer_size.is_power_of_two() {
        return Err(CliError::validation("Buffer size must be a power of 2"));
    }
    
    // Validate producer type
    match config.producer_type.to_lowercase().as_str() {
        "single" | "multi" => {}
        _ => return Err(CliError::validation(format!(
            "Invalid producer type: {}. Supported types: single, multi",
            config.producer_type
        ))),
    }
    
    // Validate wait strategy
    match config.wait_strategy.to_lowercase().as_str() {
        "blocking" | "busy-spin" | "yielding" | "sleeping" => {}
        _ => return Err(CliError::validation(format!(
            "Invalid wait strategy: {}. Supported strategies: blocking, busy-spin, yielding, sleeping",
            config.wait_strategy
        ))),
    }
    
    // Validate batch size
    if config.batch_size == 0 {
        return Err(CliError::validation("Batch size cannot be zero"));
    }
    
    Ok(())
}

/// Generate example configuration
pub fn generate_example_config(config_type: &str) -> CliResult<Config> {
    match config_type.to_lowercase().as_str() {
        "server" => Ok(Config {
            server: Some(ServerConfig::default()),
            cluster: None,
            client: None,
            defaults: Some(DefaultConfig::default()),
        }),
        "cluster" => Ok(Config {
            server: Some(ServerConfig {
                cluster_mode: true,
                ..ServerConfig::default()
            }),
            cluster: Some(ClusterConfig::default()),
            client: None,
            defaults: Some(DefaultConfig::default()),
        }),
        "client" => Ok(Config {
            server: None,
            cluster: None,
            client: Some(ClientConfig::default()),
            defaults: Some(DefaultConfig::default()),
        }),
        "full" => Ok(Config::default()),
        _ => Err(CliError::invalid_input(format!(
            "Invalid config type: {}. Supported types: server, cluster, client, full",
            config_type
        ))),
    }
}
