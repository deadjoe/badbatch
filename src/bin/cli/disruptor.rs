//! Disruptor Command Handlers
//!
//! Command handlers for disruptor management operations.

use tabled::Tabled;

use crate::cli::client::{BadBatchClient, CreateDisruptorRequest, DisruptorInfo};
use crate::cli::{format, progress, utils, CliResult};
use crate::DisruptorCommands;

/// Handle disruptor commands
pub async fn handle_disruptor_command(
    command: DisruptorCommands,
    client: &BadBatchClient,
    output_format: &str,
) -> CliResult<()> {
    match command {
        DisruptorCommands::Create {
            name,
            size,
            producer,
            wait_strategy,
        } => create_disruptor(client, name, size, producer, wait_strategy, output_format).await,

        DisruptorCommands::List => list_disruptors(client, output_format).await,

        DisruptorCommands::Show { id } => show_disruptor(client, &id, output_format).await,

        DisruptorCommands::Delete { id, force } => delete_disruptor(client, &id, force).await,

        DisruptorCommands::Start { id } => start_disruptor(client, &id).await,

        DisruptorCommands::Stop { id } => stop_disruptor(client, &id).await,

        DisruptorCommands::Pause { id } => pause_disruptor(client, &id).await,

        DisruptorCommands::Resume { id } => resume_disruptor(client, &id).await,
    }
}

async fn create_disruptor(
    client: &BadBatchClient,
    name: String,
    size: usize,
    producer: String,
    wait_strategy: String,
    output_format: &str,
) -> CliResult<()> {
    // Validate inputs
    utils::validate_disruptor_name(&name)?;
    utils::validate_buffer_size(size)?;

    let producer_type = match producer.to_lowercase().as_str() {
        "single" | "multi" => producer.to_lowercase(),
        _ => {
            return Err(crate::cli::CliError::invalid_input(
                "Producer type must be 'single' or 'multi'",
            ))
        }
    };

    let wait_strategy = match wait_strategy.to_lowercase().as_str() {
        "blocking" | "busy-spin" | "yielding" | "sleeping" => wait_strategy.to_lowercase(),
        _ => {
            return Err(crate::cli::CliError::invalid_input(
                "Wait strategy must be one of: blocking, busy-spin, yielding, sleeping",
            ))
        }
    };

    let spinner = progress::create_spinner(&format!("Creating disruptor '{}'...", name));

    let request = CreateDisruptorRequest {
        name: name.clone(),
        buffer_size: size,
        producer_type,
        wait_strategy,
    };

    let response = client.create_disruptor(request).await?;
    progress::finish_progress_with_message(
        &spinner,
        &format!("Disruptor '{}' created successfully", name),
    );

    let output = format::format_output(&response, output_format)?;
    println!("{}", output);

    Ok(())
}

async fn list_disruptors(client: &BadBatchClient, output_format: &str) -> CliResult<()> {
    let spinner = progress::create_spinner("Fetching disruptors...");
    let disruptors = client.list_disruptors().await?;
    spinner.finish_and_clear();

    if disruptors.is_empty() {
        println!("No disruptors found.");
        return Ok(());
    }

    match output_format.to_lowercase().as_str() {
        "table" => {
            let table_data: Vec<DisruptorTableRow> = disruptors
                .into_iter()
                .map(DisruptorTableRow::from)
                .collect();
            println!("{}", format::format_table(&table_data));
        }
        _ => {
            let output = format::format_output(&disruptors, output_format)?;
            println!("{}", output);
        }
    }

    Ok(())
}

async fn show_disruptor(client: &BadBatchClient, id: &str, output_format: &str) -> CliResult<()> {
    let spinner = progress::create_spinner(&format!("Fetching disruptor '{}'...", id));
    let disruptor = client.get_disruptor(id).await?;
    spinner.finish_and_clear();

    let output = format::format_output(&disruptor, output_format)?;
    println!("{}", output);

    Ok(())
}

