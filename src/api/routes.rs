//! API Routes Configuration
//!
//! This module defines all the HTTP routes for the BadBatch Disruptor API,
//! organizing them into logical groups and providing a clean routing structure.

use axum::{
    routing::{delete, get, post},
    Router,
};
use std::sync::{Arc, Mutex};
use tower_http::{
    cors::{Any, CorsLayer},
    trace::TraceLayer,
};

use crate::api::{handlers, manager::DisruptorManager, ServerConfig};

/// Create the main application router with state
pub fn create_router_with_state(
    config: &ServerConfig,
    manager: Arc<Mutex<DisruptorManager>>,
) -> Router {
    // Create API routes with state
    let api_routes = create_api_routes_with_state(manager.clone());

    let mut router = Router::new()
        .nest("/api/v1", api_routes)
        .route("/health", get(handlers::system::health_check))
        .route("/metrics", get(handlers::system::system_metrics))
        .route("/", get(handlers::system::root))
        .with_state(manager);

    // Add middleware layers
    if config.enable_cors {
        router = router.layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        );
    }

    if config.enable_logging {
        router = router.layer(TraceLayer::new_for_http());
    }

    router
}

/// Create the main application router (legacy, for compatibility)
pub fn create_router(config: &ServerConfig) -> Router {
    // For now, create routes without state management
    // TODO: This needs to be updated to use the manager properly
    let api_routes = create_api_routes();

    let mut router = Router::new()
        .nest("/api/v1", api_routes)
        .route("/health", get(handlers::system::health_check))
        .route("/metrics", get(handlers::system::system_metrics))
        .route("/", get(handlers::system::root));

    // Add middleware layers
    if config.enable_cors {
        router = router.layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        );
    }

    if config.enable_logging {
        router = router.layer(TraceLayer::new_for_http());
    }

    router
}

/// Create API v1 routes with state management
fn create_api_routes_with_state(
    manager: Arc<Mutex<DisruptorManager>>,
) -> Router<Arc<Mutex<DisruptorManager>>> {
    Router::new()
        // System routes
        .route("/health", get(handlers::system::health_check))
        .route("/metrics", get(handlers::system::system_metrics))
        .route("/version", get(handlers::system::version))
        // Disruptor management routes
        .nest(
            "/disruptor",
            create_disruptor_routes_with_state(manager.clone()),
        )
        // Batch operations
        .route(
            "/batch/events",
            post(handlers::events::publish_batch_events),
        )
        .with_state(manager)
}

/// Create API v1 routes (legacy, without state)
fn create_api_routes() -> Router {
    Router::new()
        // System routes
        .route("/health", get(handlers::system::health_check))
        .route("/metrics", get(handlers::system::system_metrics))
        .route("/version", get(handlers::system::version))
        // Disruptor management routes
        .nest("/disruptor", create_disruptor_routes())
        // Batch operations
        .route(
            "/batch/events",
            post(handlers::events::publish_batch_events),
        )
}

/// Create Disruptor-specific routes with state management
fn create_disruptor_routes_with_state(
    _manager: Arc<Mutex<DisruptorManager>>,
) -> Router<Arc<Mutex<DisruptorManager>>> {
    Router::new()
        // Disruptor lifecycle - temporarily use existing handlers
        .route("/", post(handlers::disruptor::create_disruptor))
        .route("/", get(handlers::disruptor::list_disruptors))
        .route("/:id", get(handlers::disruptor::get_disruptor))
        .route("/:id", delete(handlers::disruptor::delete_disruptor))
        // Disruptor control
        .route("/:id/start", post(handlers::disruptor::start_disruptor))
        .route("/:id/stop", post(handlers::disruptor::stop_disruptor))
        .route("/:id/pause", post(handlers::disruptor::pause_disruptor))
        .route("/:id/resume", post(handlers::disruptor::resume_disruptor))
        // Event publishing
        .route("/:id/events", post(handlers::events::publish_event))
        .route("/:id/events/batch", post(handlers::events::publish_batch))
        // Monitoring and metrics
        .route(
            "/:id/metrics",
            get(handlers::metrics::get_disruptor_metrics),
        )
        .route("/:id/health", get(handlers::metrics::get_disruptor_health))
        .route(
            "/:id/status",
            get(handlers::disruptor::get_disruptor_status_with_state),
        )
        // Event querying (optional, for debugging)
        .route("/:id/events", get(handlers::events::list_events))
        .route("/:id/events/:sequence", get(handlers::events::get_event))
}

