//! Event Command Handlers
//!
//! Command handlers for event publishing operations.

use std::path::PathBuf;
use std::time::{Duration, Instant};
use tokio::fs;
use tokio::time::sleep;
use serde_json::Value;

use crate::{EventCommands};
use crate::cli::client::{BadBatchClient, PublishEventRequest, PublishBatchRequest, EventData};
use crate::cli::{CliResult, format, utils, progress};

/// Handle event commands
pub async fn handle_event_command(
    command: EventCommands,
    client: &BadBatchClient,
    output_format: &str,
) -> CliResult<()> {
    match command {
        EventCommands::Publish {
            disruptor,
            data,
            metadata,
        } => publish_event(client, &disruptor, &data, &metadata, output_format).await,

        EventCommands::PublishFile {
            disruptor,
            file,
            batch_size,
        } => publish_file(client, &disruptor, &file, batch_size, output_format).await,

        EventCommands::Batch { disruptor, data } => {
            publish_batch(client, &disruptor, &data, output_format).await
        }

        EventCommands::Generate {
            disruptor,
            count,
            rate,
            size,
        } => generate_events(client, &disruptor, count, rate, size).await,
    }
}

async fn publish_event(
    client: &BadBatchClient,
    disruptor: &str,
    data: &str,
    metadata: &[String],
    output_format: &str,
) -> CliResult<()> {
    // Parse JSON data
    let event_data = utils::validate_json(data)?;

    // Parse metadata
    let metadata_map = if metadata.is_empty() {
        None
    } else {
        Some(utils::parse_key_value_pairs(metadata)?)
    };

    let spinner = progress::create_spinner("Publishing event...");

    let request = PublishEventRequest {
        data: event_data,
        metadata: metadata_map,
    };

    let response = client.publish_event(disruptor, request).await?;
    spinner.finish_with_message("✓ Event published successfully");

    let output = format::format_output(&response, output_format)?;
    println!("{}", output);

    Ok(())
}

async fn publish_file(
    client: &BadBatchClient,
    disruptor: &str,
    file: &PathBuf,
    batch_size: usize,
    output_format: &str,
) -> CliResult<()> {
    // Read file content
    let content = fs::read_to_string(file).await?;

    // Parse as JSON array
    let events: Vec<Value> = serde_json::from_str(&content)?;

    if events.is_empty() {
        println!("No events found in file.");
        return Ok(());
    }

    println!("Publishing {} events from file in batches of {}...", events.len(), batch_size);

    let total_batches = (events.len() + batch_size - 1) / batch_size;
    let progress_bar = progress::create_progress_bar(total_batches as u64, "Publishing batches");

    let mut total_published = 0;
    let start_time = Instant::now();

    for (_batch_idx, chunk) in events.chunks(batch_size).enumerate() {
        let batch_events: Vec<EventData> = chunk
            .iter()
            .map(|data| EventData {
                data: data.clone(),
                metadata: None,
            })
            .collect();

        let request = PublishBatchRequest {
            events: batch_events,
        };

        let response = client.publish_batch(disruptor, request).await?;
        total_published += response.count;

        progress_bar.inc(1);
        progress_bar.set_message(format!("Published {} events", total_published));
    }

    let elapsed = start_time.elapsed();
    progress_bar.finish_with_message(format!(
        "✓ Published {} events in {} ({:.2} events/sec)",
        total_published,
        utils::format_duration(elapsed),
        total_published as f64 / elapsed.as_secs_f64()
    ));

    let summary = serde_json::json!({
        "total_events": total_published,
        "batches": total_batches,
        "duration_seconds": elapsed.as_secs_f64(),
        "events_per_second": total_published as f64 / elapsed.as_secs_f64()
    });

    let output = format::format_output(&summary, output_format)?;
    println!("{}", output);

    Ok(())
}

async fn publish_batch(
    client: &BadBatchClient,
    disruptor: &str,
    data: &str,
    output_format: &str,
) -> CliResult<()> {
    // Parse JSON array
    let events: Vec<Value> = serde_json::from_str(data)?;

    if events.is_empty() {
        return Err(crate::cli::CliError::invalid_input("Event array cannot be empty"));
    }

    let spinner = progress::create_spinner(&format!("Publishing {} events...", events.len()));

    let batch_events: Vec<EventData> = events
        .into_iter()
        .map(|data| EventData {
            data,
            metadata: None,
        })
        .collect();

    let request = PublishBatchRequest {
        events: batch_events,
    };

    let response = client.publish_batch(disruptor, request).await?;
    progress::finish_progress_with_message(&spinner, &format!("Published {} events successfully", response.count));

    let output = format::format_output(&response, output_format)?;
    println!("{}", output);

    Ok(())
}

async fn generate_events(
    client: &BadBatchClient,
    disruptor: &str,
    count: usize,
    rate: usize,
    size: usize,
) -> CliResult<()> {
    println!("Generating {} events at {} events/sec with {} bytes each...", count, rate, size);

    let progress_bar = progress::create_progress_bar(count as u64, "Generating events");
    let start_time = Instant::now();

    let interval = Duration::from_secs_f64(1.0 / rate as f64);
    let mut published = 0;
    let mut last_time = Instant::now();

    for i in 0..count {
        // Generate event data
        let event_data = generate_test_event(i, size);

        let request = PublishEventRequest {
            data: event_data,
            metadata: None,
        };

        // Publish event
        match client.publish_event(disruptor, request).await {
            Ok(_) => {
                published += 1;
                progress_bar.inc(1);
            }
            Err(e) => {
                eprintln!("Failed to publish event {}: {}", i, e);
            }
        }

        // Rate limiting
        let elapsed = last_time.elapsed();
        if elapsed < interval {
            sleep(interval - elapsed).await;
        }
        last_time = Instant::now();
    }

    let total_elapsed = start_time.elapsed();
    progress_bar.finish_with_message(format!(
        "✓ Generated {} events in {} ({:.2} events/sec)",
        published,
        utils::format_duration(total_elapsed),
        published as f64 / total_elapsed.as_secs_f64()
    ));

    let summary = serde_json::json!({
        "generated_events": published,
        "failed_events": count - published,
        "duration_seconds": total_elapsed.as_secs_f64(),
        "actual_rate": published as f64 / total_elapsed.as_secs_f64(),
        "target_rate": rate
    });

    println!("{}", serde_json::to_string_pretty(&summary)?);

    Ok(())
}

fn generate_test_event(index: usize, size: usize) -> Value {
    let base_data = serde_json::json!({
        "id": index,
        "timestamp": chrono::Utc::now().to_rfc3339(),
        "type": "test_event",
        "sequence": index
    });

    // Add padding to reach desired size
    let base_size = serde_json::to_string(&base_data).unwrap().len();
    if size > base_size {
        let padding_size = size - base_size - 20; // Account for padding field overhead
        let padding = "x".repeat(padding_size.max(0));

        serde_json::json!({
            "id": index,
            "timestamp": chrono::Utc::now().to_rfc3339(),
            "type": "test_event",
            "sequence": index,
            "padding": padding
        })
    } else {
        base_data
    }
}
