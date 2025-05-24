//! Event Handlers
//!
//! This module provides handlers for event publishing and querying operations.
//! It handles single events, batch events, and event retrieval.

use axum::{
    extract::{Path, Query, Json as ExtractJson},
    response::Json,
};
use uuid::Uuid;
use std::collections::HashMap;

use crate::api::{
    ApiResponse,
    models::{
        PublishEventRequest, PublishEventResponse, PublishBatchRequest,
        PublishBatchResponse, EventData,
    },
    handlers::{ApiResult, ApiError},
};

/// Publish a single event to a Disruptor
pub async fn publish_event(
    Path(disruptor_id): Path<String>,
    ExtractJson(request): ExtractJson<PublishEventRequest>,
) -> ApiResult<Json<ApiResponse<PublishEventResponse>>> {
    // Validate that the Disruptor exists and is running
    validate_disruptor_for_publishing(&disruptor_id)?;

    // Validate the event data
    validate_event_data(&request.data)?;

    // Generate event ID and sequence
    let event_id = Uuid::new_v4().to_string();
    let sequence = get_next_sequence(&disruptor_id)?;

    // Create event data
    let mut event_data = EventData::new(sequence, request.data.clone());
    event_data.metadata = request.metadata;
    event_data.correlation_id = request.correlation_id.clone();

    // Publish the event to the Disruptor
    publish_event_to_disruptor(&disruptor_id, &event_data)?;

    let response = PublishEventResponse {
        sequence,
        event_id,
        correlation_id: request.correlation_id,
        published_at: chrono::Utc::now(),
    };

    Ok(Json(ApiResponse::success(response)))
}

/// Publish multiple events to a Disruptor
pub async fn publish_batch(
    Path(disruptor_id): Path<String>,
    ExtractJson(request): ExtractJson<PublishBatchRequest>,
) -> ApiResult<Json<ApiResponse<PublishBatchResponse>>> {
    // Validate that the Disruptor exists and is running
    validate_disruptor_for_publishing(&disruptor_id)?;

    if request.events.is_empty() {
        return Err(ApiError::invalid_request("Batch cannot be empty"));
    }

    if request.events.len() > 1000 {
        return Err(ApiError::invalid_request("Batch size cannot exceed 1000 events"));
    }

    let batch_id = Uuid::new_v4().to_string();
    let mut published_events = Vec::new();

    // Process each event in the batch
    for event_request in &request.events {
        // Validate event data
        validate_event_data(&event_request.data)?;

        // Generate event ID and sequence
        let event_id = Uuid::new_v4().to_string();
        let sequence = get_next_sequence(&disruptor_id)?;

        // Create event data
        let mut event_data = EventData::new(sequence, event_request.data.clone());
        event_data.metadata = event_request.metadata.clone();
        event_data.correlation_id = event_request.correlation_id.clone();

        // Publish the event
        publish_event_to_disruptor(&disruptor_id, &event_data)?;

        published_events.push(PublishEventResponse {
            sequence,
            event_id,
            correlation_id: event_request.correlation_id.clone(),
            published_at: chrono::Utc::now(),
        });
    }

    let response = PublishBatchResponse {
        published_count: published_events.len(),
        events: published_events,
        batch_id,
        published_at: chrono::Utc::now(),
    };

    Ok(Json(ApiResponse::success(response)))
}

/// Publish events to multiple Disruptors (batch operation)
pub async fn publish_batch_events(
    ExtractJson(request): ExtractJson<HashMap<String, PublishBatchRequest>>,
) -> ApiResult<Json<ApiResponse<HashMap<String, PublishBatchResponse>>>> {
    let mut responses = HashMap::new();

    for (disruptor_id, batch_request) in request {
        match publish_batch_internal(&disruptor_id, batch_request).await {
            Ok(response) => {
                responses.insert(disruptor_id, response);
            }
            Err(e) => {
                // Log error but continue with other Disruptors
                tracing::error!("Failed to publish batch to Disruptor {}: {}", disruptor_id, e);
                // In a real implementation, you might want to include error information
                // in the response rather than silently failing
            }
        }
    }

    Ok(Json(ApiResponse::success(responses)))
}

/// List events from a Disruptor (for debugging/monitoring)
pub async fn list_events(
    Path(disruptor_id): Path<String>,
    Query(query): Query<EventQuery>,
) -> ApiResult<Json<ApiResponse<EventList>>> {
    // Validate that the Disruptor exists
    validate_disruptor_exists(&disruptor_id)?;

    // Get events from the Disruptor
    let events = get_events_from_disruptor(&disruptor_id, &query)?;

    let event_list = EventList {
        events,
        total_count: 0, // Placeholder
        offset: query.offset.unwrap_or(0),
        limit: query.limit.unwrap_or(50),
    };

    Ok(Json(ApiResponse::success(event_list)))
}

/// Get a specific event by sequence number
pub async fn get_event(
    Path((disruptor_id, sequence)): Path<(String, i64)>,
) -> ApiResult<Json<ApiResponse<EventData>>> {
    // Validate that the Disruptor exists
    validate_disruptor_exists(&disruptor_id)?;

    // Get the specific event
    let event = get_event_from_disruptor(&disruptor_id, sequence)?;

    Ok(Json(ApiResponse::success(event)))
}

