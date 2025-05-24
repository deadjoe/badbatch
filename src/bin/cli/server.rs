//! Server Command Handlers
//!
//! Command handlers for starting BadBatch server.

use std::path::PathBuf;
use std::net::SocketAddr;

use crate::cli::CliResult;

/// Start BadBatch server
pub async fn start_server(
    config_file: Option<PathBuf>,
    bind_addr: String,
    cluster_mode: bool,
    cluster_bind: String,
    seed_nodes: Vec<String>,
) -> CliResult<()> {
    println!("Starting BadBatch server...");
    println!("  Bind address: {}", bind_addr);

    if cluster_mode {
        println!("  Cluster mode: enabled");
        println!("  Cluster bind: {}", cluster_bind);
        if !seed_nodes.is_empty() {
            println!("  Seed nodes: {:?}", seed_nodes);
        }
    } else {
        println!("  Cluster mode: disabled");
    }

    if let Some(config_path) = config_file {
        println!("  Config file: {:?}", config_path);
    }

    // Parse bind address
    let bind_socket: SocketAddr = bind_addr.parse()
        .map_err(|e| crate::cli::CliError::invalid_input(format!("Invalid bind address: {}", e)))?;

    // Create server configuration
    let mut server_config = badbatch::api::ServerConfig::default();
    server_config.host = bind_socket.ip().to_string();
    server_config.port = bind_socket.port();

    // Create and start server
    let server = badbatch::api::ApiServer::new(server_config);

    println!("✓ BadBatch server starting...");
    println!("  API endpoint: http://{}", bind_addr);
    println!("  Press Ctrl+C to stop");

    // Start the server with graceful shutdown
    let shutdown_signal = async {
        tokio::signal::ctrl_c()
            .await
            .expect("Failed to listen for shutdown signal");
        println!("\nShutdown signal received, stopping server...");
    };

    server.start_with_shutdown(shutdown_signal).await.map_err(|e| {
        crate::cli::CliError::operation(format!("Server error: {}", e))
    })?;

    println!("✓ Server stopped");

    Ok(())
}
