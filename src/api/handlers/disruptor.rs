//! Disruptor Handlers
//!
//! This module provides handlers for Disruptor lifecycle management operations
//! including creation, deletion, starting, stopping, and status queries.

use axum::{
    extract::{Path, Query, Json as ExtractJson},
    response::Json,
};
use uuid::Uuid;
use std::collections::HashMap;

use crate::api::{
    ApiResponse,
    models::{
        CreateDisruptorRequest, CreateDisruptorResponse, DisruptorInfo,
        DisruptorList, DisruptorStatus, ListDisruptorsQuery, DisruptorConfig,
    },
    handlers::{ApiResult, ApiError},
};
use crate::disruptor::ProducerType;

/// Create a new Disruptor instance
pub async fn create_disruptor(
    ExtractJson(request): ExtractJson<CreateDisruptorRequest>,
) -> ApiResult<Json<ApiResponse<CreateDisruptorResponse>>> {
    // Validate request
    validate_create_request(&request)?;

    // Generate unique ID for the Disruptor
    let disruptor_id = Uuid::new_v4().to_string();

    // Parse producer type
    let _producer_type = match request.producer_type.to_lowercase().as_str() {
        "single" => ProducerType::Single,
        "multi" => ProducerType::Multi,
        _ => return Err(ApiError::invalid_request("Invalid producer_type. Must be 'single' or 'multi'")),
    };

    // Parse wait strategy (placeholder - in real implementation would create actual strategy)
    let _wait_strategy = match request.wait_strategy.to_lowercase().as_str() {
        "blocking" | "yielding" | "busy_spin" | "sleeping" => {
            // Valid strategy names
        },
        _ => return Err(ApiError::invalid_request(
            "Invalid wait_strategy. Must be 'blocking', 'yielding', 'busy_spin', or 'sleeping'"
        )),
    };

    // Create the Disruptor instance
    // Note: This is a simplified implementation. In a real system, you would:
    // 1. Store the Disruptor instance in a manager
    // 2. Set up event handlers based on configuration
    // 3. Handle the lifecycle properly

    let disruptor_info = DisruptorInfo {
        id: disruptor_id.clone(),
        name: request.name.clone(),
        buffer_size: request.buffer_size,
        producer_type: request.producer_type.clone(),
        wait_strategy: request.wait_strategy.clone(),
        status: DisruptorStatus::Created,
        config: request.config.clone(),
        created_at: chrono::Utc::now(),
        last_activity: chrono::Utc::now(),
    };

    // Store the Disruptor (placeholder - would use a real storage mechanism)
    store_disruptor_info(&disruptor_info)?;

    let response = CreateDisruptorResponse {
        id: disruptor_id,
        config: disruptor_info,
        created_at: chrono::Utc::now(),
    };

    Ok(Json(ApiResponse::success(response)))
}

/// List all Disruptor instances
pub async fn list_disruptors(
    Query(query): Query<ListDisruptorsQuery>,
) -> ApiResult<Json<ApiResponse<DisruptorList>>> {
    // Get all Disruptor instances (placeholder implementation)
    let all_disruptors = get_all_disruptors()?;

    // Apply filters
    let mut filtered_disruptors = all_disruptors;

    if let Some(status_filter) = &query.status {
        filtered_disruptors.retain(|d| {
            match (status_filter.to_lowercase().as_str(), &d.status) {
                ("created", DisruptorStatus::Created) => true,
                ("running", DisruptorStatus::Running) => true,
                ("paused", DisruptorStatus::Paused) => true,
                ("stopping", DisruptorStatus::Stopping) => true,
                ("stopped", DisruptorStatus::Stopped) => true,
                ("error", DisruptorStatus::Error) => true,
                _ => false,
            }
        });
    }

    if let Some(name_filter) = &query.name {
        filtered_disruptors.retain(|d| {
            d.name.as_ref().map_or(false, |name| name.contains(name_filter))
        });
    }

    // Apply pagination
    let total_count = filtered_disruptors.len();
    let start = query.offset.min(total_count);
    let end = (query.offset + query.limit).min(total_count);
    let paginated_disruptors = filtered_disruptors[start..end].to_vec();

    let list = DisruptorList {
        disruptors: paginated_disruptors,
        total_count,
        offset: query.offset,
        limit: query.limit,
    };

    Ok(Json(ApiResponse::success(list)))
}

