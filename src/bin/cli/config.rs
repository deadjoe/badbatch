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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_config_structures() {
        let server_config = ServerConfig {
            host: "localhost".to_string(),
            port: 8080,
            enable_cors: true,
        };

        let cluster_config = ClusterConfig {
            enabled: true,
            bind_addr: "0.0.0.0:7946".to_string(),
            seed_nodes: vec!["127.0.0.1:7947".to_string()],
        };

        let config = Config {
            server: server_config,
            cluster: Some(cluster_config),
        };

        // Test serialization
        let yaml = serde_yaml::to_string(&config).unwrap();
        assert!(yaml.contains("localhost"));
        assert!(yaml.contains("8080"));
        assert!(yaml.contains("7946"));

        // Test deserialization
        let parsed: Config = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(parsed.server.host, "localhost");
        assert_eq!(parsed.server.port, 8080);
        assert!(parsed.cluster.is_some());
    }

    #[test]
    fn test_validate_config() {
        // Valid config
        let valid_config = Config {
            server: ServerConfig {
                host: "localhost".to_string(),
                port: 8080,
                enable_cors: true,
            },
            cluster: None,
        };
        assert!(validate_config(&valid_config).is_ok());

        // Invalid config - empty host
        let invalid_config = Config {
            server: ServerConfig {
                host: "".to_string(),
                port: 8080,
                enable_cors: true,
            },
            cluster: None,
        };
        assert!(validate_config(&invalid_config).is_err());

        // Invalid config - zero port
        let invalid_config = Config {
            server: ServerConfig {
                host: "localhost".to_string(),
                port: 0,
                enable_cors: true,
            },
            cluster: None,
        };
        assert!(validate_config(&invalid_config).is_err());
    }

    #[test]
    fn test_generate_example_config_internal() {
        // Test server config
        let server_config = generate_example_config_internal("server").unwrap();
        assert_eq!(server_config.server.host, "0.0.0.0");
        assert_eq!(server_config.server.port, 8080);
        assert!(server_config.cluster.is_none());

        // Test cluster config
        let cluster_config = generate_example_config_internal("cluster").unwrap();
        assert_eq!(cluster_config.server.host, "0.0.0.0");
        assert!(cluster_config.cluster.is_some());
        assert!(cluster_config.cluster.unwrap().enabled);

        // Test invalid config type
        let result = generate_example_config_internal("invalid");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Unknown config type"));
    }

    #[tokio::test]
    async fn test_load_config() {
        let config = Config {
            server: ServerConfig {
                host: "test-host".to_string(),
                port: 9090,
                enable_cors: false,
            },
            cluster: None,
        };

        let yaml_content = serde_yaml::to_string(&config).unwrap();

        // Create a temporary file
        let temp_path = std::env::temp_dir().join("test_config.yaml");
        tokio::fs::write(&temp_path, yaml_content).await.unwrap();

        let loaded_config = load_config(&temp_path).await.unwrap();

        assert_eq!(loaded_config.server.host, "test-host");
        assert_eq!(loaded_config.server.port, 9090);
        assert!(!loaded_config.server.enable_cors);

        // Clean up
        let _ = tokio::fs::remove_file(&temp_path).await;
    }

    #[tokio::test]
    async fn test_load_config_file_not_found() {
        let path = PathBuf::from("non_existent_file.yaml");
        let result = load_config(&path).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_load_config_invalid_yaml() {
        let invalid_yaml = "invalid: yaml: content: [";

        let temp_path = std::env::temp_dir().join("test_invalid_config.yaml");
        tokio::fs::write(&temp_path, invalid_yaml).await.unwrap();

        let result = load_config(&temp_path).await;
        assert!(result.is_err());

        // Clean up
        let _ = tokio::fs::remove_file(&temp_path).await;
    }

    #[test]
    fn test_config_debug_format() {
        let config = Config {
            server: ServerConfig {
                host: "localhost".to_string(),
                port: 8080,
                enable_cors: true,
            },
            cluster: None,
        };

        let debug_str = format!("{:?}", config);
        assert!(debug_str.contains("localhost"));
        assert!(debug_str.contains("8080"));
    }
}