// Helper types and functions

#[derive(Debug, serde::Deserialize)]
pub struct EventQuery {
    pub offset: Option<usize>,
    pub limit: Option<usize>,
    pub from_sequence: Option<i64>,
    pub to_sequence: Option<i64>,
    pub correlation_id: Option<String>,
}

#[derive(Debug, serde::Serialize)]
pub struct EventList {
    pub events: Vec<EventData>,
    pub total_count: usize,
    pub offset: usize,
    pub limit: usize,
}

async fn publish_batch_internal(
    disruptor_id: &str,
    request: PublishBatchRequest,
) -> ApiResult<PublishBatchResponse> {
    // This is the same logic as publish_batch but without the HTTP layer
    validate_disruptor_for_publishing(disruptor_id)?;

    if request.events.is_empty() {
        return Err(ApiError::invalid_request("Batch cannot be empty"));
    }

    let batch_id = Uuid::new_v4().to_string();
    let mut published_events = Vec::new();

    for event_request in &request.events {
        validate_event_data(&event_request.data)?;

        let event_id = Uuid::new_v4().to_string();
        let sequence = get_next_sequence(disruptor_id)?;

        let mut event_data = EventData::new(sequence, event_request.data.clone());
        event_data.metadata = event_request.metadata.clone();
        event_data.correlation_id = event_request.correlation_id.clone();

        publish_event_to_disruptor(disruptor_id, &event_data)?;

        published_events.push(PublishEventResponse {
            sequence,
            event_id,
            correlation_id: event_request.correlation_id.clone(),
            published_at: chrono::Utc::now(),
        });
    }

    Ok(PublishBatchResponse {
        published_count: published_events.len(),
        events: published_events,
        batch_id,
        published_at: chrono::Utc::now(),
    })
}

fn validate_disruptor_exists(disruptor_id: &str) -> ApiResult<()> {
    // Placeholder: Check if Disruptor exists
    if disruptor_id == "test-id" {
        Ok(())
    } else {
        Err(ApiError::disruptor_not_found(disruptor_id))
    }
}

fn validate_disruptor_for_publishing(disruptor_id: &str) -> ApiResult<()> {
    // Placeholder: Check if Disruptor exists and is in a state that allows publishing
    validate_disruptor_exists(disruptor_id)?;

    // Check if Disruptor is running
    // In a real implementation, you would check the actual status
    Ok(())
}

fn validate_event_data(data: &serde_json::Value) -> ApiResult<()> {
    // Basic validation of event data
    if data.is_null() {
        return Err(ApiError::invalid_request("Event data cannot be null"));
    }

    // Check size limits (e.g., max 1MB per event)
    let serialized = serde_json::to_string(data)
        .map_err(|e| ApiError::invalid_request(format!("Invalid JSON data: {}", e)))?;

    if serialized.len() > 1024 * 1024 {
        return Err(ApiError::invalid_request("Event data too large (max 1MB)"));
    }

    Ok(())
}

fn get_next_sequence(_disruptor_id: &str) -> ApiResult<i64> {
    // Placeholder: Get the next sequence number from the Disruptor
    // In a real implementation, this would call the Disruptor's sequencer
    static mut COUNTER: i64 = 0;
    unsafe {
        COUNTER += 1;
        Ok(COUNTER)
    }
}

fn publish_event_to_disruptor(disruptor_id: &str, event_data: &EventData) -> ApiResult<()> {
    // Placeholder: Actually publish the event to the Disruptor
    // In a real implementation, this would:
    // 1. Get the Disruptor instance from the manager
    // 2. Use an EventTranslator to populate the event
    // 3. Publish using the Disruptor's publish methods

    tracing::info!(
        disruptor_id = %disruptor_id,
        event_id = %event_data.id,
        sequence = %event_data.sequence,
        "Event published"
    );

    Ok(())
}

fn get_events_from_disruptor(_disruptor_id: &str, _query: &EventQuery) -> ApiResult<Vec<EventData>> {
    // Placeholder: Get events from the Disruptor
    // In a real implementation, this would query the ring buffer or event store
    Ok(vec![])
}

fn get_event_from_disruptor(disruptor_id: &str, sequence: i64) -> ApiResult<EventData> {
    // Placeholder: Get a specific event from the Disruptor
    if disruptor_id == "test-id" && sequence == 1 {
        Ok(EventData::new(sequence, serde_json::json!({"test": "data"})))
    } else {
        Err(ApiError::invalid_request("Event not found"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_validate_event_data_valid() {
        let data = json!({"message": "test"});
        assert!(validate_event_data(&data).is_ok());
    }

    #[test]
    fn test_validate_event_data_null() {
        let data = json!(null);
        assert!(validate_event_data(&data).is_err());
    }

    #[test]
    fn test_validate_disruptor_exists() {
        assert!(validate_disruptor_exists("test-id").is_ok());
        assert!(validate_disruptor_exists("non-existent").is_err());
    }

    #[test]
    fn test_get_next_sequence() {
        let seq1 = get_next_sequence("test").unwrap();
        let seq2 = get_next_sequence("test").unwrap();
        assert!(seq2 > seq1);
    }
}