/// Get information about a specific Disruptor
pub async fn get_disruptor(
    Path(id): Path<String>,
) -> ApiResult<Json<ApiResponse<DisruptorInfo>>> {
    let disruptor_info = get_disruptor_info(&id)?;
    Ok(Json(ApiResponse::success(disruptor_info)))
}

/// Delete a Disruptor instance
pub async fn delete_disruptor(
    Path(id): Path<String>,
) -> ApiResult<Json<ApiResponse<()>>> {
    // Check if Disruptor exists
    let mut disruptor_info = get_disruptor_info(&id)?;

    // Check if it's safe to delete (not running)
    match disruptor_info.status {
        DisruptorStatus::Running => {
            return Err(ApiError::invalid_request(
                "Cannot delete a running Disruptor. Stop it first."
            ));
        }
        _ => {}
    }

    // Mark as stopping and then delete
    disruptor_info.status = DisruptorStatus::Stopping;
    store_disruptor_info(&disruptor_info)?;

    // Perform cleanup (placeholder)
    cleanup_disruptor(&id)?;

    // Remove from storage
    remove_disruptor_info(&id)?;

    Ok(Json(ApiResponse::success(())))
}

/// Start a Disruptor instance
pub async fn start_disruptor(
    Path(id): Path<String>,
) -> ApiResult<Json<ApiResponse<DisruptorInfo>>> {
    let mut disruptor_info = get_disruptor_info(&id)?;

    match disruptor_info.status {
        DisruptorStatus::Created | DisruptorStatus::Stopped | DisruptorStatus::Paused => {
            disruptor_info.status = DisruptorStatus::Running;
            disruptor_info.last_activity = chrono::Utc::now();

            // Start the actual Disruptor (placeholder)
            start_disruptor_instance(&id)?;

            store_disruptor_info(&disruptor_info)?;
            Ok(Json(ApiResponse::success(disruptor_info)))
        }
        DisruptorStatus::Running => {
            Err(ApiError::invalid_request("Disruptor is already running"))
        }
        DisruptorStatus::Stopping => {
            Err(ApiError::invalid_request("Disruptor is currently stopping"))
        }
        DisruptorStatus::Error => {
            Err(ApiError::invalid_request("Disruptor is in error state. Delete and recreate."))
        }
    }
}

/// Stop a Disruptor instance
pub async fn stop_disruptor(
    Path(id): Path<String>,
) -> ApiResult<Json<ApiResponse<DisruptorInfo>>> {
    let mut disruptor_info = get_disruptor_info(&id)?;

    match disruptor_info.status {
        DisruptorStatus::Running | DisruptorStatus::Paused => {
            disruptor_info.status = DisruptorStatus::Stopping;
            disruptor_info.last_activity = chrono::Utc::now();

            // Stop the actual Disruptor (placeholder)
            stop_disruptor_instance(&id)?;

            disruptor_info.status = DisruptorStatus::Stopped;
            store_disruptor_info(&disruptor_info)?;
            Ok(Json(ApiResponse::success(disruptor_info)))
        }
        DisruptorStatus::Stopped => {
            Err(ApiError::invalid_request("Disruptor is already stopped"))
        }
        DisruptorStatus::Created => {
            Err(ApiError::invalid_request("Disruptor has not been started yet"))
        }
        DisruptorStatus::Stopping => {
            Err(ApiError::invalid_request("Disruptor is already stopping"))
        }
        DisruptorStatus::Error => {
            Err(ApiError::invalid_request("Disruptor is in error state"))
        }
    }
}

/// Pause a Disruptor instance
pub async fn pause_disruptor(
    Path(id): Path<String>,
) -> ApiResult<Json<ApiResponse<DisruptorInfo>>> {
    let mut disruptor_info = get_disruptor_info(&id)?;

    match disruptor_info.status {
        DisruptorStatus::Running => {
            disruptor_info.status = DisruptorStatus::Paused;
            disruptor_info.last_activity = chrono::Utc::now();

            // Pause the actual Disruptor (placeholder)
            pause_disruptor_instance(&id)?;

            store_disruptor_info(&disruptor_info)?;
            Ok(Json(ApiResponse::success(disruptor_info)))
        }
        DisruptorStatus::Paused => {
            Err(ApiError::invalid_request("Disruptor is already paused"))
        }
        _ => {
            Err(ApiError::invalid_request("Can only pause a running Disruptor"))
        }
    }
}

