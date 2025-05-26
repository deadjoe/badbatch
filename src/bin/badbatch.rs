//! BadBatch CLI Tool
//!
//! Command-line interface for managing BadBatch Disruptor clusters.
//! Provides commands for cluster management, event publishing, monitoring, and more.

use clap::{Parser, Subcommand};
use std::path::PathBuf;

mod cli;

use cli::CliResult;

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
    },
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
        #[arg(short = 't', long = "type", default_value = "server")]
        config_type: String,
    },
}

#[tokio::main]
async fn main() -> CliResult<()> {
    let cli = Cli::parse();

    // Initialize logging
    let log_level = if cli.verbose { "debug" } else { "info" };
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(log_level)).init();

    // Load configuration if provided
    let config = if let Some(_config_path) = &cli.config {
        // Configuration loading will be implemented later
        None
    } else {
        None
    };

    // Create client
    let client = cli::client::BadBatchClient::new(&cli.endpoint)?;

    // Execute command
    match cli.command {
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
        } => {
            cli::server::start_server(server_config, bind).await?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn test_cli_parsing_basic() {
        // Test basic command parsing
        let args = vec!["badbatch", "disruptor", "list"];
        let cli = Cli::try_parse_from(args);
        assert!(cli.is_ok());

        let cli = cli.unwrap();
        assert_eq!(cli.endpoint, "http://localhost:8080"); // default value
        assert_eq!(cli.format, "table"); // default value
        assert!(!cli.verbose); // default value
        assert!(cli.config.is_none()); // default value
    }

    #[test]
    fn test_cli_parsing_with_options() {
        let args = vec![
            "badbatch",
            "--endpoint",
            "http://example.com:9000",
            "--format",
            "json",
            "--verbose",
            "--config",
            "config.yaml",
            "disruptor",
            "list",
        ];
        let cli = Cli::try_parse_from(args);
        assert!(cli.is_ok());

        let cli = cli.unwrap();
        assert_eq!(cli.endpoint, "http://example.com:9000");
        assert_eq!(cli.format, "json");
        assert!(cli.verbose);
        assert!(cli.config.is_some());
        assert_eq!(cli.config.unwrap().to_str().unwrap(), "config.yaml");
    }



    #[test]
    fn test_disruptor_commands() {
        let test_cases = vec![
            vec!["badbatch", "disruptor", "list"],
            vec!["badbatch", "disruptor", "show", "test-id"],
            vec!["badbatch", "disruptor", "start", "test-id"],
            vec!["badbatch", "disruptor", "stop", "test-id"],
            vec!["badbatch", "disruptor", "pause", "test-id"],
            vec!["badbatch", "disruptor", "resume", "test-id"],
            vec!["badbatch", "disruptor", "delete", "test-id"],
            vec!["badbatch", "disruptor", "delete", "test-id", "--force"],
            vec!["badbatch", "disruptor", "create", "test-disruptor"],
            vec![
                "badbatch",
                "disruptor",
                "create",
                "test-disruptor",
                "--size",
                "2048",
                "--producer",
                "multi",
            ],
        ];

        for args in test_cases {
            let cli = Cli::try_parse_from(args.clone());
            assert!(cli.is_ok(), "Failed to parse: {:?}", args);

            if let Commands::Disruptor { action } = cli.unwrap().command {
                match action {
                    DisruptorCommands::List => {
                        // TODO: Implement disruptor list command
                    }
                    DisruptorCommands::Show { id } => assert_eq!(id, "test-id"),
                    DisruptorCommands::Start { id } => assert_eq!(id, "test-id"),
                    DisruptorCommands::Stop { id } => assert_eq!(id, "test-id"),
                    DisruptorCommands::Pause { id } => assert_eq!(id, "test-id"),
                    DisruptorCommands::Resume { id } => assert_eq!(id, "test-id"),
                    DisruptorCommands::Delete { id, force: _ } => {
                        assert_eq!(id, "test-id");
                        // force flag varies by test case
                    }
                    DisruptorCommands::Create {
                        name,
                        size: _,
                        producer: _,
                        wait_strategy: _,
                    } => {
                        assert_eq!(name, "test-disruptor");
                        // Other fields have defaults or specified values
                    }
                }
            } else {
                panic!("Expected disruptor command");
            }
        }
    }

    #[test]
    fn test_event_commands() {
        let test_cases = vec![
            vec![
                "badbatch",
                "event",
                "publish",
                "test-disruptor",
                r#"{"message": "hello"}"#,
            ],
            vec![
                "badbatch",
                "event",
                "batch",
                "test-disruptor",
                r#"[{"id": 1}]"#,
            ],
            vec!["badbatch", "event", "generate", "test-disruptor"],
            vec![
                "badbatch",
                "event",
                "generate",
                "test-disruptor",
                "--count",
                "500",
                "--rate",
                "50",
            ],
            vec![
                "badbatch",
                "event",
                "publish-file",
                "test-disruptor",
                "events.json",
            ],
        ];

        for args in test_cases {
            let cli = Cli::try_parse_from(args.clone());
            assert!(cli.is_ok(), "Failed to parse: {:?}", args);

            if let Commands::Event { action } = cli.unwrap().command {
                match action {
                    EventCommands::Publish {
                        disruptor,
                        data,
                        metadata: _,
                    } => {
                        assert_eq!(disruptor, "test-disruptor");
                        assert!(data.contains("message") || data.contains("hello"));
                    }
                    EventCommands::Batch { disruptor, data } => {
                        assert_eq!(disruptor, "test-disruptor");
                        assert!(data.starts_with('['));
                    }
                    EventCommands::Generate {
                        disruptor,
                        count: _,
                        rate: _,
                        size: _,
                    } => {
                        assert_eq!(disruptor, "test-disruptor");
                        // count, rate, size have defaults or specified values
                    }
                    EventCommands::PublishFile {
                        disruptor,
                        file,
                        batch_size: _,
                    } => {
                        assert_eq!(disruptor, "test-disruptor");
                        assert_eq!(file.to_str().unwrap(), "events.json");
                    }
                }
            } else {
                panic!("Expected event command");
            }
        }
    }

    #[test]
    fn test_monitor_commands() {
        let test_cases = vec![
            vec!["badbatch", "monitor", "system"],
            vec!["badbatch", "monitor", "disruptor", "test-id"],
            vec![
                "badbatch",
                "monitor",
                "watch",
                "--interval",
                "10",
                "--monitor-type",
                "system",
            ],
            vec!["badbatch", "monitor", "export", "metrics.json"],
            vec![
                "badbatch",
                "monitor",
                "export",
                "metrics.csv",
                "--format",
                "csv",
            ],
        ];

        for args in test_cases {
            let cli = Cli::try_parse_from(args.clone());
            assert!(cli.is_ok(), "Failed to parse: {:?}", args);

            if let Commands::Monitor { action } = cli.unwrap().command {
                match action {
                    MonitorCommands::System => {
                        // TODO: Implement system monitoring command
                    }
                    MonitorCommands::Disruptor { id } => assert_eq!(id, "test-id"),
                    MonitorCommands::Watch {
                        interval: _,
                        monitor_type: _,
                        target: _,
                    } => {
                        // interval, monitor_type have defaults or specified values
                    }
                    MonitorCommands::Export { output, format: _ } => {
                        assert!(output.to_str().unwrap().contains("metrics"));
                    }
                }
            } else {
                panic!("Expected monitor command");
            }
        }
    }

    #[test]
    fn test_config_commands() {
        let test_cases = vec![
            vec!["badbatch", "config", "show"],
            vec!["badbatch", "config", "validate", "config.yaml"],
            vec!["badbatch", "config", "example"],
            vec![
                "badbatch",
                "config",
                "example",
                "example.yaml",
                "--type",
                "server",
            ],
        ];

        for args in test_cases {
            let cli = Cli::try_parse_from(args.clone());
            assert!(cli.is_ok(), "Failed to parse: {:?}", args);

            if let Commands::Config { action } = cli.unwrap().command {
                match action {
                    ConfigCommands::Show => {
                        // TODO: Implement config show command
                    }
                    ConfigCommands::Validate { file } => {
                        assert_eq!(file.to_str().unwrap(), "config.yaml");
                    }
                    ConfigCommands::Example {
                        output: _,
                        config_type: _,
                    } => {
                        // output is optional, config_type has default
                    }
                }
            } else {
                panic!("Expected config command");
            }
        }
    }

    #[test]
    fn test_server_command() {
        let test_cases = vec![
            vec!["badbatch", "server"],
            vec!["badbatch", "server", "--bind", "127.0.0.1:9000"],
            vec![
                "badbatch",
                "server",
                "--config",
                "server.yaml",
            ],
        ];

        for args in test_cases {
            let cli = Cli::try_parse_from(args.clone());
            assert!(cli.is_ok(), "Failed to parse: {:?}", args);

            if let Commands::Server {
                config: _,
                bind,
            } = cli.unwrap().command
            {
                // bind has default value
                assert!(!bind.is_empty());
                // config varies by test case
            } else {
                panic!("Expected server command");
            }
        }
    }

    #[test]
    fn test_default_values() {
        let args = vec!["badbatch", "disruptor", "list"];
        let cli = Cli::try_parse_from(args).unwrap();

        assert_eq!(cli.endpoint, "http://localhost:8080");
        assert_eq!(cli.format, "table");
        assert!(!cli.verbose);
        assert!(cli.config.is_none());
    }

    #[test]
    fn test_log_level_determination() {
        // Test verbose flag
        let verbose = true;
        let log_level = if verbose { "debug" } else { "info" };
        assert_eq!(log_level, "debug");

        // Test non-verbose
        let verbose = false;
        let log_level = if verbose { "debug" } else { "info" };
        assert_eq!(log_level, "info");
    }

    #[test]
    fn test_invalid_command_parsing() {
        let invalid_args = vec![
            vec!["badbatch"],                      // Missing subcommand
            vec!["badbatch", "invalid"],           // Invalid subcommand
            vec!["badbatch", "disruptor", "show"], // Missing required argument
        ];

        for args in invalid_args {
            let cli = Cli::try_parse_from(args.clone());
            assert!(cli.is_err(), "Should fail to parse: {:?}", args);
        }
    }
}
