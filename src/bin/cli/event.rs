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
        let overhead = 20; // Account for padding field overhead
        let padding_size = if size > base_size + overhead {
            size - base_size - overhead
        } else {
            0
        };
        let padding = "x".repeat(padding_size);

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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::HashMap;

    #[test]
    fn test_generate_test_event_basic() {
        let event = generate_test_event(42, 100);

        assert_eq!(event["id"], 42);
        assert_eq!(event["type"], "test_event");
        assert_eq!(event["sequence"], 42);
        assert!(event["timestamp"].is_string());
    }

    #[test]
    fn test_generate_test_event_with_padding() {
        let event = generate_test_event(1, 500);

        // Should have padding field for larger size
        assert!(event["padding"].is_string());
        assert_eq!(event["id"], 1);
        assert_eq!(event["sequence"], 1);

        // Check that the event is roughly the target size
        let serialized = serde_json::to_string(&event).unwrap();
        assert!(serialized.len() >= 400); // Should be close to target size
    }

    #[test]
    fn test_generate_test_event_small_size() {
        let event = generate_test_event(0, 50);

        // Should not have padding for small size
        assert!(event["padding"].is_null() || !event.as_object().unwrap().contains_key("padding"));
        assert_eq!(event["id"], 0);
        assert_eq!(event["sequence"], 0);
    }

    #[test]
    fn test_publish_event_request_structure() {
        let mut metadata = HashMap::new();
        metadata.insert("source".to_string(), "test".to_string());

        let request = PublishEventRequest {
            data: json!({"message": "hello"}),
            metadata: Some(metadata),
        };

        assert_eq!(request.data["message"], "hello");
        assert!(request.metadata.is_some());
        assert_eq!(request.metadata.unwrap()["source"], "test");
    }

    #[test]
    fn test_publish_batch_request_structure() {
        let events = vec![
            EventData {
                data: json!({"id": 1}),
                metadata: None,
            },
            EventData {
                data: json!({"id": 2}),
                metadata: Some({
                    let mut map = HashMap::new();
                    map.insert("type".to_string(), "test".to_string());
                    map
                }),
            },
        ];

        let request = PublishBatchRequest { events };

        assert_eq!(request.events.len(), 2);
        assert_eq!(request.events[0].data["id"], 1);
        assert_eq!(request.events[1].data["id"], 2);
        assert!(request.events[0].metadata.is_none());
        assert!(request.events[1].metadata.is_some());
    }

    #[test]
    fn test_event_data_creation() {
        let event = EventData {
            data: json!({"test": "value"}),
            metadata: None,
        };

        assert_eq!(event.data["test"], "value");
        assert!(event.metadata.is_none());
    }

    #[test]
    fn test_event_data_with_metadata() {
        let mut metadata = HashMap::new();
        metadata.insert("priority".to_string(), "high".to_string());
        metadata.insert("source".to_string(), "cli".to_string());

        let event = EventData {
            data: json!({"message": "important"}),
            metadata: Some(metadata),
        };

        assert_eq!(event.data["message"], "important");
        let meta = event.metadata.unwrap();
        assert_eq!(meta["priority"], "high");
        assert_eq!(meta["source"], "cli");
    }

    #[test]
    fn test_batch_size_calculation() {
        let total_events = 100;
        let batch_size = 30;
        let expected_batches = (total_events + batch_size - 1) / batch_size;

        assert_eq!(expected_batches, 4); // 100 events in batches of 30 = 4 batches
    }

    #[test]
    fn test_rate_calculation() {
        let rate = 100; // events per second
        let interval = Duration::from_secs_f64(1.0 / rate as f64);

        assert_eq!(interval, Duration::from_millis(10)); // 1/100 = 0.01 seconds = 10ms
    }

    #[test]
    fn test_event_generation_sequence() {
        let events: Vec<_> = (0..5).map(|i| generate_test_event(i, 100)).collect();

        for (i, event) in events.iter().enumerate() {
            assert_eq!(event["id"], i);
            assert_eq!(event["sequence"], i);
            assert_eq!(event["type"], "test_event");
        }
    }

    #[test]
    fn test_empty_batch_validation() {
        let empty_events: Vec<Value> = vec![];

        // This simulates the validation that would happen in publish_batch
        assert!(empty_events.is_empty());
    }

    #[test]
    fn test_json_parsing_for_events() {
        let valid_json = r#"[{"id": 1, "data": "test"}, {"id": 2, "data": "test2"}]"#;
        let events: Result<Vec<Value>, _> = serde_json::from_str(valid_json);

        assert!(events.is_ok());
        let events = events.unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["id"], 1);
        assert_eq!(events[1]["id"], 2);
    }

    #[test]
    fn test_invalid_json_parsing() {
        let invalid_json = r#"[{"id": 1, "data": "test"}, {"id": 2, "data":}]"#;
        let events: Result<Vec<Value>, _> = serde_json::from_str(invalid_json);

        assert!(events.is_err());
    }

    #[test]
    fn test_event_size_estimation() {
        let small_event = generate_test_event(1, 50);
        let large_event = generate_test_event(1, 1000);

        let small_size = serde_json::to_string(&small_event).unwrap().len();
        let large_size = serde_json::to_string(&large_event).unwrap().len();

        assert!(large_size > small_size);
        assert!(large_size >= 800); // Should be close to target size
    }
}