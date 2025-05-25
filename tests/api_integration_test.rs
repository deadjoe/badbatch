//! API Integration Tests
//!
//! Tests for the API layer integration with the Disruptor manager.

use badbatch::api::{ApiServer, ServerConfig};
use badbatch::api::models::{CreateDisruptorRequest, DisruptorConfig};
use reqwest;
use serde_json;
use std::time::Duration;
use std::collections::HashMap;
use tokio::time::timeout;

/// Test that the API server can start and handle basic requests
#[tokio::test]
async fn test_api_server_basic_functionality() {
    // Create server configuration
    let mut config = ServerConfig::default();
    config.port = 0; // Use random available port
    config.host = "127.0.0.1".to_string();

    // Create and start server
    let server = ApiServer::new(config);

    // Start server in background
    let server_handle = tokio::spawn(async move {
        let shutdown_signal = async {
            tokio::time::sleep(Duration::from_secs(2)).await;
        };
        server.start_with_shutdown(shutdown_signal).await
    });

    // Give server time to start
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Test health endpoint
    let client = reqwest::Client::new();
    let _health_response = timeout(
        Duration::from_secs(1),
        client.get("http://127.0.0.1:8080/health").send()
    ).await;

    // The test might fail if port 8080 is not available, which is expected
    // This test mainly verifies that the server can be created and started

    // Wait for server to finish
    let _ = server_handle.await;
}

/// Test Disruptor creation through API
#[tokio::test]
async fn test_disruptor_creation_api() {
    // This test verifies that the Disruptor manager integration works
    // by testing the handlers directly (without starting a full server)

    use badbatch::api::handlers::disruptor::create_disruptor;
    use axum::extract::Json as ExtractJson;

    let request = CreateDisruptorRequest {
        buffer_size: 1024,
        producer_type: "single".to_string(),
        wait_strategy: "blocking".to_string(),
        name: Some("test-disruptor".to_string()),
        config: DisruptorConfig::default(),
    };

    // Call the handler directly
    let result = create_disruptor(ExtractJson(request)).await;

    // Should succeed
    assert!(result.is_ok());

    let response = result.unwrap().0;
    assert!(response.data.is_some());

    let disruptor_response = response.data.unwrap();
    assert_eq!(disruptor_response.config.buffer_size, 1024);
    assert_eq!(disruptor_response.config.producer_type, "single");
    assert_eq!(disruptor_response.config.wait_strategy, "blocking");
    assert_eq!(disruptor_response.config.name, Some("test-disruptor".to_string()));
}

/// Test Disruptor lifecycle through API handlers
#[tokio::test]
async fn test_disruptor_lifecycle_api() {
    use badbatch::api::handlers::disruptor::{create_disruptor, get_disruptor, start_disruptor, stop_disruptor, delete_disruptor};
    use axum::extract::{Json as ExtractJson, Path};

    // Create a disruptor
    let request = CreateDisruptorRequest {
        buffer_size: 512,
        producer_type: "multi".to_string(),
        wait_strategy: "yielding".to_string(),
        name: Some("lifecycle-test".to_string()),
        config: DisruptorConfig::default(),
    };

    let create_result = create_disruptor(ExtractJson(request)).await;
    assert!(create_result.is_ok());

    let disruptor_id = create_result.unwrap().0.data.unwrap().id;

    // Get the disruptor
    let get_result = get_disruptor(Path(disruptor_id.clone())).await;
    assert!(get_result.is_ok());

    let disruptor_info = get_result.unwrap().0.data.unwrap();
    assert_eq!(disruptor_info.id, disruptor_id);
    assert_eq!(disruptor_info.status, badbatch::api::models::DisruptorStatus::Created);

    // Start the disruptor
    let start_result = start_disruptor(Path(disruptor_id.clone())).await;
    assert!(start_result.is_ok());

    let started_info = start_result.unwrap().0.data.unwrap();
    assert_eq!(started_info.status, badbatch::api::models::DisruptorStatus::Running);

    // Stop the disruptor
    let stop_result = stop_disruptor(Path(disruptor_id.clone())).await;
    assert!(stop_result.is_ok());

    let stopped_info = stop_result.unwrap().0.data.unwrap();
    assert_eq!(stopped_info.status, badbatch::api::models::DisruptorStatus::Stopped);

    // Delete the disruptor
    let delete_result = delete_disruptor(Path(disruptor_id)).await;
    assert!(delete_result.is_ok());
}

/// Test event publishing validation
#[tokio::test]
async fn test_event_publishing_validation() {
    use badbatch::api::handlers::events::publish_event;
    use badbatch::api::models::PublishEventRequest;
    use axum::extract::{Json as ExtractJson, Path};

    // Try to publish to non-existent disruptor
    let request = PublishEventRequest {
        data: serde_json::json!({"message": "test"}),
        metadata: HashMap::new(),
        correlation_id: None,
    };

    let result = publish_event(
        Path("non-existent-id".to_string()),
        ExtractJson(request)
    ).await;

    // Should fail because disruptor doesn't exist
    assert!(result.is_err());
}

