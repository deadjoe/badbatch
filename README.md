<div align="center">
  <img src="badbatch.png" alt="BadBatch Logo" width="200"/>

  # BadBatch Disruptor Engine

  [![License: AGPL v3](https://img.shields.io/badge/License-AGPL%20v3-blue.svg)](https://www.gnu.org/licenses/agpl-3.0)
  [![Tests](https://github.com/deadjoe/badbatch/workflows/Tests/badge.svg)](https://github.com/deadjoe/badbatch/actions)

</div>

> A high-performance, lock-free disruptor engine written in Rust, implementing the LMAX Disruptor pattern with modern Rust optimizations and disruptor-rs inspired features.

## 🚀 Features

### 🔥 LMAX Disruptor Core Engine

- **Lock-free ring buffer** with power-of-2 sizing for maximum performance
- **Multiple wait strategies**: Blocking, BusySpin, Yielding, Sleeping
- **Event processors** with batch processing capabilities
- **Single and multi-producer** support with bitmap optimization
- **Comprehensive exception handling** with custom error types
- **Sequence barriers** and dependency management
- **Event factories, handlers, and translators** for flexible event processing
- **Full compatibility** with original LMAX Disruptor patterns

### ⚡ Modern Rust Optimizations (inspired by disruptor-rs)

- **Simplified Producer API**: Closure-based publishing with `producer.publish(|event| { ... })`
- **Batch Publishing**: Efficient `BatchIterMut` for zero-copy batch operations
- **Builder Pattern**: Fluent API with `build_single_producer(size, factory, wait_strategy)`
- **Thread Management**: CPU affinity support with `ThreadBuilder::new().pin_at_core(1)`
- **Elegant Consumer**: Automatic lifecycle management with `ElegantConsumer::new()`
- **Simple Wait Strategies**: Streamlined strategies for easier usage

### 🌐 REST API Interface (Framework Ready)

- **HTTP API framework** with Axum integration
- **Disruptor lifecycle management** endpoints (create, start, stop, status)
- **Event publishing** endpoints (single, batch)
- **Real-time monitoring** and metrics collection
- **Health checks** and system status reporting
- **Standardized error handling** with proper HTTP responses
- **Middleware support** (CORS, logging, timeout, rate limiting)
- **Extensible architecture** for custom endpoints

### 🔗 Distributed Cluster (Foundation)

- **Gossip protocol** framework (SWIM-style membership)
- **Node discovery** and membership management structure
- **Service registration** and discovery interfaces
- **Health monitoring** and failure detection mechanisms
- **Event replication** framework foundation
- **Cluster state management** and coordination primitives
- **Configurable architecture** for distributed scenarios

### 🛠️ Command Line Interface

- **Comprehensive CLI** with server, disruptor, event, and monitor commands
- **HTTP client** integration for API communication
- **Configuration management** (YAML/JSON support)
- **Multiple output formats** (JSON, YAML, table)
- **Progress indicators** and user-friendly output
- **Performance testing** and load generation capabilities
- **Real-time monitoring** and metrics display
- **Extensible command structure** for custom operations

## 📊 Performance & Safety

- **Zero-cost abstractions** with Rust's type system
- **Memory safety** without garbage collection
- **Thread safety** guaranteed at compile time
- **Lock-free data structures** for maximum throughput
- **NUMA-aware** design considerations

## 🏗️ Architecture

BadBatch follows a modular, layered architecture:

```
┌─────────────────────────────────────────────────────────────┐
│                    BadBatch Disruptor Engine                │
├─────────────────────────────────────────────────────────────┤
│  🌐 REST API Layer                                          │
│  ├─ Disruptor Management (CRUD)                            │
│  ├─ Event Publishing (Single/Batch)                        │
│  ├─ Monitoring & Metrics                                   │
│  └─ Health Checks                                          │
├─────────────────────────────────────────────────────────────┤
│  🔗 Cluster Layer                                           │
│  ├─ Gossip Protocol (Node Discovery)                       │
│  ├─ Membership Management                                   │
│  ├─ Service Discovery                                       │
│  ├─ Health Monitoring                                       │
│  └─ Event Replication                                       │
├─────────────────────────────────────────────────────────────┤
│  ⚡ Disruptor Core                                          │
│  ├─ Ring Buffer (Lock-free)                                │
│  ├─ Event Processors (Multi-threaded)                      │
│  ├─ Wait Strategies (4 types)                              │
│  ├─ Producer Types (Single/Multi)                          │
│  └─ Exception Handling                                      │
└─────────────────────────────────────────────────────────────┘
```

## 🚀 Quick Start

### Installation

#### From Source
```bash
git clone https://github.com/deadjoe/badbatch.git
cd badbatch
cargo build --release
```

### Basic Usage

#### 1. Start the Server
```bash
# Start standalone server
badbatch server --bind 0.0.0.0:8080

# Start with cluster mode
badbatch server --bind 0.0.0.0:8080 --cluster --cluster-bind 0.0.0.0:7946
```

#### 2. Create a Disruptor
```bash
# Create a disruptor with default settings
badbatch disruptor create my-disruptor

# Create with custom settings
badbatch disruptor create my-disruptor \
  --size 2048 \
  --producer multi \
  --wait-strategy busy-spin
```

#### 3. Publish Events
```bash
# Publish a single event
badbatch event publish my-disruptor '{"message": "Hello, World!"}'

# Publish batch events
badbatch event batch my-disruptor '[
  {"data": "event1"},
  {"data": "event2"},
  {"data": "event3"}
]'

# Generate performance test events
badbatch event generate my-disruptor --count 10000 --rate 1000
```

#### 4. Monitor Performance
```bash
# Show system metrics
badbatch monitor system

# Show disruptor metrics
badbatch monitor disruptor my-disruptor

# Real-time monitoring
badbatch monitor watch --type disruptor --target my-disruptor
```

## 📖 Documentation

### API Endpoints

| Method | Endpoint | Description |
|--------|----------|-------------|
| `GET` | `/` | API root with documentation |
| `GET` | `/health` | System health check |
| `GET` | `/metrics` | System metrics |
| `GET` | `/api/v1/health` | API health check |
| `GET` | `/api/v1/metrics` | API system metrics |
| `GET` | `/api/v1/version` | API version |
| `POST` | `/api/v1/disruptor` | Create disruptor |
| `GET` | `/api/v1/disruptor` | List disruptors |
| `GET` | `/api/v1/disruptor/{id}` | Get disruptor |
| `DELETE` | `/api/v1/disruptor/{id}` | Delete disruptor |
| `POST` | `/api/v1/disruptor/{id}/start` | Start disruptor |
| `POST` | `/api/v1/disruptor/{id}/stop` | Stop disruptor |
| `POST` | `/api/v1/disruptor/{id}/pause` | Pause disruptor |
| `POST` | `/api/v1/disruptor/{id}/resume` | Resume disruptor |
| `POST` | `/api/v1/disruptor/{id}/events` | Publish single event |
| `POST` | `/api/v1/disruptor/{id}/events/batch` | Publish batch events |
| `GET` | `/api/v1/disruptor/{id}/events` | Query events |
| `GET` | `/api/v1/disruptor/{id}/events/{sequence}` | Get specific event |
| `GET` | `/api/v1/disruptor/{id}/metrics` | Disruptor metrics |
| `GET` | `/api/v1/disruptor/{id}/health` | Disruptor health |
| `GET` | `/api/v1/disruptor/{id}/status` | Disruptor status |
| `POST` | `/api/v1/batch/events` | Publish batch to multiple disruptors |

### Configuration

BadBatch supports YAML and JSON configuration files:

```yaml
# config.yaml
server:
  bind_addr: "0.0.0.0:8080"
  cluster_mode: true
  cluster_bind_addr: "0.0.0.0:7946"
  seed_nodes:
    - "10.0.0.1:7946"
    - "10.0.0.2:7946"

cluster:
  gossip_interval_ms: 200
  probe_interval_secs: 1
  enable_compression: true
  enable_encryption: false

defaults:
  buffer_size: 1024
  producer_type: "single"
  wait_strategy: "blocking"
  batch_size: 100
```

### Programming API

#### Traditional LMAX Disruptor API

```rust
use badbatch::disruptor::{
    Disruptor, ProducerType, BlockingWaitStrategy, DefaultEventFactory,
    EventHandler, EventTranslator, Result,
};

#[derive(Debug, Default)]
struct MyEvent {
    value: i64,
    message: String,
}

// Event handler implementation
struct MyEventHandler;

impl EventHandler<MyEvent> for MyEventHandler {
    fn on_event(&mut self, event: &mut MyEvent, sequence: i64, end_of_batch: bool) -> Result<()> {
        println!("Processing event {} with value {} (end_of_batch: {})",
                 sequence, event.value, end_of_batch);
        Ok(())
    }
}

// Create and configure the Disruptor
let factory = DefaultEventFactory::<MyEvent>::new();
let mut disruptor = Disruptor::new(
    factory,
    1024, // Buffer size (must be power of 2)
    ProducerType::Single,
    Box::new(BlockingWaitStrategy::new()),
)?
.handle_events_with(MyEventHandler)
.build();

// Start the Disruptor
disruptor.start()?;

// Publish events using EventTranslator
struct MyEventTranslator {
    value: i64,
    message: String,
}

impl EventTranslator<MyEvent> for MyEventTranslator {
    fn translate_to(&self, event: &mut MyEvent, _sequence: i64) {
        event.value = self.value;
        event.message = self.message.clone();
    }
}

let translator = MyEventTranslator {
    value: 42,
    message: "Hello, World!".to_string(),
};
disruptor.publish_event(translator)?;

// Shutdown when done
disruptor.shutdown()?;
```

#### Modern disruptor-rs Inspired API

```rust
use badbatch::disruptor::{
    build_single_producer, Producer, ElegantConsumer, RingBuffer,
    simple_wait_strategy::BusySpin, event_factory::ClosureEventFactory,
};
use std::sync::Arc;

#[derive(Debug, Default)]
struct MyEvent {
    value: i64,
}

// Simple producer with closure-based publishing
let mut producer = build_single_producer(1024, || MyEvent::default(), BusySpin)
    .handle_events_with(|event, sequence, end_of_batch| {
        println!("Processing event {} with value {} (batch_end: {})",
                 sequence, event.value, end_of_batch);
    })
    .build();

// Publish events with closures
producer.publish(|event| {
    event.value = 42;
});

// Batch publishing
producer.batch_publish(5, |batch| {
    for (i, event) in batch.enumerate() {
        event.value = i as i64;
    }
});

// Elegant consumer with CPU affinity
let factory = ClosureEventFactory::new(|| MyEvent::default());
let ring_buffer = Arc::new(RingBuffer::new(1024, factory)?);

let consumer = ElegantConsumer::with_affinity(
    ring_buffer,
    |event, sequence, end_of_batch| {
        println!("Processing: {} at {}", event.value, sequence);
    },
    BusySpin,
    1, // Pin to CPU core 1
)?;

// Graceful shutdown
consumer.shutdown()?;
```

## 🔧 Development

### Prerequisites
- Rust 1.70 or later
- Git

### Building
```bash
git clone https://github.com/deadjoe/badbatch.git
cd badbatch
cargo build
```

### Testing

```bash
# Run all tests (271 tests total)
cargo test

# Run comprehensive test suite with quality checks
bash scripts/test-all.sh

# Run specific test categories
cargo test --lib                    # Unit tests (149 tests)
cargo test --bin                    # CLI tests (110 tests)
cargo test --test '*'               # Integration tests (12 tests)
cargo test --doc                    # Documentation tests (10 tests)

# Run benchmarks
cargo bench
```

### Code Quality

```bash
# Comprehensive quality checks (recommended)
bash scripts/test-all.sh

# Individual quality checks
cargo fmt                           # Format code
cargo clippy --all-targets --all-features -- -D warnings  # Lint code
cargo audit                         # Security audit
cargo deny check                    # Dependency management

# Documentation
cargo doc --no-deps --open          # Generate and open docs
cargo test --doc                    # Test documentation examples

# Coverage analysis (requires cargo-llvm-cov)
cargo llvm-cov --lib --html         # Generate HTML coverage report
```

## 📈 Performance

BadBatch is designed for high-performance event processing with the following characteristics:

### Core Performance Features

- **Lock-free architecture**: Zero-cost abstractions with no locks in hot paths
- **Memory efficiency**: Pre-allocated ring buffer with zero runtime allocation
- **CPU optimization**: Cache-friendly data structures with proper padding
- **Rust advantages**: Memory safety without runtime overhead
- **Mechanical sympathy**: NUMA-aware design and CPU affinity support

### Optimization Techniques

- **Cache Line Padding**: Uses `crossbeam_utils::CachePadded` to prevent false sharing
- **Bitmap Optimization**: O(1) availability checking for large buffers (inspired by disruptor-rs)
- **Batch Processing**: Automatic batching reduces coordination overhead
- **Bit Manipulation**: Fast modulo operations using bit masks for power-of-2 buffer sizes
- **Memory Layout**: Optimal data structures (`Box<[UnsafeCell<T>]>`) for better cache locality

## 🤝 Contributing

We welcome contributions! Please follow these steps:

### Development Process
1. Fork the repository
2. Create a feature branch
3. Make your changes
4. Add tests
5. Run the test suite
6. Submit a pull request

### Code of Conduct
This project adheres to the [Rust Code of Conduct](https://www.rust-lang.org/policies/code-of-conduct).

## 📄 License

This project is licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).

See the [LICENSE](LICENSE) file for details.

## 🙏 Acknowledgments

- [LMAX Disruptor](https://github.com/LMAX-Exchange/disruptor) - Original Java implementation and design patterns
- [disruptor-rs](https://github.com/sklose/disruptor) - Rust implementation inspiration for modern API design
- [Rust Community](https://www.rust-lang.org/community) - For the amazing ecosystem and safety guarantees
- [Tokio](https://tokio.rs/) - Async runtime for network components
- [Axum](https://github.com/tokio-rs/axum) - Web framework for REST API
- [crossbeam-utils](https://github.com/crossbeam-rs/crossbeam) - Cache-friendly atomic operations

## 📞 Support

- 🐛 [Issue Tracker](https://github.com/deadjoe/badbatch/issues)
- 💬 [Discussions](https://github.com/deadjoe/badbatch/discussions)
- 📖 [DESIGN.md](docs/DESIGN.md) - Comprehensive design documentation

---

**Made with ❤️ in Rust**
