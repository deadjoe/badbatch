//! Output Formatting
//!
//! Utilities for formatting CLI output in different formats.

use serde::Serialize;
use tabled::{Table, Tabled};

use crate::cli::CliResult;

/// Format output based on the specified format
pub fn format_output<T>(data: &T, format: &str) -> CliResult<String>
where
    T: Serialize,
{
    match format.to_lowercase().as_str() {
        "json" => Ok(serde_json::to_string_pretty(data)?),
        "yaml" => Ok(serde_yaml::to_string(data)?),
        "table" => {
            // For table format, we need the type to implement Tabled
            // This is a simplified implementation that falls back to JSON
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
pub fn format_item<T>(item: &T, format: &str) -> CliResult<String>
where
    T: Serialize,
{
    format_output(item, format)
}

/// Format success message
pub fn success_message(message: &str) -> String {
    format!("✓ {}", message)
}

/// Format error message
pub fn error_message(message: &str) -> String {
    format!("✗ {}", message)
}

/// Format warning message
pub fn warning_message(message: &str) -> String {
    format!("⚠ {}", message)
}

/// Format info message
pub fn info_message(message: &str) -> String {
    format!("ℹ {}", message)
}
