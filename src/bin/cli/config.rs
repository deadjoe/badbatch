//! Config Command Handlers
//!
//! Command handlers for configuration management operations.

use std::path::PathBuf;

use crate::{ConfigCommands};
use crate::cli::{CliResult, format};
use serde::{Serialize, Deserialize};

// Simple config structure for CLI
#[derive(Debug, Serialize, Deserialize)]
pub struct Config {
    pub server: ServerConfig,
    pub cluster: Option<ClusterConfig>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    pub enable_cors: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ClusterConfig {
    pub enabled: bool,
    pub bind_addr: String,
    pub seed_nodes: Vec<String>,
}

/// Handle config commands
pub async fn handle_config_command(
    command: ConfigCommands,
    config: Option<&Config>,
    output_format: &str,
) -> CliResult<()> {
    match command {
        ConfigCommands::Show => show_config(config, output_format).await,
        ConfigCommands::Validate { file } => validate_config_file(&file).await,
        ConfigCommands::Example { output, config_type } => {
            generate_example_config(output.as_ref(), &config_type).await
        }
    }
}

async fn show_config(config: Option<&Config>, output_format: &str) -> CliResult<()> {
    match config {
        Some(config) => {
            let output = format::format_output(config, output_format)?;
            println!("{}", output);
        }
        None => {
            println!("No configuration loaded. Use --config to specify a configuration file.");
        }
    }

    Ok(())
}

async fn validate_config_file(file: &PathBuf) -> CliResult<()> {
    println!("Validating configuration file: {:?}", file);

    let config = load_config(file).await?;
    validate_config(&config)?;

    println!("✓ Configuration is valid");
    Ok(())
}

async fn load_config(file: &PathBuf) -> CliResult<Config> {
    let content = tokio::fs::read_to_string(file).await?;
    let config: Config = serde_yaml::from_str(&content)?;
    Ok(config)
}

fn validate_config(config: &Config) -> CliResult<()> {
    // Basic validation
    if config.server.host.is_empty() {
        return Err(crate::cli::CliError::invalid_input("Server host cannot be empty".to_string()));
    }

    if config.server.port == 0 {
        return Err(crate::cli::CliError::invalid_input("Server port cannot be zero".to_string()));
    }

    Ok(())
}

fn generate_example_config_internal(config_type: &str) -> CliResult<Config> {
    match config_type {
        "server" => Ok(Config {
            server: ServerConfig {
                host: "0.0.0.0".to_string(),
                port: 8080,
                enable_cors: true,
            },
            cluster: None,
        }),
        "cluster" => Ok(Config {
            server: ServerConfig {
                host: "0.0.0.0".to_string(),
                port: 8080,
                enable_cors: true,
            },
            cluster: Some(ClusterConfig {
                enabled: true,
                bind_addr: "0.0.0.0:7946".to_string(),
                seed_nodes: vec!["127.0.0.1:7947".to_string()],
            }),
        }),
        _ => Err(crate::cli::CliError::invalid_input(
            format!("Unknown config type: {}. Valid types are: server, cluster", config_type)
        )),
    }
}

async fn generate_example_config(
    output: Option<&PathBuf>,
    config_type: &str,
) -> CliResult<()> {
    let config = generate_example_config_internal(config_type)?;

    let content = serde_yaml::to_string(&config)?;

    match output {
        Some(path) => {
            tokio::fs::write(path, &content).await?;
            println!("✓ Example configuration written to {:?}", path);
        }
        None => {
            println!("{}", content);
        }
    }

    Ok(())
}
