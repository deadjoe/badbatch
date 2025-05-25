//! CLI Utility Functions
//!
//! Common utility functions used across CLI commands.

use std::collections::HashMap;
use std::time::Duration;
use serde_json::Value;

use crate::cli::{CliError, CliResult};
use badbatch::disruptor::is_power_of_two;

/// Validate that a string is valid JSON
pub fn validate_json(data: &str) -> CliResult<Value> {
    serde_json::from_str(data).map_err(|e| {
        CliError::invalid_input(format!("Invalid JSON: {}", e))
    })
}

/// Parse key-value pairs from command line arguments
/// Format: key1=value1,key2=value2
pub fn parse_key_value_pairs(pairs: &[String]) -> CliResult<HashMap<String, String>> {
    let mut result = HashMap::new();

    for pair in pairs {
        if let Some((key, value)) = pair.split_once('=') {
            result.insert(key.to_string(), value.to_string());
        } else {
            return Err(CliError::invalid_input(format!(
                "Invalid key-value pair: '{}'. Expected format: key=value",
                pair
            )));
        }
    }

    Ok(result)
}

/// Validate disruptor name
pub fn validate_disruptor_name(name: &str) -> CliResult<()> {
    if name.is_empty() {
        return Err(CliError::invalid_input("Disruptor name cannot be empty".to_string()));
    }

    if name.len() > 64 {
        return Err(CliError::invalid_input("Disruptor name cannot exceed 64 characters".to_string()));
    }

    // Check for valid characters (alphanumeric, dash, underscore)
    if !name.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_') {
        return Err(CliError::invalid_input(
            "Disruptor name can only contain alphanumeric characters, dashes, and underscores".to_string()
        ));
    }

    Ok(())
}

/// Validate buffer size (must be power of 2)
pub fn validate_buffer_size(size: usize) -> CliResult<()> {
    if size == 0 {
        return Err(CliError::invalid_input("Buffer size cannot be zero".to_string()));
    }

    if !is_power_of_two(size) {
        return Err(CliError::invalid_input(
            "Buffer size must be a power of 2 (e.g., 1024, 2048, 4096)".to_string()
        ));
    }

    if size < 2 {
        return Err(CliError::invalid_input("Buffer size must be at least 2".to_string()));
    }

    if size > 1024 * 1024 * 1024 {
        return Err(CliError::invalid_input("Buffer size cannot exceed 1GB".to_string()));
    }

    Ok(())
}

/// Confirm an action with the user
pub fn confirm_action(message: &str) -> CliResult<bool> {
    use std::io::{self, Write};

    print!("{} [y/N]: ", message);
    io::stdout().flush().map_err(|e| {
        CliError::operation(format!("Failed to flush stdout: {}", e))
    })?;

    let mut input = String::new();
    io::stdin().read_line(&mut input).map_err(|e| {
        CliError::operation(format!("Failed to read input: {}", e))
    })?;

    let input = input.trim().to_lowercase();
    Ok(input == "y" || input == "yes")
}

/// Format a duration in human-readable format
pub fn format_duration(duration: Duration) -> String {
    let total_seconds = duration.as_secs();

    if total_seconds < 60 {
        format!("{}s", total_seconds)
    } else if total_seconds < 3600 {
        let minutes = total_seconds / 60;
        let seconds = total_seconds % 60;
        format!("{}m {}s", minutes, seconds)
    } else if total_seconds < 86400 {
        let hours = total_seconds / 3600;
        let minutes = (total_seconds % 3600) / 60;
        let seconds = total_seconds % 60;
        if seconds > 0 {
            format!("{}h {}m {}s", hours, minutes, seconds)
        } else {
            format!("{}h {}m", hours, minutes)
        }
    } else {
        let days = total_seconds / 86400;
        let hours = (total_seconds % 86400) / 3600;
        format!("{}d {}h", days, hours)
    }
}

/// Format bytes in human-readable format
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    const THRESHOLD: f64 = 1024.0;

    if bytes == 0 {
        return "0 B".to_string();
    }

    let mut size = bytes as f64;
    let mut unit_index = 0;

    while size >= THRESHOLD && unit_index < UNITS.len() - 1 {
        size /= THRESHOLD;
        unit_index += 1;
    }

    if unit_index == 0 {
        format!("{} {}", bytes, UNITS[unit_index])
    } else {
        format!("{:.1} {}", size, UNITS[unit_index])
    }
}

/// Format a rate (events per second)
#[allow(dead_code)]
pub fn format_rate(rate: f64) -> String {
    if rate < 1000.0 {
        format!("{:.1} /s", rate)
    } else if rate < 1_000_000.0 {
        format!("{:.1}K /s", rate / 1000.0)
    } else {
        format!("{:.1}M /s", rate / 1_000_000.0)
    }
}

/// Truncate a string to a maximum length
#[allow(dead_code)]
pub fn truncate_string(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len.saturating_sub(3)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_json() {
        assert!(validate_json(r#"{"key": "value"}"#).is_ok());
        assert!(validate_json("invalid json").is_err());
    }

    #[test]
    fn test_parse_key_value_pairs() {
        let pairs = vec!["key1=value1".to_string(), "key2=value2".to_string()];
        let result = parse_key_value_pairs(&pairs).unwrap();
        assert_eq!(result.get("key1"), Some(&"value1".to_string()));
        assert_eq!(result.get("key2"), Some(&"value2".to_string()));
    }

    #[test]
    fn test_validate_disruptor_name() {
        assert!(validate_disruptor_name("valid-name_123").is_ok());
        assert!(validate_disruptor_name("").is_err());
        assert!(validate_disruptor_name("invalid name with spaces").is_err());
    }

    #[test]
    fn test_validate_buffer_size() {
        assert!(validate_buffer_size(1024).is_ok());
        assert!(validate_buffer_size(1023).is_err());
        assert!(validate_buffer_size(0).is_err());
    }

    #[test]
    fn test_format_duration() {
        assert_eq!(format_duration(Duration::from_secs(30)), "30s");
        assert_eq!(format_duration(Duration::from_secs(90)), "1m 30s");
        assert_eq!(format_duration(Duration::from_secs(3600)), "1h 0m");
        assert_eq!(format_duration(Duration::from_secs(3661)), "1h 1m 1s");
    }

    #[test]
    fn test_format_bytes() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(1024), "1.0 KB");
        assert_eq!(format_bytes(1536), "1.5 KB");
        assert_eq!(format_bytes(1048576), "1.0 MB");
    }

    #[test]
    fn test_format_rate() {
        assert_eq!(format_rate(100.0), "100.0 /s");
        assert_eq!(format_rate(1500.0), "1.5K /s");
        assert_eq!(format_rate(1500000.0), "1.5M /s");
    }

    #[test]
    fn test_truncate_string() {
        assert_eq!(truncate_string("short", 10), "short");
        assert_eq!(truncate_string("this is a very long string", 10), "this is...");
    }
}