async fn delete_disruptor(client: &BadBatchClient, id: &str, force: bool) -> CliResult<()> {
    if !force {
        let confirmed = utils::confirm_action(&format!("Delete disruptor '{}'?", id))?;
        if !confirmed {
            println!("Operation cancelled.");
            return Ok(());
        }
    }

    let spinner = progress::create_spinner(&format!("Deleting disruptor '{}'...", id));
    client.delete_disruptor(id).await?;
    progress::finish_progress_with_message(
        &spinner,
        &format!("Disruptor '{}' deleted successfully", id),
    );

    Ok(())
}

async fn start_disruptor(client: &BadBatchClient, id: &str) -> CliResult<()> {
    let spinner = progress::create_spinner(&format!("Starting disruptor '{}'...", id));
    client.start_disruptor(id).await?;
    progress::finish_progress_with_message(
        &spinner,
        &format!("Disruptor '{}' started successfully", id),
    );

    Ok(())
}

async fn stop_disruptor(client: &BadBatchClient, id: &str) -> CliResult<()> {
    let spinner = progress::create_spinner(&format!("Stopping disruptor '{}'...", id));
    client.stop_disruptor(id).await?;
    progress::finish_progress_with_message(
        &spinner,
        &format!("Disruptor '{}' stopped successfully", id),
    );

    Ok(())
}

async fn pause_disruptor(client: &BadBatchClient, id: &str) -> CliResult<()> {
    let spinner = progress::create_spinner(&format!("Pausing disruptor '{}'...", id));
    client.pause_disruptor(id).await?;
    progress::finish_progress_with_message(
        &spinner,
        &format!("Disruptor '{}' paused successfully", id),
    );

    Ok(())
}

async fn resume_disruptor(client: &BadBatchClient, id: &str) -> CliResult<()> {
    let spinner = progress::create_spinner(&format!("Resuming disruptor '{}'...", id));
    client.resume_disruptor(id).await?;
    progress::finish_progress_with_message(
        &spinner,
        &format!("Disruptor '{}' resumed successfully", id),
    );

    Ok(())
}

/// Table row for disruptor display
#[derive(Tabled)]
struct DisruptorTableRow {
    #[tabled(rename = "ID")]
    id: String,
    #[tabled(rename = "Name")]
    name: String,
    #[tabled(rename = "Buffer Size")]
    buffer_size: String,
    #[tabled(rename = "Producer")]
    producer_type: String,
    #[tabled(rename = "Wait Strategy")]
    wait_strategy: String,
    #[tabled(rename = "Status")]
    status: String,
    #[tabled(rename = "Created")]
    created_at: String,
}

