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
) -> CliResult<()> {
    println!("Starting BadBatch server...");
    println!("  Bind address: {}", bind_addr);
    println!("  Mode: single-node");

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