/// Create Disruptor-specific routes (legacy, without state)
fn create_disruptor_routes() -> Router {
    Router::new()
        // Disruptor lifecycle
        .route("/", post(handlers::disruptor::create_disruptor))
        .route("/", get(handlers::disruptor::list_disruptors))
        .route("/:id", get(handlers::disruptor::get_disruptor))
        .route("/:id", delete(handlers::disruptor::delete_disruptor))
        // Disruptor control
        .route("/:id/start", post(handlers::disruptor::start_disruptor))
        .route("/:id/stop", post(handlers::disruptor::stop_disruptor))
        .route("/:id/pause", post(handlers::disruptor::pause_disruptor))
        .route("/:id/resume", post(handlers::disruptor::resume_disruptor))
        // Event publishing
        .route("/:id/events", post(handlers::events::publish_event))
        .route("/:id/events/batch", post(handlers::events::publish_batch))
        // Monitoring and metrics
        .route(
            "/:id/metrics",
            get(handlers::metrics::get_disruptor_metrics),
        )
        .route("/:id/health", get(handlers::metrics::get_disruptor_health))
        .route(
            "/:id/status",
            get(handlers::disruptor::get_disruptor_status),
        )
        // Event querying (optional, for debugging)
        .route("/:id/events", get(handlers::events::list_events))
        .route("/:id/events/:sequence", get(handlers::events::get_event))
}

/// Route groups for documentation and organization
pub mod route_groups {
    /// System-level routes
    pub const SYSTEM: &[&str] = &[
        "GET /health",
        "GET /metrics",
        "GET /api/v1/health",
        "GET /api/v1/metrics",
        "GET /api/v1/version",
    ];

    /// Disruptor management routes
    pub const DISRUPTOR_MANAGEMENT: &[&str] = &[
        "POST /api/v1/disruptor",
        "GET /api/v1/disruptor",
        "GET /api/v1/disruptor/:id",
        "DELETE /api/v1/disruptor/:id",
    ];

    /// Disruptor control routes
    pub const DISRUPTOR_CONTROL: &[&str] = &[
        "POST /api/v1/disruptor/:id/start",
        "POST /api/v1/disruptor/:id/stop",
        "POST /api/v1/disruptor/:id/pause",
        "POST /api/v1/disruptor/:id/resume",
    ];

    /// Event publishing routes
    pub const EVENT_PUBLISHING: &[&str] = &[
        "POST /api/v1/disruptor/:id/events",
        "POST /api/v1/disruptor/:id/events/batch",
        "POST /api/v1/batch/events",
    ];

    /// Monitoring routes
    pub const MONITORING: &[&str] = &[
        "GET /api/v1/disruptor/:id/metrics",
        "GET /api/v1/disruptor/:id/health",
        "GET /api/v1/disruptor/:id/status",
    ];

    /// Event querying routes
    pub const EVENT_QUERYING: &[&str] = &[
        "GET /api/v1/disruptor/:id/events",
        "GET /api/v1/disruptor/:id/events/:sequence",
    ];
}

/// Route documentation
pub struct RouteDoc {
    pub method: &'static str,
    pub path: &'static str,
    pub description: &'static str,
    pub parameters: Vec<RouteParam>,
    pub request_body: Option<&'static str>,
    pub response_body: Option<&'static str>,
}

/// Route parameter documentation
pub struct RouteParam {
    pub name: &'static str,
    pub param_type: &'static str,
    pub description: &'static str,
    pub required: bool,
}

