//! Config Command Handlers
//!
//! Command handlers for configuration management operations.

use std::path::PathBuf;

use crate::bin::{ConfigCommands, config::Config};
use crate::cli::{CliResult, format};

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
    
    let config = crate::config::load_config(file).await?;
    crate::config::validate_config(&config)?;
    
    println!("✓ Configuration is valid");
    Ok(())
}

async fn generate_example_config(
    output: Option<&PathBuf>,
    config_type: &str,
) -> CliResult<()> {
    let config = crate::config::generate_example_config(config_type)?;
    
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
