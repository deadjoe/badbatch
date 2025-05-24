//! BadBatch - High-Performance Data Processing Engine
//!
//! A Rust implementation of the LMAX Disruptor pattern for ultra-low latency
//! inter-thread messaging with REST API and distributed clustering capabilities.

mod disruptor;
mod api;
mod cluster;
mod client;

use clap::Parser;
use std::sync::Arc;
use tracing::{info, error};
use tracing_subscriber;

/// Command line arguments
#[derive(Parser, Debug)]
#[command(name = "badbatch")]
#[command(about = "High-performance data processing engine with Disruptor-style queue")]
#[command(version)]
pub struct Args {
    /// HTTP server port
    #[arg(short, long, default_value = "8080")]
    pub port: u16,

    /// Ring buffer size (must be power of 2)
    #[arg(short, long, default_value = "1024")]
    pub buffer_size: usize,

    /// Wait strategy
    #[arg(short, long, default_value = "blocking")]
    #[arg(value_parser = parse_wait_strategy)]
    pub wait_strategy: WaitStrategyType,

    /// Enable cluster mode
    #[arg(short, long)]
    pub cluster: bool,

    /// Seed nodes for clustering (comma-separated)
    #[arg(short, long)]
    pub seed_nodes: Option<String>,

    /// Node ID (auto-generated if not provided)
    #[arg(short, long)]
    pub node_id: Option<String>,

    /// Log level
    #[arg(long, default_value = "info")]
    pub log_level: String,
}

/// Wait strategy types
#[derive(Debug, Clone)]
pub enum WaitStrategyType {
    Blocking,
    Yielding,
    BusySpin,
    Sleeping,
}

fn parse_wait_strategy(s: &str) -> Result<WaitStrategyType, String> {
    match s.to_lowercase().as_str() {
        "blocking" => Ok(WaitStrategyType::Blocking),
        "yielding" => Ok(WaitStrategyType::Yielding),
        "busy-spin" | "busyspin" => Ok(WaitStrategyType::BusySpin),
        "sleeping" => Ok(WaitStrategyType::Sleeping),
        _ => Err(format!("Invalid wait strategy: {}", s)),
    }
}

/// Application configuration
#[derive(Debug, Clone)]
pub struct Config {
    pub port: u16,
    pub buffer_size: usize,
    pub wait_strategy: WaitStrategyType,
    pub cluster_enabled: bool,
    pub seed_nodes: Vec<String>,
    pub node_id: String,
    pub log_level: String,
}

impl From<Args> for Config {
    fn from(args: Args) -> Self {
        let node_id = args.node_id.unwrap_or_else(|| {
            uuid::Uuid::new_v4().to_string()
        });

        let seed_nodes = args.seed_nodes
            .map(|nodes| {
                nodes.split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();

        Self {
            port: args.port,
            buffer_size: args.buffer_size,
            wait_strategy: args.wait_strategy,
            cluster_enabled: args.cluster,
            seed_nodes,
            node_id,
            log_level: args.log_level,
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Parse command line arguments
    let args = Args::parse();
    let config = Config::from(args);

    // Initialize logging
    init_logging(&config.log_level)?;

    info!("Starting BadBatch data processing engine");
    info!("Configuration: {:?}", config);

    // Validate configuration
    validate_config(&config)?;

    // TODO: Initialize the Disruptor engine
    // TODO: Start the REST API server
    // TODO: Initialize clustering if enabled
    // TODO: Start performance monitoring

    info!("BadBatch engine started successfully on port {}", config.port);

    // Keep the application running
    tokio::signal::ctrl_c().await?;
    info!("Shutting down BadBatch engine");

    Ok(())
}

/// Initialize logging based on the specified level
fn init_logging(level: &str) -> anyhow::Result<()> {
    let filter = match level.to_lowercase().as_str() {
        "trace" => "trace",
        "debug" => "debug",
        "info" => "info",
        "warn" => "warn",
        "error" => "error",
        _ => "info",
    };

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_thread_ids(true)
        .with_file(true)
        .with_line_number(true)
        .init();

    Ok(())
}

/// Validate the configuration
fn validate_config(config: &Config) -> anyhow::Result<()> {
    // Validate buffer size is power of 2
    if !disruptor::is_power_of_two(config.buffer_size) {
        return Err(anyhow::anyhow!(
            "Buffer size must be a power of 2, got: {}",
            config.buffer_size
        ));
    }

    // Validate port range
    if config.port == 0 {
        return Err(anyhow::anyhow!("Port must be greater than 0"));
    }

    // Validate cluster configuration
    if config.cluster_enabled && config.seed_nodes.is_empty() {
        return Err(anyhow::anyhow!(
            "Cluster mode requires at least one seed node"
        ));
    }

    info!("Configuration validation passed");
    Ok(())
}
