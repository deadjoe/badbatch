//! CLI Module
//!
//! Command-line interface implementation for BadBatch.

pub mod cluster;
pub mod config;
pub mod disruptor;
pub mod event;
pub mod monitor;
pub mod output;
pub mod progress;
pub mod server;
pub mod utils;

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
    pub fn format_output<T>(data: &T, format: &str) -> crate::cli::CliResult<String>
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
            _ => Err(crate::cli::CliError::invalid_input(format!(
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
    pub fn format_item<T>(item: &T, format: &str) -> crate::cli::CliResult<String>
    where
        T: Serialize,
    {
        format_output(item, format)
    }
}


