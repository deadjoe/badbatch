//! BadBatch CLI Tool
//!
//! Command-line interface for managing BadBatch Disruptor clusters.
//! Provides commands for cluster management, event publishing, monitoring, and more.

use clap::{Parser, Subcommand};
use std::path::PathBuf;
use tokio;

mod cli;
mod client;
mod config;

use cli::{CliError, CliResult};

/// BadBatch Disruptor Engine CLI
#[derive(Parser)]
#[command(name = "badbatch")]
#[command(about = "A high-performance distributed disruptor engine")]
#[command(version = env!("CARGO_PKG_VERSION"))]
#[command(author = "BadBatch Team")]
struct Cli {
    /// Configuration file path
    #[arg(short, long, value_name = "FILE")]
    config: Option<PathBuf>,

    /// Server endpoint URL
    #[arg(short, long, default_value = "http://localhost:8080")]
    endpoint: String,

    /// Enable verbose output
    #[arg(short, long)]
    verbose: bool,

    /// Output format (json, yaml, table)
    #[arg(short, long, default_value = "table")]
    format: String,

    /// Subcommands
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Cluster management commands
    Cluster {
        #[command(subcommand)]
        action: ClusterCommands,
    },
    /// Disruptor management commands
    Disruptor {
        #[command(subcommand)]
        action: DisruptorCommands,
    },
    /// Event publishing commands
    Event {
        #[command(subcommand)]
        action: EventCommands,
    },
    /// Monitoring and metrics commands
    Monitor {
        #[command(subcommand)]
        action: MonitorCommands,
    },
    /// Configuration management
    Config {
        #[command(subcommand)]
        action: ConfigCommands,
    },
    /// Start a BadBatch server
    Server {
        /// Server configuration file
        #[arg(short, long)]
        config: Option<PathBuf>,
        /// Bind address
        #[arg(short, long, default_value = "0.0.0.0:8080")]
        bind: String,
        /// Cluster mode
        #[arg(long)]
        cluster: bool,
        /// Cluster bind address
        #[arg(long, default_value = "0.0.0.0:7946")]
        cluster_bind: String,
        /// Seed nodes for cluster
        #[arg(long)]
        seed: Vec<String>,
    },
}

#[derive(Subcommand)]
enum ClusterCommands {
    /// Show cluster status
    Status,
    /// List cluster nodes
    Nodes,
    /// Join a cluster
    Join {
        /// Seed node address
        seed: String,
    },
    /// Leave the cluster
    Leave,
    /// Show cluster health
    Health,
}

#[derive(Subcommand)]
enum DisruptorCommands {
    /// Create a new disruptor
    Create {
        /// Disruptor name
        name: String,
        /// Buffer size (must be power of 2)
        #[arg(short, long, default_value = "1024")]
        size: usize,
        /// Producer type (single, multi)
        #[arg(short, long, default_value = "single")]
        producer: String,
        /// Wait strategy (blocking, busy-spin, yielding, sleeping)
        #[arg(short, long, default_value = "blocking")]
        wait_strategy: String,
    },
    /// List all disruptors
    List,
    /// Show disruptor details
    Show {
        /// Disruptor ID or name
        id: String,
    },
    /// Delete a disruptor
    Delete {
        /// Disruptor ID or name
        id: String,
        /// Force deletion without confirmation
        #[arg(short, long)]
        force: bool,
    },
    /// Start a disruptor
    Start {
        /// Disruptor ID or name
        id: String,
    },
    /// Stop a disruptor
    Stop {
        /// Disruptor ID or name
        id: String,
    },
    /// Pause a disruptor
    Pause {
        /// Disruptor ID or name
        id: String,
    },
    /// Resume a disruptor
    Resume {
        /// Disruptor ID or name
        id: String,
    },
}

#[derive(Subcommand)]
enum EventCommands {
    /// Publish a single event
    Publish {
        /// Disruptor ID or name
        disruptor: String,
        /// Event data (JSON)
        data: String,
        /// Event metadata
        #[arg(short, long)]
        metadata: Vec<String>,
    },
    /// Publish events from file
    PublishFile {
        /// Disruptor ID or name
        disruptor: String,
        /// Input file path
        file: PathBuf,
        /// Batch size
        #[arg(short, long, default_value = "100")]
        batch_size: usize,
    },
    /// Publish events in batch
    Batch {
        /// Disruptor ID or name
        disruptor: String,
        /// Event data (JSON array)
        data: String,
    },
    /// Generate test events
    Generate {
        /// Disruptor ID or name
        disruptor: String,
        /// Number of events to generate
        #[arg(short, long, default_value = "1000")]
        count: usize,
        /// Events per second
        #[arg(short, long, default_value = "100")]
        rate: usize,
        /// Event size in bytes
        #[arg(short, long, default_value = "256")]
        size: usize,
    },
}

#[derive(Subcommand)]
enum MonitorCommands {
    /// Show system metrics
    System,
    /// Show disruptor metrics
    Disruptor {
        /// Disruptor ID or name
        id: String,
    },
    /// Show cluster metrics
    Cluster,
    /// Continuous monitoring (watch mode)
    Watch {
        /// Refresh interval in seconds
        #[arg(short, long, default_value = "5")]
        interval: u64,
        /// Monitor type (system, disruptor, cluster)
        #[arg(short, long, default_value = "system")]
        monitor_type: String,
        /// Target ID for disruptor monitoring
        #[arg(long)]
        target: Option<String>,
    },
    /// Export metrics to file
    Export {
        /// Output file path
        output: PathBuf,
        /// Export format (json, csv, prometheus)
        #[arg(short, long, default_value = "json")]
        format: String,
    },
}

#[derive(Subcommand)]
enum ConfigCommands {
    /// Show current configuration
    Show,
    /// Validate configuration file
    Validate {
        /// Configuration file path
        file: PathBuf,
    },
    /// Generate example configuration
    Example {
        /// Output file path
        output: Option<PathBuf>,
        /// Configuration type (server, cluster, client)
        #[arg(short, long, default_value = "server")]
        config_type: String,
    },
}

#[tokio::main]
async fn main() -> CliResult<()> {
    let cli = Cli::parse();

    // Initialize logging
    let log_level = if cli.verbose { "debug" } else { "info" };
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(log_level))
        .init();

    // Load configuration if provided
    let config = if let Some(config_path) = &cli.config {
        Some(config::load_config(config_path).await?)
    } else {
        None
    };

    // Create client
    let client = client::BadBatchClient::new(&cli.endpoint)?;

    // Execute command
    match cli.command {
        Commands::Cluster { action } => {
            cli::cluster::handle_cluster_command(action, &client, &cli.format).await?;
        }
        Commands::Disruptor { action } => {
            cli::disruptor::handle_disruptor_command(action, &client, &cli.format).await?;
        }
        Commands::Event { action } => {
            cli::event::handle_event_command(action, &client, &cli.format).await?;
        }
        Commands::Monitor { action } => {
            cli::monitor::handle_monitor_command(action, &client, &cli.format).await?;
        }
        Commands::Config { action } => {
            cli::config::handle_config_command(action, config.as_ref(), &cli.format).await?;
        }
        Commands::Server {
            config: server_config,
            bind,
            cluster,
            cluster_bind,
            seed,
        } => {
            cli::server::start_server(server_config, bind, cluster, cluster_bind, seed).await?;
        }
    }

    Ok(())
}