/// Resume a paused Disruptor instance
pub async fn resume_disruptor(
    Path(id): Path<String>,
) -> ApiResult<Json<ApiResponse<DisruptorInfo>>> {
    let mut disruptor_info = get_disruptor_info(&id)?;

    match disruptor_info.status {
        DisruptorStatus::Paused => {
            disruptor_info.status = DisruptorStatus::Running;
            disruptor_info.last_activity = chrono::Utc::now();

            // Resume the actual Disruptor (placeholder)
            resume_disruptor_instance(&id)?;

            store_disruptor_info(&disruptor_info)?;
            Ok(Json(ApiResponse::success(disruptor_info)))
        }
        DisruptorStatus::Running => {
            Err(ApiError::invalid_request("Disruptor is already running"))
        }
        _ => {
            Err(ApiError::invalid_request("Can only resume a paused Disruptor"))
        }
    }
}

/// Get Disruptor status
pub async fn get_disruptor_status(
    Path(id): Path<String>,
) -> ApiResult<Json<ApiResponse<DisruptorStatus>>> {
    let disruptor_info = get_disruptor_info(&id)?;
    Ok(Json(ApiResponse::success(disruptor_info.status)))
}

// Helper functions (placeholder implementations)

fn validate_create_request(request: &CreateDisruptorRequest) -> ApiResult<()> {
    if !crate::disruptor::is_power_of_two(request.buffer_size) {
        return Err(ApiError::invalid_request("buffer_size must be a power of 2"));
    }

    if request.buffer_size < 2 || request.buffer_size > 1024 * 1024 {
        return Err(ApiError::invalid_request("buffer_size must be between 2 and 1048576"));
    }

    Ok(())
}

fn store_disruptor_info(_info: &DisruptorInfo) -> ApiResult<()> {
    // Placeholder: In a real implementation, this would store to a database or in-memory store
    Ok(())
}

fn get_disruptor_info(id: &str) -> ApiResult<DisruptorInfo> {
    // Placeholder: In a real implementation, this would retrieve from storage
    // For now, return a mock Disruptor
    if id == "test-id" {
        Ok(DisruptorInfo {
            id: id.to_string(),
            name: Some("Test Disruptor".to_string()),
            buffer_size: 1024,
            producer_type: "single".to_string(),
            wait_strategy: "blocking".to_string(),
            status: DisruptorStatus::Created,
            config: DisruptorConfig::default(),
            created_at: chrono::Utc::now(),
            last_activity: chrono::Utc::now(),
        })
    } else {
        Err(ApiError::disruptor_not_found(id))
    }
}

fn get_all_disruptors() -> ApiResult<Vec<DisruptorInfo>> {
    // Placeholder: Return empty list for now
    Ok(vec![])
}

fn remove_disruptor_info(_id: &str) -> ApiResult<()> {
    // Placeholder: In a real implementation, this would remove from storage
    Ok(())
}

fn cleanup_disruptor(_id: &str) -> ApiResult<()> {
    // Placeholder: In a real implementation, this would clean up resources
    Ok(())
}

fn start_disruptor_instance(_id: &str) -> ApiResult<()> {
    // Placeholder: In a real implementation, this would start the Disruptor
    Ok(())
}

fn stop_disruptor_instance(_id: &str) -> ApiResult<()> {
    // Placeholder: In a real implementation, this would stop the Disruptor
    Ok(())
}

fn pause_disruptor_instance(_id: &str) -> ApiResult<()> {
    // Placeholder: In a real implementation, this would pause the Disruptor
    Ok(())
}

fn resume_disruptor_instance(_id: &str) -> ApiResult<()> {
    // Placeholder: In a real implementation, this would resume the Disruptor
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;


    #[tokio::test]
    async fn test_get_disruptor_existing() {
        let result = get_disruptor(axum::extract::Path("test-id".to_string())).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_get_disruptor_not_found() {
        let result = get_disruptor(axum::extract::Path("non-existent".to_string())).await;
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_create_request_valid() {
        let request = CreateDisruptorRequest {
            buffer_size: 1024,
            producer_type: "single".to_string(),
            wait_strategy: "blocking".to_string(),
            name: None,
            config: DisruptorConfig::default(),
        };
        assert!(validate_create_request(&request).is_ok());
    }

    #[test]
    fn test_validate_create_request_invalid_buffer_size() {
        let request = CreateDisruptorRequest {
            buffer_size: 1023, // Not a power of 2
            producer_type: "single".to_string(),
            wait_strategy: "blocking".to_string(),
            name: None,
            config: DisruptorConfig::default(),
        };
        assert!(validate_create_request(&request).is_err());
    }
}