/// Get documentation for all routes
pub fn get_route_documentation() -> Vec<RouteDoc> {
    vec![
        RouteDoc {
            method: "POST",
            path: "/api/v1/disruptor",
            description: "Create a new Disruptor instance",
            parameters: vec![],
            request_body: Some("CreateDisruptorRequest"),
            response_body: Some("CreateDisruptorResponse"),
        },
        RouteDoc {
            method: "GET",
            path: "/api/v1/disruptor",
            description: "List all Disruptor instances",
            parameters: vec![
                RouteParam {
                    name: "offset",
                    param_type: "query",
                    description: "Pagination offset",
                    required: false,
                },
                RouteParam {
                    name: "limit",
                    param_type: "query",
                    description: "Pagination limit",
                    required: false,
                },
                RouteParam {
                    name: "status",
                    param_type: "query",
                    description: "Filter by status",
                    required: false,
                },
            ],
            request_body: None,
            response_body: Some("DisruptorList"),
        },
        RouteDoc {
            method: "GET",
            path: "/api/v1/disruptor/:id",
            description: "Get information about a specific Disruptor",
            parameters: vec![RouteParam {
                name: "id",
                param_type: "path",
                description: "Disruptor ID",
                required: true,
            }],
            request_body: None,
            response_body: Some("DisruptorInfo"),
        },
        RouteDoc {
            method: "POST",
            path: "/api/v1/disruptor/:id/events",
            description: "Publish a single event to the Disruptor",
            parameters: vec![RouteParam {
                name: "id",
                param_type: "path",
                description: "Disruptor ID",
                required: true,
            }],
            request_body: Some("PublishEventRequest"),
            response_body: Some("PublishEventResponse"),
        },
        RouteDoc {
            method: "GET",
            path: "/api/v1/disruptor/:id/metrics",
            description: "Get performance metrics for a Disruptor",
            parameters: vec![RouteParam {
                name: "id",
                param_type: "path",
                description: "Disruptor ID",
                required: true,
            }],
            request_body: None,
            response_body: Some("DisruptorMetrics"),
        },
        RouteDoc {
            method: "GET",
            path: "/health",
            description: "System health check",
            parameters: vec![],
            request_body: None,
            response_body: Some("HealthResponse"),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::len_zero)]
    fn test_route_groups_not_empty() {
        // Verify route groups are properly defined
        assert!(
            route_groups::SYSTEM.len() > 0,
            "SYSTEM routes should not be empty"
        );
        assert!(
            route_groups::DISRUPTOR_MANAGEMENT.len() > 0,
            "DISRUPTOR_MANAGEMENT routes should not be empty"
        );
        assert!(
            route_groups::EVENT_PUBLISHING.len() > 0,
            "EVENT_PUBLISHING routes should not be empty"
        );
        assert!(
            route_groups::MONITORING.len() > 0,
            "MONITORING routes should not be empty"
        );
    }

    #[test]
    fn test_route_documentation() {
        let docs = get_route_documentation();
        assert!(!docs.is_empty());

        // Check that we have documentation for key routes
        assert!(docs
            .iter()
            .any(|doc| doc.path == "/api/v1/disruptor" && doc.method == "POST"));
        assert!(docs.iter().any(|doc| doc.path == "/health"));
    }

    #[test]
    fn test_create_router() {
        let config = ServerConfig::default();
        let _router = create_router(&config);
        // Router creation should not panic
        // Test passes if no panic occurs
    }

    #[test]
    fn test_create_router_with_state() {
        use crate::api::manager::DisruptorManager;

        let config = ServerConfig::default();
        let manager = Arc::new(Mutex::new(DisruptorManager::new()));
        let _router = create_router_with_state(&config, manager);
        // Router creation with state should not panic
        // Test passes if no panic occurs
    }

    #[test]
    fn test_state_management_vs_global() {
        use crate::api::manager::DisruptorManager;

        // Test that we can create multiple independent manager instances
        // This demonstrates the advantage over global state
        let manager1 = Arc::new(Mutex::new(DisruptorManager::new()));
        let manager2 = Arc::new(Mutex::new(DisruptorManager::new()));

        let config = ServerConfig::default();

        // Create two routers with different state
        let _router1 = create_router_with_state(&config, manager1);
        let _router2 = create_router_with_state(&config, manager2);

        // This would not be possible with global state - each router
        // can have its own independent DisruptorManager instance
        // Test passes if no panic occurs
    }
}
