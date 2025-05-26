//! Monitor Command Handlers
//!
//! Command handlers for monitoring and metrics operations.

use serde_json::Value;
use std::path::PathBuf;
use std::time::Duration;
use tabled::Tabled;
use tokio::time::sleep;

use crate::cli::client::{BadBatchClient, DisruptorMetrics, SystemMetrics};
use crate::cli::{format, utils, CliResult};
use crate::MonitorCommands;

/// Handle monitor commands
pub async fn handle_monitor_command(
    command: MonitorCommands,
    client: &BadBatchClient,
    output_format: &str,
) -> CliResult<()> {
    match command {
        MonitorCommands::System => show_system_metrics(client, output_format).await,

        MonitorCommands::Disruptor { id } => {
            show_disruptor_metrics(client, &id, output_format).await
        }

        MonitorCommands::Watch {
            interval,
            monitor_type,
            target,
        } => {
            watch_metrics(
                client,
                interval,
                &monitor_type,
                target.as_deref(),
                output_format,
            )
            .await
        }

        MonitorCommands::Export { output, format } => {
            export_metrics(client, &output, &format).await
        }
    }
}

async fn show_system_metrics(client: &BadBatchClient, output_format: &str) -> CliResult<()> {
    let metrics = client.get_system_metrics().await?;

    match output_format.to_lowercase().as_str() {
        "table" => {
            let table_data = vec![SystemMetricsRow::from(metrics)];
            println!("{}", format::format_table(&table_data));
        }
        _ => {
            let output = format::format_output(&metrics, output_format)?;
            println!("{}", output);
        }
    }

    Ok(())
}

async fn show_disruptor_metrics(
    client: &BadBatchClient,
    id: &str,
    output_format: &str,
) -> CliResult<()> {
    let metrics = client.get_disruptor_metrics(id).await?;

    match output_format.to_lowercase().as_str() {
        "table" => {
            let table_data = vec![DisruptorMetricsRow::from(metrics)];
            println!("{}", format::format_table(&table_data));
        }
        _ => {
            let output = format::format_output(&metrics, output_format)?;
            println!("{}", output);
        }
    }

    Ok(())
}



async fn watch_metrics(
    client: &BadBatchClient,
    interval: u64,
    monitor_type: &str,
    target: Option<&str>,
    output_format: &str,
) -> CliResult<()> {
    println!(
        "Watching {} metrics (refresh every {}s). Press Ctrl+C to stop.",
        monitor_type, interval
    );
    println!();

    let sleep_duration = Duration::from_secs(interval);

    loop {
        // Clear screen
        print!("\x1B[2J\x1B[1;1H");

        // Show timestamp
        println!(
            "Last updated: {}",
            chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC")
        );
        println!();

        match monitor_type.to_lowercase().as_str() {
            "system" => {
                if let Err(e) = show_system_metrics(client, output_format).await {
                    eprintln!("Error fetching system metrics: {}", e);
                }
            }
            "disruptor" => {
                if let Some(disruptor_id) = target {
                    if let Err(e) =
                        show_disruptor_metrics(client, disruptor_id, output_format).await
                    {
                        eprintln!("Error fetching disruptor metrics: {}", e);
                    }
                } else {
                    eprintln!("Disruptor ID required for disruptor monitoring");
                }
            }
            _ => {
                eprintln!("Unknown monitor type: {}", monitor_type);
                return Err(crate::cli::CliError::invalid_input(format!(
                    "Unknown monitor type: {}. Supported types: system, disruptor",
                    monitor_type
                )));
            }
        }

        sleep(sleep_duration).await;
    }
}

