//! CLI Module
//!
//! Command-line interface implementation for BadBatch.

pub mod client;
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
#[allow(dead_code)]
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

#[allow(dead_code)]
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
    #[allow(dead_code)]
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
    #[allow(dead_code)]
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
    #[allow(dead_code)]
    pub fn format_item<T>(item: &T, format: &str) -> crate::cli::CliResult<String>
    where
        T: Serialize,
    {
        format_output(item, format)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cli_error_constructors() {
        // Test config error
        let config_err = CliError::config("Invalid configuration");
        assert!(matches!(config_err, CliError::Config { .. }));
        assert_eq!(
            config_err.to_string(),
            "Configuration error: Invalid configuration"
        );

        // Test validation error
        let validation_err = CliError::validation("Invalid input");
        assert!(matches!(validation_err, CliError::Validation { .. }));
        assert_eq!(
            validation_err.to_string(),
            "Validation error: Invalid input"
        );

        // Test server error
        let server_err = CliError::server(404, "Not found".to_string());
        assert!(matches!(server_err, CliError::Server { .. }));
        assert_eq!(server_err.to_string(), "Server error: 404 - Not found");

        // Test not found error
        let not_found_err = CliError::not_found("resource");
        assert!(matches!(not_found_err, CliError::NotFound { .. }));
        assert_eq!(not_found_err.to_string(), "Not found: resource");

        // Test invalid input error
        let invalid_input_err = CliError::invalid_input("bad input");
        assert!(matches!(invalid_input_err, CliError::InvalidInput { .. }));
        assert_eq!(invalid_input_err.to_string(), "Invalid input: bad input");

        // Test operation error
        let operation_err = CliError::operation("operation failed");
        assert!(matches!(operation_err, CliError::Operation { .. }));
        assert_eq!(
            operation_err.to_string(),
            "Operation failed: operation failed"
        );

        // Test network error
        let network_err = CliError::network("connection failed");
        assert!(matches!(network_err, CliError::Network { .. }));
        assert_eq!(network_err.to_string(), "Network error: connection failed");

        // Test timeout error
        let timeout_err = CliError::timeout(30);
        assert!(matches!(timeout_err, CliError::Timeout { .. }));
        assert_eq!(
            timeout_err.to_string(),
            "Timeout error: operation timed out after 30s"
        );
    }

    #[test]
    fn test_cli_error_from_conversions() {
        // Test from serde_json::Error
        let json_err = serde_json::from_str::<serde_json::Value>("invalid json").unwrap_err();
        let cli_err: CliError = json_err.into();
        assert!(matches!(cli_err, CliError::Json(_)));

        // Test from std::io::Error
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let cli_err: CliError = io_err.into();
        assert!(matches!(cli_err, CliError::Io(_)));

        // Test from url::ParseError
        let url_err = url::Url::parse("invalid url").unwrap_err();
        let cli_err: CliError = url_err.into();
        assert!(matches!(cli_err, CliError::UrlError(_)));
    }

    mod format_tests {
        use super::*;
        use serde::{Deserialize, Serialize};

        #[derive(Serialize, Deserialize)]
        struct TestData {
            name: String,
            value: i32,
        }

        #[test]
        fn test_format_output_json() {
            let data = TestData {
                name: "test".to_string(),
                value: 42,
            };

            let result = format::format_output(&data, "json").unwrap();
            assert!(result.contains("\"name\": \"test\""));
            assert!(result.contains("\"value\": 42"));
        }

        #[test]
        fn test_format_output_yaml() {
            let data = TestData {
                name: "test".to_string(),
                value: 42,
            };

            let result = format::format_output(&data, "yaml").unwrap();
            assert!(result.contains("name: test"));
            assert!(result.contains("value: 42"));
        }

        #[test]
        fn test_format_output_table() {
            let data = TestData {
                name: "test".to_string(),
                value: 42,
            };

            // Table format falls back to JSON for now
            let result = format::format_output(&data, "table").unwrap();
            assert!(result.contains("\"name\": \"test\""));
        }

        #[test]
        fn test_format_output_unsupported() {
            let data = TestData {
                name: "test".to_string(),
                value: 42,
            };

            let result = format::format_output(&data, "xml");
            assert!(result.is_err());
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("Unsupported format"));
        }

        #[test]
        fn test_format_item() {
            let data = TestData {
                name: "test".to_string(),
                value: 42,
            };

            let result = format::format_item(&data, "json").unwrap();
            assert!(result.contains("\"name\": \"test\""));
        }
    }
}