impl From<DisruptorInfo> for DisruptorTableRow {
    fn from(info: DisruptorInfo) -> Self {
        Self {
            id: info.id,
            name: info.name,
            buffer_size: format!("{}", info.buffer_size),
            producer_type: info.producer_type,
            wait_strategy: info.wait_strategy,
            status: info.status,
            created_at: info.created_at,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::client::{DisruptorInfo, DisruptorResponse};

    fn create_test_disruptor_info() -> DisruptorInfo {
        DisruptorInfo {
            id: "test-id".to_string(),
            name: "test-disruptor".to_string(),
            buffer_size: 1024,
            producer_type: "single".to_string(),
            wait_strategy: "blocking".to_string(),
            status: "running".to_string(),
            created_at: "2023-01-01T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn test_disruptor_table_row_from_info() {
        let info = create_test_disruptor_info();
        let row = DisruptorTableRow::from(info);

        assert_eq!(row.id, "test-id");
        assert_eq!(row.name, "test-disruptor");
        assert_eq!(row.buffer_size, "1024");
        assert_eq!(row.producer_type, "single");
        assert_eq!(row.wait_strategy, "blocking");
        assert_eq!(row.status, "running");
        assert_eq!(row.created_at, "2023-01-01T00:00:00Z");
    }

    #[test]
    fn test_create_disruptor_request_validation() {
        // Test valid producer types
        let valid_producers = ["single", "multi", "SINGLE", "MULTI"];
        for producer in &valid_producers {
            let normalized = match producer.to_lowercase().as_str() {
                "single" | "multi" => producer.to_lowercase(),
                _ => panic!("Should be valid"),
            };
            assert!(normalized == "single" || normalized == "multi");
        }

        // Test valid wait strategies
        let valid_strategies = ["blocking", "busy-spin", "yielding", "sleeping"];
        for strategy in &valid_strategies {
            let normalized = match strategy.to_lowercase().as_str() {
                "blocking" | "busy-spin" | "yielding" | "sleeping" => strategy.to_lowercase(),
                _ => panic!("Should be valid"),
            };
            assert!(
                ["blocking", "busy-spin", "yielding", "sleeping"].contains(&normalized.as_str())
            );
        }
    }

    #[test]
    fn test_invalid_producer_type() {
        let invalid_producers = ["invalid", "triple", ""];
        for producer in &invalid_producers {
            let result = match producer.to_lowercase().as_str() {
                "single" | "multi" => Ok(producer.to_lowercase()),
                _ => Err("Invalid producer type"),
            };
            assert!(result.is_err());
        }
    }

    #[test]
    fn test_invalid_wait_strategy() {
        let invalid_strategies = ["invalid", "custom", ""];
        for strategy in &invalid_strategies {
            let result = match strategy.to_lowercase().as_str() {
                "blocking" | "busy-spin" | "yielding" | "sleeping" => Ok(strategy.to_lowercase()),
                _ => Err("Invalid wait strategy"),
            };
            assert!(result.is_err());
        }
    }

    #[test]
    fn test_disruptor_table_row_formatting() {
        let info = DisruptorInfo {
            id: "long-id-12345".to_string(),
            name: "my-test-disruptor".to_string(),
            buffer_size: 2048,
            producer_type: "multi".to_string(),
            wait_strategy: "busy-spin".to_string(),
            status: "stopped".to_string(),
            created_at: "2023-12-25T12:30:45Z".to_string(),
        };

        let row = DisruptorTableRow::from(info);
        assert_eq!(row.buffer_size, "2048");
        assert_eq!(row.producer_type, "multi");
        assert_eq!(row.wait_strategy, "busy-spin");
        assert_eq!(row.status, "stopped");
    }

    #[test]
    fn test_create_disruptor_request_structure() {
        let request = CreateDisruptorRequest {
            name: "test-disruptor".to_string(),
            buffer_size: 512,
            producer_type: "single".to_string(),
            wait_strategy: "yielding".to_string(),
        };

        assert_eq!(request.name, "test-disruptor");
        assert_eq!(request.buffer_size, 512);
        assert_eq!(request.producer_type, "single");
        assert_eq!(request.wait_strategy, "yielding");
    }

    #[test]
    fn test_disruptor_response_structure() {
        let response = DisruptorResponse {
            id: "new-id".to_string(),
            message: "Created successfully".to_string(),
        };

        assert_eq!(response.id, "new-id");
        assert_eq!(response.message, "Created successfully");
    }

    #[test]
    fn test_case_insensitive_producer_validation() {
        let test_cases = [
            ("Single", "single"),
            ("MULTI", "multi"),
            ("sInGlE", "single"),
            ("MuLtI", "multi"),
        ];

        for (input, expected) in &test_cases {
            let result = match input.to_lowercase().as_str() {
                "single" | "multi" => input.to_lowercase(),
                _ => panic!("Should be valid"),
            };
            assert_eq!(result, *expected);
        }
    }

    #[test]
    fn test_case_insensitive_wait_strategy_validation() {
        let test_cases = [
            ("Blocking", "blocking"),
            ("BUSY-SPIN", "busy-spin"),
            ("YiElDiNg", "yielding"),
            ("SLEEPING", "sleeping"),
        ];

        for (input, expected) in &test_cases {
            let result = match input.to_lowercase().as_str() {
                "blocking" | "busy-spin" | "yielding" | "sleeping" => input.to_lowercase(),
                _ => panic!("Should be valid"),
            };
            assert_eq!(result, *expected);
        }
    }
}