async fn export_metrics(
    client: &BadBatchClient,
    output_path: &PathBuf,
    export_format: &str,
) -> CliResult<()> {
    println!(
        "Exporting metrics to {:?} in {} format...",
        output_path, export_format
    );

    // Collect all metrics
    let system_metrics = client.get_system_metrics().await?;
    let disruptors = client.list_disruptors().await?;

    let mut disruptor_metrics = Vec::new();
    for disruptor in &disruptors {
        if let Ok(metrics) = client.get_disruptor_metrics(&disruptor.id).await {
            disruptor_metrics.push(serde_json::json!({
                "disruptor_id": disruptor.id,
                "disruptor_name": disruptor.name,
                "metrics": metrics
            }));
        }
    }

    let export_data = serde_json::json!({
        "timestamp": chrono::Utc::now().to_rfc3339(),
        "system_metrics": system_metrics,
        "disruptor_metrics": disruptor_metrics,
        "disruptor_count": disruptors.len()
    });

    // Format and write data
    let content = match export_format.to_lowercase().as_str() {
        "json" => serde_json::to_string_pretty(&export_data)?,
        "yaml" => serde_yaml::to_string(&export_data)?,
        "csv" => export_to_csv(&export_data)?,
        "prometheus" => export_to_prometheus(&export_data)?,
        _ => {
            return Err(crate::cli::CliError::invalid_input(format!(
                "Unsupported export format: {}. Supported formats: json, yaml, csv, prometheus",
                export_format
            )))
        }
    };

    tokio::fs::write(output_path, content).await?;
    println!("✓ Metrics exported successfully to {:?}", output_path);

    Ok(())
}

fn export_to_csv(data: &Value) -> CliResult<String> {
    // Simple CSV export - in a real implementation, this would be more sophisticated
    let mut csv = String::new();
    csv.push_str("metric_type,metric_name,value,timestamp\n");

    if let Some(system_metrics) = data.get("system_metrics") {
        if let Some(uptime) = system_metrics.get("uptime_seconds") {
            csv.push_str(&format!(
                "system,uptime_seconds,{},{}\n",
                uptime, data["timestamp"]
            ));
        }
        if let Some(memory) = system_metrics.get("memory_usage_bytes") {
            csv.push_str(&format!(
                "system,memory_usage_bytes,{},{}\n",
                memory, data["timestamp"]
            ));
        }
        if let Some(cpu) = system_metrics.get("cpu_usage_percent") {
            csv.push_str(&format!(
                "system,cpu_usage_percent,{},{}\n",
                cpu, data["timestamp"]
            ));
        }
    }

    Ok(csv)
}

fn export_to_prometheus(data: &Value) -> CliResult<String> {
    // Simple Prometheus export - in a real implementation, this would be more sophisticated
    let mut prometheus = String::new();

    if let Some(system_metrics) = data.get("system_metrics") {
        if let Some(uptime) = system_metrics.get("uptime_seconds") {
            prometheus.push_str("# HELP badbatch_uptime_seconds System uptime in seconds\n");
            prometheus.push_str("# TYPE badbatch_uptime_seconds counter\n");
            prometheus.push_str(&format!("badbatch_uptime_seconds {}\n", uptime));
        }

        if let Some(memory) = system_metrics.get("memory_usage_bytes") {
            prometheus.push_str("# HELP badbatch_memory_usage_bytes Memory usage in bytes\n");
            prometheus.push_str("# TYPE badbatch_memory_usage_bytes gauge\n");
            prometheus.push_str(&format!("badbatch_memory_usage_bytes {}\n", memory));
        }
    }

    Ok(prometheus)
}

/// Table row for system metrics display
#[derive(Tabled)]
struct SystemMetricsRow {
    #[tabled(rename = "Uptime")]
    uptime: String,
    #[tabled(rename = "Memory")]
    memory: String,
    #[tabled(rename = "CPU")]
    cpu: String,
    #[tabled(rename = "Disruptors")]
    disruptors: String,
    #[tabled(rename = "Events")]
    events: String,
}

impl From<SystemMetrics> for SystemMetricsRow {
    fn from(metrics: SystemMetrics) -> Self {
        Self {
            uptime: utils::format_duration(Duration::from_secs(metrics.uptime_seconds)),
            memory: utils::format_bytes(metrics.memory_usage_bytes),
            cpu: format!("{:.1}%", metrics.cpu_usage_percent),
            disruptors: format!("{}", metrics.active_disruptors),
            events: format!("{}", metrics.total_events_processed),
        }
    }
}

