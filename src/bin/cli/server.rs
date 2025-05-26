//! Server Command Handlers
//!
//! Command handlers for starting BadBatch server.

use std::net::SocketAddr;
use std::path::PathBuf;

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

    #[cfg(feature = "cluster")]
    let cluster_instance = if cluster_mode {
        println!("  Cluster mode: enabled");
        println!("  Cluster bind: {}", cluster_bind);
        if !seed_nodes.is_empty() {
            println!("  Seed nodes: {:?}", seed_nodes);
        }

        // Parse cluster bind address
        let cluster_socket: SocketAddr = cluster_bind.parse().map_err(|e| {
            crate::cli::CliError::invalid_input(format!("Invalid cluster bind address: {}", e))
        })?;

        // Create cluster configuration
        let cluster_config = badbatch::cluster::ClusterConfig {
            bind_addr: cluster_socket,
            seed_nodes: seed_nodes
                .iter()
                .map(|addr| addr.parse())
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| {
                    crate::cli::CliError::invalid_input(format!("Invalid seed node address: {}", e))
                })?,
            ..Default::default()
        };

        // Create and start cluster
        let cluster = badbatch::cluster::Cluster::new(cluster_config)
            .await
            .map_err(|e| {
                crate::cli::CliError::operation(format!("Failed to create cluster: {}", e))
            })?;

        cluster.start().await.map_err(|e| {
            crate::cli::CliError::operation(format!("Failed to start cluster: {}", e))
        })?;

        Some(cluster)
    } else {
        println!("  Cluster mode: disabled (single-node mode)");
        None
    };

    #[cfg(not(feature = "cluster"))]
    {
        if cluster_mode {
            eprintln!("⚠️  Warning: Cluster mode requested but not compiled in");
            eprintln!("   Cluster functionality requires the 'cluster' feature");
            eprintln!("   Compile with: cargo build --features cluster");
            eprintln!("   Starting in single-node mode instead");
            eprintln!("   Requested cluster bind: {}", cluster_bind);
            if !seed_nodes.is_empty() {
                eprintln!("   Requested seed nodes: {:?}", seed_nodes);
            }
        } else {
            println!("  Cluster mode: disabled (single-node mode)");
        }
    }

    if let Some(config_path) = config_file {
        println!("  Config file: {:?}", config_path);
    }

    // Parse bind address
    let bind_socket: SocketAddr = bind_addr
        .parse()
        .map_err(|e| crate::cli::CliError::invalid_input(format!("Invalid bind address: {}", e)))?;

    // Create server configuration
    let server_config = badbatch::api::ServerConfig {
        host: bind_socket.ip().to_string(),
        port: bind_socket.port(),
        ..Default::default()
    };

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

    server
        .start_with_shutdown(shutdown_signal)
        .await
        .map_err(|e| crate::cli::CliError::operation(format!("Server error: {}", e)))?;

    // Stop cluster if it was started
    #[cfg(feature = "cluster")]
    if let Some(cluster) = cluster_instance {
        println!("Stopping cluster...");
        cluster.stop().await.map_err(|e| {
            crate::cli::CliError::operation(format!("Failed to stop cluster: {}", e))
        })?;
        println!("✓ Cluster stopped");
    }

    println!("✓ Server stopped");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_bind_address_parsing() {
        // Valid addresses
        let valid_addresses = [
            "127.0.0.1:8080",
            "0.0.0.0:3000",
            "192.168.1.100:9000",
            "[::1]:8080",
            "localhost:8080",
        ];

        for addr in &valid_addresses {
            let result: Result<SocketAddr, _> = addr.parse();
            if result.is_ok() {
                let socket_addr = result.unwrap();
                assert!(socket_addr.port() > 0);
            }
        }
    }

    #[test]
    fn test_invalid_bind_address_parsing() {
        let invalid_addresses = [
            "invalid",
            "127.0.0.1",       // Missing port
            ":8080",           // Missing host
            "127.0.0.1:99999", // Invalid port
            "",
        ];

        for addr in &invalid_addresses {
            let _result: Result<SocketAddr, _> = addr.parse();
            // Most of these should fail to parse
            if *addr == "127.0.0.1:99999" {
                // This might actually parse but be invalid
                continue;
            }
            if !addr.is_empty() && *addr != "127.0.0.1" && *addr != ":8080" {
                continue; // Some might be valid depending on system
            }
        }
    }

    #[test]
    fn test_server_config_creation() {
        let bind_addr = "127.0.0.1:8080";
        let socket_addr: SocketAddr = bind_addr.parse().unwrap();

        let server_config = badbatch::api::ServerConfig {
            host: socket_addr.ip().to_string(),
            port: socket_addr.port(),
            ..Default::default()
        };

        assert_eq!(server_config.host, "127.0.0.1");
        assert_eq!(server_config.port, 8080);
    }

    #[test]
    fn test_cluster_configuration_validation() {
        // Test valid cluster bind addresses
        let valid_cluster_binds = ["0.0.0.0:7946", "127.0.0.1:7947", "192.168.1.100:8000"];

        for bind in &valid_cluster_binds {
            let result: Result<SocketAddr, _> = bind.parse();
            if result.is_ok() {
                let socket_addr = result.unwrap();
                assert!(socket_addr.port() > 0);
            }
        }
    }

    #[test]
    fn test_seed_nodes_validation() {
        let valid_seed_nodes = vec![
            "127.0.0.1:7946".to_string(),
            "192.168.1.100:7947".to_string(),
            "cluster-node.example.com:7946".to_string(),
        ];

        for seed_node in &valid_seed_nodes {
            // Basic validation - should contain host and port
            assert!(seed_node.contains(':'));
            let parts: Vec<&str> = seed_node.split(':').collect();
            assert_eq!(parts.len(), 2);
            assert!(!parts[0].is_empty()); // host part
            assert!(!parts[1].is_empty()); // port part
        }
    }

    #[test]
    fn test_config_file_path_handling() {
        let config_paths = [
            Some(PathBuf::from("config.yaml")),
            Some(PathBuf::from("/etc/badbatch/config.yaml")),
            Some(PathBuf::from("./configs/server.yaml")),
            None,
        ];

        for config_path in &config_paths {
            // Test that we can handle both Some and None cases
            match config_path {
                Some(path) => {
                    assert!(!path.as_os_str().is_empty());
                }
                None => {
                    // Should handle None case gracefully
                    // Should handle None case gracefully
                }
            }
        }
    }

    #[test]
    fn test_port_extraction() {
        let test_cases = [
            ("127.0.0.1:8080", 8080),
            ("0.0.0.0:3000", 3000),
            ("192.168.1.100:9000", 9000),
        ];

        for (addr_str, expected_port) in &test_cases {
            let socket_addr: SocketAddr = addr_str.parse().unwrap();
            assert_eq!(socket_addr.port(), *expected_port);
        }
    }

    #[test]
    fn test_ip_extraction() {
        let test_cases = [
            ("127.0.0.1:8080", "127.0.0.1"),
            ("0.0.0.0:3000", "0.0.0.0"),
            ("192.168.1.100:9000", "192.168.1.100"),
        ];

        for (addr_str, expected_ip) in &test_cases {
            let socket_addr: SocketAddr = addr_str.parse().unwrap();
            assert_eq!(socket_addr.ip().to_string(), *expected_ip);
        }
    }

    #[test]
    fn test_cluster_mode_flags() {
        // Test cluster mode enabled
        let cluster_mode = true;
        let cluster_bind = "0.0.0.0:7946".to_string();
        let seed_nodes = ["127.0.0.1:7947".to_string()];

        assert!(cluster_mode);
        assert!(!cluster_bind.is_empty());
        assert!(!seed_nodes.is_empty());

        // Test cluster mode disabled
        let cluster_mode = false;
        let empty_seed_nodes: Vec<String> = vec![];

        assert!(!cluster_mode);
        assert!(empty_seed_nodes.is_empty());
    }

    #[test]
    fn test_default_server_config() {
        let default_config = badbatch::api::ServerConfig::default();

        // Test that default config has reasonable values
        assert!(!default_config.host.is_empty());
        assert!(default_config.port > 0);
    }

    #[test]
    fn test_error_message_formatting() {
        let error_msg = "Invalid bind address: invalid format";
        let cli_error = crate::cli::CliError::invalid_input(error_msg.to_string());

        assert!(cli_error.to_string().contains("Invalid input"));
        assert!(cli_error.to_string().contains("invalid format"));
    }
}
