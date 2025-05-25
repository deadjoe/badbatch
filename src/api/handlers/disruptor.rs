//! Disruptor Handlers
//!
//! This module provides handlers for Disruptor lifecycle management operations
//! including creation, deletion, starting, stopping, and status queries.

use axum::{
    extract::{Path, Query, Json as ExtractJson},
    response::Json,
};
use std::sync::{Arc, Mutex, OnceLock};

use crate::api::{
    ApiResponse,
    models::{
        CreateDisruptorRequest, CreateDisruptorResponse, DisruptorInfo,
        DisruptorList, DisruptorStatus, ListDisruptorsQuery,
    },
    handlers::ApiResult,
    manager::DisruptorManager,
};

// Global manager instance for now - this is a temporary solution
static GLOBAL_MANAGER: OnceLock<Arc<Mutex<DisruptorManager>>> = OnceLock::new();

fn get_manager() -> &'static Arc<Mutex<DisruptorManager>> {
    GLOBAL_MANAGER.get_or_init(|| Arc::new(Mutex::new(DisruptorManager::new())))
}

/// Create a new Disruptor instance
pub async fn create_disruptor(
    ExtractJson(request): ExtractJson<CreateDisruptorRequest>,
) -> ApiResult<Json<ApiResponse<CreateDisruptorResponse>>> {
    // Get the global manager
    let manager = get_manager();
    let manager = manager.lock().map_err(|_| crate::api::error::ApiError::internal("Failed to acquire manager lock"))?;

    // Create the Disruptor using the manager
    let disruptor_info = manager.create_disruptor(
        request.name.clone(),
        request.buffer_size,
        &request.producer_type,
        &request.wait_strategy,
        request.config.clone(),
    )?;

    let response = CreateDisruptorResponse {
        id: disruptor_info.id.clone(),
        config: disruptor_info,
        created_at: chrono::Utc::now(),
    };

    Ok(Json(ApiResponse::success(response)))
}

/// List all Disruptor instances
pub async fn list_disruptors(
    Query(query): Query<ListDisruptorsQuery>,
) -> ApiResult<Json<ApiResponse<DisruptorList>>> {
    // Get the global manager
    let manager = get_manager();
    let manager = manager.lock().map_err(|_| crate::api::error::ApiError::internal("Failed to acquire manager lock"))?;

    // Get all Disruptor instances from manager
    let all_disruptors = manager.list_disruptors()?;

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
    let manager = get_manager();
    let manager = manager.lock().map_err(|_| crate::api::error::ApiError::internal("Failed to acquire manager lock"))?;
    let disruptor_info = manager.get_disruptor_info(&id)?;
    Ok(Json(ApiResponse::success(disruptor_info)))
}

/// Delete a Disruptor instance
pub async fn delete_disruptor(
    Path(id): Path<String>,
) -> ApiResult<Json<ApiResponse<()>>> {
    let manager = get_manager();
    let manager = manager.lock().map_err(|_| crate::api::error::ApiError::internal("Failed to acquire manager lock"))?;
    manager.remove_disruptor(&id)?;
    Ok(Json(ApiResponse::success(())))
}

/// Start a Disruptor instance
pub async fn start_disruptor(
    Path(id): Path<String>,
) -> ApiResult<Json<ApiResponse<DisruptorInfo>>> {
    let manager = get_manager();
    let manager = manager.lock().map_err(|_| crate::api::error::ApiError::internal("Failed to acquire manager lock"))?;
    let disruptor_info = manager.start_disruptor(&id)?;
    Ok(Json(ApiResponse::success(disruptor_info)))
}

/// Stop a Disruptor instance
pub async fn stop_disruptor(
    Path(id): Path<String>,
) -> ApiResult<Json<ApiResponse<DisruptorInfo>>> {
    let manager = get_manager();
    let manager = manager.lock().map_err(|_| crate::api::error::ApiError::internal("Failed to acquire manager lock"))?;
    let disruptor_info = manager.stop_disruptor(&id)?;
    Ok(Json(ApiResponse::success(disruptor_info)))
}

/// Pause a Disruptor instance
pub async fn pause_disruptor(
    Path(id): Path<String>,
) -> ApiResult<Json<ApiResponse<DisruptorInfo>>> {
    let manager = get_manager();
    let manager = manager.lock().map_err(|_| crate::api::error::ApiError::internal("Failed to acquire manager lock"))?;
    let _disruptor_info = manager.get_disruptor_info(&id)?;
    manager.update_status(&id, DisruptorStatus::Paused)?;
    let updated_info = manager.get_disruptor_info(&id)?;
    Ok(Json(ApiResponse::success(updated_info)))
}

/// Resume a paused Disruptor instance
pub async fn resume_disruptor(
    Path(id): Path<String>,
) -> ApiResult<Json<ApiResponse<DisruptorInfo>>> {
    let manager = get_manager();
    let manager = manager.lock().map_err(|_| crate::api::error::ApiError::internal("Failed to acquire manager lock"))?;
    let _disruptor_info = manager.get_disruptor_info(&id)?;
    manager.update_status(&id, DisruptorStatus::Running)?;
    let updated_info = manager.get_disruptor_info(&id)?;
    Ok(Json(ApiResponse::success(updated_info)))
}

/// Get Disruptor status
pub async fn get_disruptor_status(
    Path(id): Path<String>,
) -> ApiResult<Json<ApiResponse<DisruptorStatus>>> {
    let manager = get_manager();
    let manager = manager.lock().map_err(|_| crate::api::error::ApiError::internal("Failed to acquire manager lock"))?;
    let disruptor_info = manager.get_disruptor_info(&id)?;
    Ok(Json(ApiResponse::success(disruptor_info.status)))
}

// Helper functions - these are now mostly handled by the DisruptorManager

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::models::DisruptorConfig;

    #[tokio::test]
    async fn test_get_disruptor_not_found() {
        let result = get_disruptor(
            axum::extract::Path("non-existent".to_string())
        ).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_create_and_get_disruptor() {
        // Create a disruptor
        let request = CreateDisruptorRequest {
            buffer_size: 1024,
            producer_type: "single".to_string(),
            wait_strategy: "blocking".to_string(),
            name: Some("test".to_string()),
            config: DisruptorConfig::default(),
        };

        let create_result = create_disruptor(
            axum::extract::Json(request)
        ).await;

        assert!(create_result.is_ok());
        let response = create_result.unwrap().0;
        let disruptor_id = response.data.unwrap().id;

        // Get the disruptor
        let get_result = get_disruptor(
            axum::extract::Path(disruptor_id)
        ).await;

        assert!(get_result.is_ok());
    }

    #[tokio::test]
    async fn test_list_disruptors() {
        let query = ListDisruptorsQuery {
            offset: 0,
            limit: 10,
            status: None,
            name: None,
        };

        let result = list_disruptors(
            axum::extract::Query(query)
        ).await;

        assert!(result.is_ok());
    }
}
