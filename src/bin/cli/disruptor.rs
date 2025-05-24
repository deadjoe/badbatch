//! Disruptor Command Handlers
//!
//! Command handlers for disruptor management operations.

use tabled::Tabled;

use crate::{DisruptorCommands, client::BadBatchClient};
use crate::cli::{CliResult, format, utils, progress};

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
        _ => return Err(crate::cli::CliError::invalid_input(
            "Producer type must be 'single' or 'multi'"
        )),
    };

    let wait_strategy = match wait_strategy.to_lowercase().as_str() {
        "blocking" | "busy-spin" | "yielding" | "sleeping" => wait_strategy.to_lowercase(),
        _ => return Err(crate::cli::CliError::invalid_input(
            "Wait strategy must be one of: blocking, busy-spin, yielding, sleeping"
        )),
    };

    let spinner = progress::create_spinner(&format!("Creating disruptor '{}'...", name));

    let request = crate::client::CreateDisruptorRequest {
        name: name.clone(),
        buffer_size: size,
        producer_type,
        wait_strategy,
    };

    let response = client.create_disruptor(request).await?;
    progress::finish_progress_with_message(&spinner, &format!("Disruptor '{}' created successfully", name));

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
    progress::finish_progress_with_message(&spinner, &format!("Disruptor '{}' deleted successfully", id));

    Ok(())
}

async fn start_disruptor(client: &BadBatchClient, id: &str) -> CliResult<()> {
    let spinner = progress::create_spinner(&format!("Starting disruptor '{}'...", id));
    client.start_disruptor(id).await?;
    progress::finish_progress_with_message(&spinner, &format!("Disruptor '{}' started successfully", id));

    Ok(())
}

async fn stop_disruptor(client: &BadBatchClient, id: &str) -> CliResult<()> {
    let spinner = progress::create_spinner(&format!("Stopping disruptor '{}'...", id));
    client.stop_disruptor(id).await?;
    progress::finish_progress_with_message(&spinner, &format!("Disruptor '{}' stopped successfully", id));

    Ok(())
}

async fn pause_disruptor(client: &BadBatchClient, id: &str) -> CliResult<()> {
    let spinner = progress::create_spinner(&format!("Pausing disruptor '{}'...", id));
    client.pause_disruptor(id).await?;
    progress::finish_progress_with_message(&spinner, &format!("Disruptor '{}' paused successfully", id));

    Ok(())
}

async fn resume_disruptor(client: &BadBatchClient, id: &str) -> CliResult<()> {
    let spinner = progress::create_spinner(&format!("Resuming disruptor '{}'...", id));
    client.resume_disruptor(id).await?;
    progress::finish_progress_with_message(&spinner, &format!("Disruptor '{}' resumed successfully", id));

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

impl From<crate::client::DisruptorInfo> for DisruptorTableRow {
    fn from(info: crate::client::DisruptorInfo) -> Self {
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
