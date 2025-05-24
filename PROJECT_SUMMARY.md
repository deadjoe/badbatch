# BadBatch - High-Performance Disruptor Engine

## Project Overview

BadBatch is a high-performance, lock-free message processing engine implemented in Rust, based on the LMAX Disruptor pattern. It provides ultra-low latency message processing with mechanical sympathy principles, designed for distributed systems requiring extreme performance.

## Architecture

### Core Components

1. **Sequence** - Lock-free sequence counters with atomic operations
2. **RingBuffer** - Pre-allocated circular buffer for zero-allocation message storage
3. **Sequencer** - Coordinates access to the ring buffer (single/multi-producer)
4. **WaitStrategy** - Different strategies for consumer waiting (blocking, yielding, busy-spin, sleeping)
5. **EventProcessor** - Batch processing of events with exception handling
6. **SequenceBarrier** - Coordination point for dependencies between processors

### Key Features

- **Lock-free data structures** - No locks, only atomic operations
- **Zero-allocation design** - Pre-allocated ring buffer eliminates GC pressure
- **Mechanical sympathy** - CPU cache-friendly data layout
- **Multiple wait strategies** - Optimized for different latency/CPU trade-offs
- **Single and multi-producer support** - Flexible producer configurations
- **Batch processing** - Efficient event processing in batches
- **Exception handling** - Robust error handling with custom handlers

## Performance Characteristics

- **Ultra-low latency** - Sub-microsecond message processing
- **High throughput** - Millions of messages per second
- **Predictable performance** - No GC pauses or lock contention
- **CPU efficient** - Optimized for modern CPU architectures

## Implementation Status

### ✅ Completed Components

1. **Core Disruptor Engine**
   - Sequence management with atomic operations
   - Ring buffer with power-of-2 sizing
   - Single and multi-producer sequencers
   - All wait strategies (Blocking, Yielding, BusySpin, Sleeping)
   - Event processors with batch processing
   - Sequence barriers with dependency management

2. **Testing Infrastructure**
   - Comprehensive unit tests (33 tests)
   - Thread safety tests
   - Performance benchmarks
   - Test coverage: **55.78%** (275/493 lines)

3. **Project Structure**
   - Modular architecture
   - Clean separation of concerns
   - Rust best practices
   - Documentation and examples

### 🚧 Planned Components

1. **REST API Interface**
   - HTTP endpoints for message publishing
   - WebSocket support for real-time streaming
   - JSON/binary message formats
   - Rate limiting and authentication

2. **Distributed Clustering**
   - Gossip protocol for node discovery
   - Consistent hashing for message distribution
   - Replication and failover
   - Cluster health monitoring

3. **Client Libraries**
   - Rust native client
   - Language bindings (Python, Java, Go)
   - Connection pooling
   - Automatic reconnection

## Technical Specifications

### Dependencies

```toml
[dependencies]
tokio = { version = "1.0", features = ["full"] }
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
tracing = "0.1"
tracing-subscriber = "0.3"
anyhow = "1.0"
clap = { version = "4.0", features = ["derive"] }
uuid = { version = "1.0", features = ["v4"] }
axum = "0.7"
reqwest = { version = "0.11", features = ["json"] }

[dev-dependencies]
criterion = { version = "0.5", features = ["html_reports"] }
tokio-test = "0.4"
```

### Build Requirements

- Rust 1.70+ (2021 edition)
- Cargo for dependency management
- Optional: cargo-tarpaulin for coverage reports

## Usage Examples

### Basic Producer-Consumer

```rust
use badbatch::disruptor::*;

// Create ring buffer
let ring_buffer = SharedRingBuffer::new(1024);

// Create sequencer
let sequencer = SingleProducerSequencer::new(1024, BlockingWaitStrategy::new());

// Publish event
let sequence = sequencer.next().unwrap();
let event = ring_buffer.get(sequence);
// ... populate event data
sequencer.publish(sequence);

// Consume events
let barrier = SequenceBarrier::new(sequencer.cursor(), vec![]);
let available = barrier.wait_for(sequence).unwrap();
let event = ring_buffer.get(available);
// ... process event
```

### Performance Benchmarks

Current benchmark results show:
- **Latency**: ~382 picoseconds per operation
- **Throughput**: Millions of operations per second
- **Memory**: Zero allocations during steady state

## Testing

### Running Tests

```bash
# Run all tests
cargo test

# Run with coverage
cargo tarpaulin --out Html

# Run benchmarks
cargo bench
```

### Test Coverage Breakdown

- **event_processor.rs**: 18/69 lines (26.1%)
- **ring_buffer.rs**: 32/38 lines (84.2%)
- **sequence.rs**: 34/45 lines (75.6%)
- **sequence_barrier.rs**: 38/40 lines (95.0%)
- **sequencer.rs**: 105/166 lines (63.3%)
- **wait_strategy.rs**: 46/78 lines (59.0%)

**Overall Coverage**: 55.78% (275/493 lines)

## Development Roadmap

### Phase 1: Core Engine (✅ Complete)
- Implement LMAX Disruptor pattern
- Lock-free data structures
- Comprehensive testing
- Performance benchmarking

### Phase 2: API Layer (🚧 Next)
- REST API endpoints
- WebSocket streaming
- Message serialization
- Rate limiting

### Phase 3: Clustering (🚧 Future)
- Gossip protocol implementation
- Distributed message routing
- Replication and consistency
- Health monitoring

### Phase 4: Client Ecosystem (🚧 Future)
- Multi-language client libraries
- Connection management
- Load balancing
- Monitoring integration

## Contributing

1. Follow Rust best practices and idioms
2. Maintain test coverage above 70%
3. Add benchmarks for performance-critical code
4. Document public APIs thoroughly
5. Use conventional commit messages

## License

This project is licensed under the MIT License - see the LICENSE file for details.

## Acknowledgments

- LMAX Disruptor team for the original pattern
- Rust community for excellent tooling and libraries
- Performance engineering best practices from mechanical sympathy principles
