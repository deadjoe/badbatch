//! Output Formatting
//!
//! Utilities for formatting CLI output in different formats.

use serde::Serialize;
use tabled::{Table, Tabled};

use crate::cli::CliResult;

/// Format output based on the specified format
#[allow(dead_code)]
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
pub fn format_item<T>(item: &T, format: &str) -> CliResult<String>
where
    T: Serialize,
{
    format_output(item, format)
}

/// Format success message
/// This function is used by CLI commands for user feedback
#[allow(dead_code)]
pub fn success_message(message: &str) -> String {
    format!("✓ {}", message)
}

/// Format error message
/// This function is used by CLI commands for error reporting
#[allow(dead_code)]
pub fn error_message(message: &str) -> String {
    format!("✗ {}", message)
}

/// Format warning message
/// This function is used by CLI commands for warnings
#[allow(dead_code)]
pub fn warning_message(message: &str) -> String {
    format!("⚠ {}", message)
}

/// Format info message
/// This function is used by CLI commands for information
#[allow(dead_code)]
pub fn info_message(message: &str) -> String {
    format!("ℹ {}", message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};
    use tabled::Tabled;

    #[derive(Serialize, Deserialize)]
    struct TestData {
        name: String,
        value: i32,
    }

    #[derive(Tabled)]
    struct TestTableData {
        id: u32,
        name: String,
        active: bool,
    }

    #[test]
    fn test_message_formatting() {
        assert_eq!(success_message("test"), "✓ test");
        assert_eq!(error_message("test"), "✗ test");
        assert_eq!(warning_message("test"), "⚠ test");
        assert_eq!(info_message("test"), "ℹ test");
    }

    #[test]
    fn test_format_output_json() {
        let data = TestData {
            name: "test".to_string(),
            value: 42,
        };

        let result = format_output(&data, "json").unwrap();
        assert!(result.contains("\"name\": \"test\""));
        assert!(result.contains("\"value\": 42"));
    }

    #[test]
    fn test_format_output_yaml() {
        let data = TestData {
            name: "test".to_string(),
            value: 42,
        };

        let result = format_output(&data, "yaml").unwrap();
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
        let result = format_output(&data, "table").unwrap();
        assert!(result.contains("\"name\": \"test\""));
    }

    #[test]
    fn test_format_output_unsupported() {
        let data = TestData {
            name: "test".to_string(),
            value: 42,
        };

        let result = format_output(&data, "xml");
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

        let result = format_item(&data, "json").unwrap();
        assert!(result.contains("\"name\": \"test\""));
    }

    #[test]
    fn test_format_table_empty() {
        let empty_data: Vec<TestTableData> = vec![];
        let result = format_table(&empty_data);
        assert_eq!(result, "No items found.");
    }

    #[test]
    fn test_format_table_with_data() {
        let data = vec![
            TestTableData {
                id: 1,
                name: "test1".to_string(),
                active: true,
            },
            TestTableData {
                id: 2,
                name: "test2".to_string(),
                active: false,
            },
        ];

        let result = format_table(&data);
        assert!(result.contains("test1"));
        assert!(result.contains("test2"));
        assert!(result.contains("true"));
        assert!(result.contains("false"));
    }

    #[test]
    fn test_format_output_case_insensitive() {
        let data = TestData {
            name: "test".to_string(),
            value: 42,
        };

        // Test uppercase
        let result = format_output(&data, "JSON").unwrap();
        assert!(result.contains("\"name\": \"test\""));

        // Test mixed case
        let result = format_output(&data, "YaML").unwrap();
        assert!(result.contains("name: test"));
    }

    #[test]
    fn test_message_formatting_edge_cases() {
        // Test empty string
        assert_eq!(success_message(""), "✓ ");

        // Test string with special characters
        assert_eq!(
            error_message("Error: 404 Not Found"),
            "✗ Error: 404 Not Found"
        );

        // Test multiline string
        let multiline = "Line 1\nLine 2";
        assert_eq!(warning_message(multiline), "⚠ Line 1\nLine 2");
    }
}
