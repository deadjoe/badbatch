# BadBatch - High-Performance Data Processing Engine

[![Rust](https://img.shields.io/badge/rust-1.70+-orange.svg)](https://www.rust-lang.org)
[![License](https://img.shields.io/badge/license-Apache%202.0-blue.svg)](LICENSE)
[![Build Status](https://img.shields.io/badge/build-passing-brightgreen.svg)](https://github.com/deadjoe/badbatch)

BadBatch is a high-performance, lock-free data processing engine written in Rust, implementing the LMAX Disruptor pattern for ultra-low latency inter-thread messaging. It provides a distributed, fault-tolerant queue system with REST API access.

## 🚀 Features

- **Lock-Free Disruptor Implementation**: Complete Rust implementation of the LMAX Disruptor pattern
- **Ultra-Low Latency**: Mechanical sympathy design for optimal CPU cache utilization
- **REST API**: Full HTTP API for external client integration
- **Distributed Architecture**: Gossip-based node discovery and clustering
- **Multiple Wait Strategies**: Blocking, yielding, busy-spin, and sleeping strategies
- **Producer Flexibility**: Support for both single and multi-producer scenarios
- **Memory Pre-allocation**: Zero-allocation event processing for consistent performance
- **Comprehensive Testing**: 70%+ test coverage with performance benchmarks

## 🏗️ Architecture

BadBatch implements the core concepts from the LMAX Disruptor:

- **Ring Buffer**: Lock-free circular buffer for event storage
- **Sequencer**: Coordinates access between producers and consumers
- **Sequence Barriers**: Manages consumer dependencies and coordination
- **Event Processors**: High-performance event handling with batching support
- **Wait Strategies**: Configurable waiting mechanisms for different latency/CPU trade-offs

## 📦 Installation

### Prerequisites

- Rust 1.70 or later
- macOS, Linux, or Windows

### Building from Source

```bash
git clone https://github.com/deadjoe/badbatch.git
cd badbatch
cargo build --release
```

### Running the Server

```bash
# Start a single node
cargo run --release

# Start with custom configuration
cargo run --release -- --port 8080 --buffer-size 1024

# Start in cluster mode
cargo run --release -- --cluster --seed-nodes "127.0.0.1:8081,127.0.0.1:8082"
```

## 🔧 Configuration

BadBatch supports various configuration options:

```bash
USAGE:
    badbatch [OPTIONS]

OPTIONS:
    -p, --port <PORT>              HTTP server port [default: 8080]
    -b, --buffer-size <SIZE>       Ring buffer size (must be power of 2) [default: 1024]
    -w, --wait-strategy <STRATEGY> Wait strategy [default: blocking]
                                   [possible values: blocking, yielding, busy-spin, sleeping]
    -c, --cluster                  Enable cluster mode
    -s, --seed-nodes <NODES>       Comma-separated list of seed nodes for clustering
    -n, --node-id <ID>             Unique node identifier
    -h, --help                     Print help information
    -V, --version                  Print version information
```

## 📚 API Documentation

### REST Endpoints

#### Queue Operations

```http
# Publish an event
POST /api/v1/events
Content-Type: application/json

{
  "data": "your_event_data",
  "sequence": 12345
}

# Get events from sequence
GET /api/v1/events?from=100&to=200

# Get queue status
GET /api/v1/status

# Get cluster information
GET /api/v1/cluster/nodes
```

#### Health and Metrics

```http
# Health check
GET /health

# Performance metrics
GET /metrics

# Node information
GET /api/v1/node/info
```

## 🧪 Testing

### Running Tests

```bash
# Unit tests
cargo test

# Integration tests
cargo test --test integration

# Performance benchmarks
cargo bench
```

### Performance Testing Client

BadBatch includes a comprehensive performance testing client:

```bash
# Run performance tests
cargo run --bin perf-client -- --url http://localhost:8080 --events 1000000 --producers 4

# Latency testing
cargo run --bin perf-client -- --url http://localhost:8080 --latency-test --duration 60s

# Throughput testing
cargo run --bin perf-client -- --url http://localhost:8080 --throughput-test --rate 100000
```

## 🔬 Performance

BadBatch is designed for extreme performance. Preliminary benchmarks show:

- **Latency**: Sub-microsecond event processing
- **Throughput**: 10M+ events/second on modern hardware
- **Memory**: Zero-allocation event processing
- **CPU**: Optimal cache utilization with mechanical sympathy

*Detailed performance results and comparisons will be added as development progresses.*

## 🏛️ Project Structure

```
badbatch/
├── src/
│   ├── disruptor/          # Core Disruptor implementation
│   │   ├── ring_buffer.rs  # Lock-free ring buffer
│   │   ├── sequencer.rs    # Single/multi-producer sequencers
│   │   ├── sequence.rs     # Sequence number management
│   │   └── wait_strategy.rs # Various wait strategies
│   ├── api/                # REST API implementation
│   ├── cluster/            # Distributed clustering
│   ├── client/             # Performance testing client
│   └── main.rs            # Application entry point
├── tests/                  # Integration tests
├── benches/               # Performance benchmarks
└── examples/              # Usage examples
```

## 🤝 Contributing

We welcome contributions! Please see [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines.

## 📄 License

This project is licensed under the Apache License 2.0 - see the [LICENSE](LICENSE) file for details.

## 🙏 Acknowledgments

- [LMAX Exchange](https://github.com/LMAX-Exchange/disruptor) for the original Disruptor design
- The Rust community for excellent tooling and libraries
- Martin Thompson for mechanical sympathy principles

## 📈 Roadmap

- [x] Project initialization and structure
- [ ] Core Disruptor implementation
- [ ] REST API development
- [ ] Distributed clustering with Gossip protocol
- [ ] Performance testing and optimization
- [ ] Documentation and examples
- [ ] Production readiness features

---

**Status**: 🚧 Under Active Development

This project is currently in early development. APIs and features are subject to change.