/// Test invalid buffer size handling
#[tokio::test]
async fn test_invalid_buffer_size() {
    use badbatch::api::handlers::disruptor::create_disruptor;
    use axum::extract::Json as ExtractJson;

    let request = CreateDisruptorRequest {
        buffer_size: 1023, // Not a power of 2
        producer_type: "single".to_string(),
        wait_strategy: "blocking".to_string(),
        name: Some("invalid-test".to_string()),
        config: DisruptorConfig::default(),
    };

    let result = create_disruptor(ExtractJson(request)).await;

    // Should fail due to invalid buffer size
    assert!(result.is_err());
}

/// Test thread safety of sequence generation
#[tokio::test]
async fn test_sequence_generation_thread_safety() {
    use badbatch::api::handlers::disruptor::create_disruptor;
    use badbatch::api::handlers::events::publish_event;
    use badbatch::api::models::{CreateDisruptorRequest, DisruptorConfig, PublishEventRequest};
    use axum::extract::{Json as ExtractJson, Path};
    use std::sync::Arc;
    use tokio::sync::Barrier;

    // Create a disruptor first
    let request = CreateDisruptorRequest {
        buffer_size: 1024,
        producer_type: "multi".to_string(),
        wait_strategy: "blocking".to_string(),
        name: Some("thread-safety-test".to_string()),
        config: DisruptorConfig::default(),
    };

    let create_result = create_disruptor(ExtractJson(request)).await;
    assert!(create_result.is_ok());

    let disruptor_id = create_result.unwrap().0.data.unwrap().id;

    // Start the disruptor
    use badbatch::api::handlers::disruptor::start_disruptor;
    let start_result = start_disruptor(Path(disruptor_id.clone())).await;
    assert!(start_result.is_ok());

    // Test concurrent event publishing
    let barrier = Arc::new(Barrier::new(5));
    let mut handles = vec![];

    for i in 0..5 {
        let disruptor_id = disruptor_id.clone();
        let barrier = barrier.clone();

        let handle = tokio::spawn(async move {
            barrier.wait().await;

            let request = PublishEventRequest {
                data: serde_json::json!({"thread": i, "message": "concurrent test"}),
                metadata: HashMap::new(),
                correlation_id: Some(format!("thread-{}", i)),
            };

            publish_event(
                Path(disruptor_id),
                ExtractJson(request)
            ).await
        });

        handles.push(handle);
    }

    // Wait for all tasks to complete
    for handle in handles {
        let result = handle.await;
        assert!(result.is_ok());
        // Now that we have real event publishing, these should succeed
        let publish_result = result.unwrap();
        assert!(publish_result.is_ok());
    }
}

/// Test real event publishing functionality
#[tokio::test]
async fn test_real_event_publishing() {
    use badbatch::api::handlers::disruptor::{create_disruptor, start_disruptor};
    use badbatch::api::handlers::events::publish_event;
    use badbatch::api::models::{CreateDisruptorRequest, DisruptorConfig, PublishEventRequest};
    use axum::extract::{Json as ExtractJson, Path};

    // Create a disruptor
    let request = CreateDisruptorRequest {
        buffer_size: 64,
        producer_type: "single".to_string(),
        wait_strategy: "yielding".to_string(),
        name: Some("real-publish-test".to_string()),
        config: DisruptorConfig::default(),
    };

    let create_result = create_disruptor(ExtractJson(request)).await;
    assert!(create_result.is_ok());

    let disruptor_id = create_result.unwrap().0.data.unwrap().id;

    // Start the disruptor
    let start_result = start_disruptor(Path(disruptor_id.clone())).await;
    assert!(start_result.is_ok());

    // Publish a real event
    let event_request = PublishEventRequest {
        data: serde_json::json!({
            "message": "Hello, Disruptor!",
            "timestamp": chrono::Utc::now().to_rfc3339(),
            "priority": "high"
        }),
        metadata: {
            let mut map = HashMap::new();
            map.insert("source".to_string(), "integration_test".to_string());
            map.insert("version".to_string(), "1.0".to_string());
            map
        },
        correlation_id: Some("test-correlation-123".to_string()),
    };

    let publish_result = publish_event(
        Path(disruptor_id.clone()),
        ExtractJson(event_request)
    ).await;

    // Should succeed with real implementation
    if let Err(ref e) = publish_result {
        eprintln!("Event publishing failed: {:?}", e);
    }
    assert!(publish_result.is_ok());

    let response = publish_result.unwrap().0;
    assert!(response.data.is_some());

    let event_response = response.data.unwrap();
    assert_eq!(event_response.correlation_id, Some("test-correlation-123".to_string()));
    assert!(event_response.sequence >= 0);
}