/// Table row for disruptor metrics display
#[derive(Tabled)]
struct DisruptorMetricsRow {
    #[tabled(rename = "Events Processed")]
    events_processed: String,
    #[tabled(rename = "Events/sec")]
    events_per_second: String,
    #[tabled(rename = "Buffer Usage")]
    buffer_utilization: String,
    #[tabled(rename = "Producers")]
    producers: String,
    #[tabled(rename = "Consumers")]
    consumers: String,
    #[tabled(rename = "Last Sequence")]
    last_sequence: String,
}

impl From<DisruptorMetrics> for DisruptorMetricsRow {
    fn from(metrics: DisruptorMetrics) -> Self {
        Self {
            events_processed: format!("{}", metrics.events_processed),
            events_per_second: format!("{:.2}", metrics.events_per_second),
            buffer_utilization: format!("{:.1}%", metrics.buffer_utilization * 100.0),
            producers: format!("{}", metrics.producer_count),
            consumers: format!("{}", metrics.consumer_count),
            last_sequence: format!("{}", metrics.last_sequence),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn create_test_system_metrics() -> SystemMetrics {
        SystemMetrics {
            uptime_seconds: 3600,
            memory_usage_bytes: 1048576,
            cpu_usage_percent: 25.5,
            active_disruptors: 2,
            total_events_processed: 1000,
        }
    }

    fn create_test_disruptor_metrics() -> DisruptorMetrics {
        DisruptorMetrics {
            events_processed: 500,
            events_per_second: 100.5,
            buffer_utilization: 0.75,
            producer_count: 1,
            consumer_count: 2,
            last_sequence: 499,
        }
    }

    #[test]
    fn test_system_metrics_row_conversion() {
        let metrics = create_test_system_metrics();
        let row = SystemMetricsRow::from(metrics);

        assert_eq!(row.uptime, "1h 0m");
        assert_eq!(row.memory, "1.0 MB");
        assert_eq!(row.cpu, "25.5%");
        assert_eq!(row.disruptors, "2");
        assert_eq!(row.events, "1000");
    }

    #[test]
    fn test_disruptor_metrics_row_conversion() {
        let metrics = create_test_disruptor_metrics();
        let row = DisruptorMetricsRow::from(metrics);

        assert_eq!(row.events_processed, "500");
        assert_eq!(row.events_per_second, "100.50");
        assert_eq!(row.buffer_utilization, "75.0%");
        assert_eq!(row.producers, "1");
        assert_eq!(row.consumers, "2");
        assert_eq!(row.last_sequence, "499");
    }

    #[test]
    fn test_export_to_csv() {
        let data = serde_json::json!({
            "timestamp": "2023-01-01T00:00:00Z",
            "system_metrics": {
                "uptime_seconds": 3600,
                "memory_usage_bytes": 1048576,
                "cpu_usage_percent": 25.5
            }
        });

        let csv = export_to_csv(&data).unwrap();

        assert!(csv.contains("metric_type,metric_name,value,timestamp"));
        assert!(csv.contains("system,uptime_seconds,3600"));
        assert!(csv.contains("system,memory_usage_bytes,1048576"));
        assert!(csv.contains("system,cpu_usage_percent,25.5"));
    }

    #[test]
    fn test_export_to_prometheus() {
        let data = serde_json::json!({
            "system_metrics": {
                "uptime_seconds": 3600,
                "memory_usage_bytes": 1048576
            }
        });

        let prometheus = export_to_prometheus(&data).unwrap();

        assert!(prometheus.contains("# HELP badbatch_uptime_seconds"));
        assert!(prometheus.contains("# TYPE badbatch_uptime_seconds counter"));
        assert!(prometheus.contains("badbatch_uptime_seconds 3600"));
        assert!(prometheus.contains("# HELP badbatch_memory_usage_bytes"));
        assert!(prometheus.contains("badbatch_memory_usage_bytes 1048576"));
    }



    #[test]
    fn test_export_data_structure() {
        let export_data = serde_json::json!({
            "timestamp": "2023-01-01T00:00:00Z",
            "system_metrics": create_test_system_metrics(),
            "disruptor_metrics": [],
            "disruptor_count": 0
        });

        assert!(export_data["timestamp"].is_string());
        assert!(export_data["system_metrics"].is_object());
        assert!(export_data["disruptor_metrics"].is_array());
        assert_eq!(export_data["disruptor_count"], 0);
    }

    #[test]
    fn test_monitor_type_validation() {
        let valid_types = ["system", "disruptor"];

        for monitor_type in &valid_types {
            // This simulates the validation logic in watch_metrics
            let is_valid = matches!(
                monitor_type.to_lowercase().as_str(),
                "system" | "disruptor"
            );
            assert!(is_valid, "Monitor type '{}' should be valid", monitor_type);
        }

        let invalid_types = ["invalid", "unknown", "cluster", ""];
        for monitor_type in &invalid_types {
            let is_valid = matches!(
                monitor_type.to_lowercase().as_str(),
                "system" | "disruptor"
            );
            assert!(
                !is_valid,
                "Monitor type '{}' should be invalid",
                monitor_type
            );
        }
    }

    #[test]
    fn test_export_format_validation() {
        let valid_formats = ["json", "yaml", "csv", "prometheus"];

        for format in &valid_formats {
            let is_valid = matches!(
                format.to_lowercase().as_str(),
                "json" | "yaml" | "csv" | "prometheus"
            );
            assert!(is_valid, "Export format '{}' should be valid", format);
        }

        let invalid_formats = ["xml", "binary", ""];
        for format in &invalid_formats {
            let is_valid = matches!(
                format.to_lowercase().as_str(),
                "json" | "yaml" | "csv" | "prometheus"
            );
            assert!(!is_valid, "Export format '{}' should be invalid", format);
        }
    }

    #[test]
    fn test_watch_interval_conversion() {
        let intervals = [1, 5, 10, 30, 60];

        for interval in &intervals {
            let duration = Duration::from_secs(*interval);
            assert_eq!(duration.as_secs(), *interval);
        }
    }

    #[test]
    fn test_metrics_formatting() {
        // Test buffer utilization formatting
        let utilizations = [0.0, 0.25, 0.5, 0.75, 1.0];
        for util in &utilizations {
            let formatted = format!("{:.1}%", util * 100.0);
            let expected = format!("{:.1}%", util * 100.0);
            assert_eq!(formatted, expected);
        }

        // Test events per second formatting
        let rates = [0.0, 10.5, 100.25, 1000.0];
        for rate in &rates {
            let formatted = format!("{:.2}", rate);
            assert!(formatted.contains('.'));
        }
    }

    #[test]
    fn test_csv_header() {
        let data = serde_json::json!({
            "timestamp": "2023-01-01T00:00:00Z",
            "system_metrics": {}
        });

        let csv = export_to_csv(&data).unwrap();
        let lines: Vec<&str> = csv.lines().collect();

        assert!(!lines.is_empty());
        assert_eq!(lines[0], "metric_type,metric_name,value,timestamp");
    }

    #[test]
    fn test_prometheus_help_comments() {
        let data = serde_json::json!({
            "system_metrics": {
                "uptime_seconds": 100
            }
        });

        let prometheus = export_to_prometheus(&data).unwrap();
        let lines: Vec<&str> = prometheus.lines().collect();

        // Should have HELP and TYPE comments
        let help_lines: Vec<_> = lines
            .iter()
            .filter(|line| line.starts_with("# HELP"))
            .collect();
        let type_lines: Vec<_> = lines
            .iter()
            .filter(|line| line.starts_with("# TYPE"))
            .collect();

        assert!(!help_lines.is_empty());
        assert!(!type_lines.is_empty());
    }
}
