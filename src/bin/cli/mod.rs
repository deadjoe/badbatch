//! CLI Module
//!
//! Command-line interface implementation for BadBatch.

pub mod cluster;
pub mod config;
pub mod disruptor;
pub mod event;
pub mod monitor;
pub mod server;
pub mod output;

use std::fmt;

/// CLI result type
pub type CliResult<T> = Result<T, CliError>;

/// CLI errors
#[derive(Debug, thiserror::Error)]
pub enum CliError {
    #[error("HTTP client error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("JSON serialization error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("YAML error: {0}")]
    Yaml(#[from] serde_yaml::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("URL parse error: {0}")]
    UrlError(#[from] url::ParseError),

    #[error("Configuration error: {message}")]
    Config { message: String },

    #[error("Validation error: {message}")]
    Validation { message: String },

    #[error("Server error: {status} - {message}")]
    Server { status: u16, message: String },

    #[error("Not found: {resource}")]
    NotFound { resource: String },

    #[error("Invalid input: {message}")]
    InvalidInput { message: String },

    #[error("Operation failed: {message}")]
    Operation { message: String },

    #[error("Network error: {message}")]
    Network { message: String },

    #[error("Timeout error: operation timed out after {seconds}s")]
    Timeout { seconds: u64 },
}

impl CliError {
    /// Create a configuration error
    pub fn config<S: Into<String>>(message: S) -> Self {
        Self::Config {
            message: message.into(),
        }
    }

    /// Create a validation error
    pub fn validation<S: Into<String>>(message: S) -> Self {
        Self::Validation {
            message: message.into(),
        }
    }

    /// Create a server error
    pub fn server(status: u16, message: String) -> Self {
        Self::Server { status, message }
    }

    /// Create a not found error
    pub fn not_found<S: Into<String>>(resource: S) -> Self {
        Self::NotFound {
            resource: resource.into(),
        }
    }

    /// Create an invalid input error
    pub fn invalid_input<S: Into<String>>(message: S) -> Self {
        Self::InvalidInput {
            message: message.into(),
        }
    }

    /// Create an operation error
    pub fn operation<S: Into<String>>(message: S) -> Self {
        Self::Operation {
            message: message.into(),
        }
    }

    /// Create a network error
    pub fn network<S: Into<String>>(message: S) -> Self {
        Self::Network {
            message: message.into(),
        }
    }

    /// Create a timeout error
    pub fn timeout(seconds: u64) -> Self {
        Self::Timeout { seconds }
    }
}

/// Output formatting utilities
pub mod format {
    use serde::Serialize;
    use tabled::{Table, Tabled};

    /// Format output based on the specified format
    pub fn format_output<T>(data: &T, format: &str) -> crate::bin::cli::CliResult<String>
    where
        T: Serialize,
    {
        match format.to_lowercase().as_str() {
            "json" => Ok(serde_json::to_string_pretty(data)?),
            "yaml" => Ok(serde_yaml::to_string(data)?),
            "table" => {
                // For table format, we need the type to implement Tabled
                // This is a simplified implementation
                Ok(serde_json::to_string_pretty(data)?)
            }
            _ => Err(crate::bin::cli::CliError::invalid_input(format!(
                "Unsupported format: {}. Supported formats: json, yaml, table",
                format
            ))),
        }
    }

    /// Format a list of items as a table
    pub fn format_table<T>(items: &[T]) -> String
    where
        T: Tabled,
    {
        if items.is_empty() {
            "No items found.".to_string()
        } else {
            Table::new(items).to_string()
        }
    }

    /// Format a single item
    pub fn format_item<T>(item: &T, format: &str) -> crate::bin::cli::CliResult<String>
    where
        T: Serialize,
    {
        format_output(item, format)
    }
}

/// Common utilities for CLI commands
pub mod utils {
    use super::CliError;
    use std::collections::HashMap;

    /// Parse key-value pairs from command line arguments
    pub fn parse_key_value_pairs(pairs: &[String]) -> Result<HashMap<String, String>, CliError> {
        let mut map = HashMap::new();

        for pair in pairs {
            let parts: Vec<&str> = pair.splitn(2, '=').collect();
            if parts.len() != 2 {
                return Err(CliError::invalid_input(format!(
                    "Invalid key-value pair: '{}'. Expected format: key=value",
                    pair
                )));
            }
            map.insert(parts[0].to_string(), parts[1].to_string());
        }

        Ok(map)
    }

    /// Validate JSON string
    pub fn validate_json(json_str: &str) -> Result<serde_json::Value, CliError> {
        serde_json::from_str(json_str).map_err(|e| {
            CliError::invalid_input(format!("Invalid JSON: {}", e))
        })
    }

    /// Confirm user action
    pub fn confirm_action(message: &str) -> Result<bool, CliError> {
        use std::io::{self, Write};

        print!("{} (y/N): ", message);
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;

        Ok(input.trim().to_lowercase() == "y" || input.trim().to_lowercase() == "yes")
    }

    /// Format duration in human-readable format
    pub fn format_duration(duration: std::time::Duration) -> String {
        let secs = duration.as_secs();
        if secs < 60 {
            format!("{}s", secs)
        } else if secs < 3600 {
            format!("{}m {}s", secs / 60, secs % 60)
        } else {
            format!("{}h {}m {}s", secs / 3600, (secs % 3600) / 60, secs % 60)
        }
    }

    /// Format bytes in human-readable format
    pub fn format_bytes(bytes: u64) -> String {
        const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
        let mut size = bytes as f64;
        let mut unit_index = 0;

        while size >= 1024.0 && unit_index < UNITS.len() - 1 {
            size /= 1024.0;
            unit_index += 1;
        }

        if unit_index == 0 {
            format!("{} {}", bytes, UNITS[unit_index])
        } else {
            format!("{:.2} {}", size, UNITS[unit_index])
        }
    }

    /// Validate disruptor name
    pub fn validate_disruptor_name(name: &str) -> Result<(), CliError> {
        if name.is_empty() {
            return Err(CliError::invalid_input("Disruptor name cannot be empty"));
        }

        if name.len() > 64 {
            return Err(CliError::invalid_input("Disruptor name cannot exceed 64 characters"));
        }

        if !name.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_') {
            return Err(CliError::invalid_input(
                "Disruptor name can only contain alphanumeric characters, hyphens, and underscores"
            ));
        }

        Ok(())
    }

    /// Validate buffer size (must be power of 2)
    pub fn validate_buffer_size(size: usize) -> Result<(), CliError> {
        if size == 0 {
            return Err(CliError::invalid_input("Buffer size cannot be zero"));
        }

        if !size.is_power_of_two() {
            return Err(CliError::invalid_input("Buffer size must be a power of 2"));
        }

        if size < 2 {
            return Err(CliError::invalid_input("Buffer size must be at least 2"));
        }

        if size > 1_048_576 {
            return Err(CliError::invalid_input("Buffer size cannot exceed 1,048,576"));
        }

        Ok(())
    }
}

/// Progress reporting utilities
pub mod progress {
    use indicatif::{ProgressBar, ProgressStyle};
    use std::time::Duration;

    /// Create a progress bar for operations
    pub fn create_progress_bar(total: u64, message: &str) -> ProgressBar {
        let pb = ProgressBar::new(total);
        pb.set_style(
            ProgressStyle::default_bar()
                .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} {msg}")
                .unwrap()
                .progress_chars("#>-"),
        );
        pb.set_message(message.to_string());
        pb.enable_steady_tick(Duration::from_millis(100));
        pb
    }

    /// Create a spinner for indeterminate operations
    pub fn create_spinner(message: &str) -> ProgressBar {
        let pb = ProgressBar::new_spinner();
        pb.set_style(
            ProgressStyle::default_spinner()
                .template("{spinner:.green} {msg}")
                .unwrap(),
        );
        pb.set_message(message.to_string());
        pb.enable_steady_tick(Duration::from_millis(100));
        pb
    }
}
