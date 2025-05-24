//! Monitor Command Handlers
//!
//! Command handlers for monitoring and metrics operations.

use std::path::PathBuf;
use std::time::Duration;
use tokio::time::sleep;
use tabled::Tabled;
use serde_json::Value;

use crate::{MonitorCommands, client::BadBatchClient};
use crate::cli::{CliResult, format, utils};

/// Handle monitor commands
pub async fn handle_monitor_command(
    command: MonitorCommands,
    client: &BadBatchClient,
    output_format: &str,
) -> CliResult<()> {
    match command {
        MonitorCommands::System => show_system_metrics(client, output_format).await,

        MonitorCommands::Disruptor { id } => show_disruptor_metrics(client, &id, output_format).await,

        MonitorCommands::Cluster => show_cluster_metrics(client, output_format).await,

        MonitorCommands::Watch {
            interval,
            monitor_type,
            target,
        } => watch_metrics(client, interval, &monitor_type, target.as_deref(), output_format).await,

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

async fn show_cluster_metrics(client: &BadBatchClient, output_format: &str) -> CliResult<()> {
    // For now, show system metrics as cluster metrics
    // In a real implementation, this would fetch cluster-specific metrics
    let metrics = client.get_system_metrics().await?;

    let cluster_metrics = serde_json::json!({
        "cluster_status": "active",
        "node_count": 1,
        "healthy_nodes": 1,
        "system_metrics": metrics
    });

    let output = format::format_output(&cluster_metrics, output_format)?;
    println!("{}", output);

    Ok(())
}

async fn watch_metrics(
    client: &BadBatchClient,
    interval: u64,
    monitor_type: &str,
    target: Option<&str>,
    output_format: &str,
) -> CliResult<()> {
    println!("Watching {} metrics (refresh every {}s). Press Ctrl+C to stop.", monitor_type, interval);
    println!();

    let sleep_duration = Duration::from_secs(interval);

    loop {
        // Clear screen
        print!("\x1B[2J\x1B[1;1H");

        // Show timestamp
        println!("Last updated: {}", chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC"));
        println!();

        match monitor_type.to_lowercase().as_str() {
            "system" => {
                if let Err(e) = show_system_metrics(client, output_format).await {
                    eprintln!("Error fetching system metrics: {}", e);
                }
            }
            "disruptor" => {
                if let Some(disruptor_id) = target {
                    if let Err(e) = show_disruptor_metrics(client, disruptor_id, output_format).await {
                        eprintln!("Error fetching disruptor metrics: {}", e);
                    }
                } else {
                    eprintln!("Disruptor ID required for disruptor monitoring");
                }
            }
            "cluster" => {
                if let Err(e) = show_cluster_metrics(client, output_format).await {
                    eprintln!("Error fetching cluster metrics: {}", e);
                }
            }
            _ => {
                eprintln!("Unknown monitor type: {}", monitor_type);
                return Err(crate::cli::CliError::invalid_input(format!(
                    "Unknown monitor type: {}. Supported types: system, disruptor, cluster",
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
    println!("Exporting metrics to {:?} in {} format...", output_path, export_format);

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
        _ => return Err(crate::cli::CliError::invalid_input(format!(
            "Unsupported export format: {}. Supported formats: json, yaml, csv, prometheus",
            export_format
        ))),
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
            csv.push_str(&format!("system,uptime_seconds,{},{}\n", uptime, data["timestamp"]));
        }
        if let Some(memory) = system_metrics.get("memory_usage_bytes") {
            csv.push_str(&format!("system,memory_usage_bytes,{},{}\n", memory, data["timestamp"]));
        }
        if let Some(cpu) = system_metrics.get("cpu_usage_percent") {
            csv.push_str(&format!("system,cpu_usage_percent,{},{}\n", cpu, data["timestamp"]));
        }
    }

    Ok(csv)
}

fn export_to_prometheus(data: &Value) -> CliResult<String> {
    // Simple Prometheus export - in a real implementation, this would be more sophisticated
    let mut prometheus = String::new();

    if let Some(system_metrics) = data.get("system_metrics") {
        if let Some(uptime) = system_metrics.get("uptime_seconds") {
            prometheus.push_str(&format!("# HELP badbatch_uptime_seconds System uptime in seconds\n"));
            prometheus.push_str(&format!("# TYPE badbatch_uptime_seconds counter\n"));
            prometheus.push_str(&format!("badbatch_uptime_seconds {}\n", uptime));
        }

        if let Some(memory) = system_metrics.get("memory_usage_bytes") {
            prometheus.push_str(&format!("# HELP badbatch_memory_usage_bytes Memory usage in bytes\n"));
            prometheus.push_str(&format!("# TYPE badbatch_memory_usage_bytes gauge\n"));
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

impl From<crate::client::SystemMetrics> for SystemMetricsRow {
    fn from(metrics: crate::client::SystemMetrics) -> Self {
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

impl From<crate::client::DisruptorMetrics> for DisruptorMetricsRow {
    fn from(metrics: crate::client::DisruptorMetrics) -> Self {
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
