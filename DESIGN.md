# BadBatch Disruptor Engine - Design Document

## Table of Contents

1. [Overview](#overview)
2. [Architecture](#architecture)
3. [Core Components](#core-components)
4. [Design Patterns](#design-patterns)
5. [Performance Considerations](#performance-considerations)
6. [Concurrency Model](#concurrency-model)
7. [Memory Management](#memory-management)
8. [Error Handling](#error-handling)
9. [API Design](#api-design)
10. [Distributed Systems Design](#distributed-systems-design)
11. [Testing Strategy](#testing-strategy)
12. [Future Considerations](#future-considerations)

## Overview

BadBatch is a high-performance, distributed disruptor engine written in Rust, implementing the LMAX Disruptor pattern with modern distributed systems capabilities. The design prioritizes:

- **Zero-cost abstractions** leveraging Rust's type system
- **Memory safety** without garbage collection overhead
- **Lock-free concurrency** for maximum throughput
- **Modular architecture** for extensibility
- **Distributed coordination** for horizontal scaling

### Design Goals

1. **Performance**: Achieve sub-microsecond latency with millions of operations per second
2. **Safety**: Compile-time guarantees for memory and thread safety
3. **Scalability**: Horizontal scaling through distributed clustering
4. **Maintainability**: Clean, modular architecture with comprehensive testing
5. **Compatibility**: API compatibility with LMAX Disruptor concepts

## Architecture

### High-Level Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                    BadBatch Disruptor Engine                │
├─────────────────────────────────────────────────────────────┤
│  🌐 Presentation Layer                                      │
│  ├─ REST API (Axum)                                        │
│  ├─ CLI Interface (Clap)                                   │
│  └─ Configuration Management                                │
├─────────────────────────────────────────────────────────────┤
│  🔗 Distributed Layer                                       │
│  ├─ Cluster Management                                      │
│  ├─ Service Discovery                                       │
│  ├─ Health Monitoring                                       │
│  └─ Event Replication                                       │
├─────────────────────────────────────────────────────────────┤
│  ⚡ Disruptor Core Layer                                    │
│  ├─ Ring Buffer                                             │
│  ├─ Event Processors                                        │
│  ├─ Wait Strategies                                         │
│  └─ Sequence Management                                     │
├─────────────────────────────────────────────────────────────┤
│  🔧 Foundation Layer                                        │
│  ├─ Memory Management                                       │
│  ├─ Error Handling                                          │
│  ├─ Logging & Metrics                                       │
│  └─ Configuration                                           │
└─────────────────────────────────────────────────────────────┘
```

### Module Structure

```
src/
├── lib.rs                 # Library root and public API
├── disruptor/             # Core disruptor implementation
│   ├── mod.rs
│   ├── ring_buffer.rs     # Lock-free ring buffer
│   ├── sequence.rs        # Sequence management
│   ├── wait_strategy.rs   # Wait strategies
│   ├── event_processor.rs # Event processing
│   ├── producer.rs        # Event producers
│   └── builder.rs         # Disruptor builder pattern
├── api/                   # REST API layer
│   ├── mod.rs
│   ├── handlers.rs        # HTTP request handlers
│   ├── models.rs          # API data models
│   ├── routes.rs          # Route definitions
│   ├── middleware.rs      # HTTP middleware
│   └── server.rs          # Server implementation
├── cluster/               # Distributed clustering
│   ├── mod.rs
│   ├── node.rs            # Node management
│   ├── gossip.rs          # Gossip protocol
│   ├── membership.rs      # Cluster membership
│   ├── discovery.rs       # Service discovery
│   ├── health.rs          # Health monitoring
│   ├── replication.rs     # Event replication
│   ├── config.rs          # Cluster configuration
│   └── error.rs           # Cluster-specific errors
└── bin/                   # CLI application
    ├── badbatch.rs        # Main CLI entry point
    ├── cli/               # CLI modules
    ├── client.rs          # HTTP client
    └── config.rs          # Configuration management
```

## Core Components

### 1. Ring Buffer

The ring buffer is the heart of the disruptor pattern, implemented as a lock-free circular buffer.

**Key Design Decisions:**

- **Power-of-2 sizing**: Enables fast modulo operations using bitwise AND
- **Padding**: Cache line padding to prevent false sharing
- **Atomic sequences**: Lock-free coordination using atomic operations
- **Generic events**: Type-safe event storage with zero-cost abstractions

```rust
pub struct RingBuffer<T> {
    buffer: Vec<UnsafeCell<T>>,
    buffer_size: usize,
    index_mask: usize,
    cursor: AtomicU64,
}
```

**Memory Layout:**
```
Cache Line 1: [cursor] [padding...]
Cache Line 2: [buffer_size] [index_mask] [padding...]
Cache Line 3+: [event_0] [event_1] ... [event_n]
```

### 2. Sequence Management

Sequences coordinate between producers and consumers without locks.

**Components:**
- **Cursor**: Producer's current position
- **Gating Sequences**: Consumer positions that gate producer progress
- **Sequence Barriers**: Coordination points for dependencies

```rust
pub struct Sequence {
    value: AtomicU64,
    _padding: [u8; CACHE_LINE_SIZE - 8],
}
```

### 3. Wait Strategies

Different strategies for consumer waiting, each optimized for specific use cases:

- **BlockingWaitStrategy**: Low CPU usage, higher latency
- **BusySpinWaitStrategy**: Lowest latency, high CPU usage
- **YieldingWaitStrategy**: Balanced approach
- **SleepingWaitStrategy**: Progressive backoff

### 4. Event Processors

Event processors handle the consumption and processing of events.

**Types:**
- **BatchEventProcessor**: Processes events in batches
- **WorkerPool**: Multiple workers processing from the same ring buffer
- **EventHandler**: User-defined event processing logic

## Design Patterns

### 1. Builder Pattern

Disruptor construction uses the builder pattern for type-safe configuration:

```rust
let disruptor = DisruptorBuilder::new()
    .buffer_size(1024)
    .single_producer()
    .blocking_wait_strategy()
    .build()?;
```

### 2. Strategy Pattern

Wait strategies implement a common interface with different behaviors:

```rust
pub trait WaitStrategy: Send + Sync {
    fn wait_for(&self, sequence: u64, cursor: &Sequence,
                dependent_sequence: &Sequence, barrier: &SequenceBarrier)
                -> Result<u64, DisruptorError>;
}
```

### 3. Observer Pattern

Event handlers observe and react to events:

```rust
pub trait EventHandler<T>: Send {
    fn on_event(&mut self, event: &T, sequence: u64, end_of_batch: bool);
    fn on_start(&mut self) {}
    fn on_shutdown(&mut self) {}
}
```

### 4. Factory Pattern

Event factories create and initialize events:

```rust
pub trait EventFactory<T>: Send + Sync {
    fn new_instance(&self) -> T;
}
```

## Performance Considerations

### 1. Cache Optimization

- **Cache line padding**: Prevents false sharing between threads
- **Sequential access**: Ring buffer design optimizes for CPU cache prefetching
- **Memory locality**: Related data structures are co-located

### 2. Lock-Free Design

- **Atomic operations**: All coordination uses atomic compare-and-swap
- **Memory ordering**: Careful use of memory ordering for correctness
- **ABA problem**: Avoided through sequence-based coordination

### 3. Zero-Copy Operations

- **In-place updates**: Events are updated in the ring buffer
- **Reference passing**: No unnecessary copying of event data
- **Batch processing**: Amortizes coordination overhead

### 4. NUMA Awareness

- **Thread affinity**: Processors can be bound to specific CPU cores
- **Memory allocation**: Ring buffer allocated on appropriate NUMA node
- **Topology detection**: Runtime detection of system topology

## Concurrency Model

### 1. Single Writer Principle

- **One producer per ring buffer**: Eliminates contention
- **Multiple consumers**: Supported through sequence barriers
- **Dependency chains**: Complex processing topologies

### 2. Memory Model

- **Acquire-Release semantics**: Ensures proper ordering
- **Volatile reads/writes**: Prevents compiler optimizations
- **Memory barriers**: Explicit control over memory ordering

### 3. Thread Safety

- **Send + Sync bounds**: Compile-time thread safety guarantees
- **Immutable sharing**: Shared state is immutable where possible
- **Interior mutability**: Careful use of UnsafeCell for performance

## Memory Management

### 1. Allocation Strategy

- **Pre-allocation**: Ring buffer and events allocated upfront
- **Pool reuse**: Event objects reused to minimize allocation
- **RAII**: Rust's ownership system ensures proper cleanup

### 2. Memory Layout

- **Contiguous allocation**: Ring buffer uses contiguous memory
- **Alignment**: Proper alignment for atomic operations
- **Padding**: Cache line padding for performance

### 3. Garbage Collection Avoidance

- **No GC pressure**: Zero allocation in hot paths
- **Deterministic cleanup**: Predictable memory management
- **Resource management**: RAII for all resources

## Error Handling

### 1. Error Types

```rust
#[derive(Debug, thiserror::Error)]
pub enum DisruptorError {
    #[error("Ring buffer is full")]
    InsufficientCapacity,
    #[error("Sequence barrier timeout")]
    Timeout,
    #[error("Invalid configuration: {message}")]
    Configuration { message: String },
    #[error("Shutdown in progress")]
    Shutdown,
}
```

### 2. Error Propagation

- **Result types**: All fallible operations return Result
- **Error context**: Rich error information for debugging
- **Graceful degradation**: System continues operating when possible

### 3. Recovery Strategies

- **Retry logic**: Automatic retry for transient failures
- **Circuit breakers**: Prevent cascade failures
- **Fallback mechanisms**: Alternative processing paths

## API Design

### 1. REST API Principles

- **RESTful design**: Standard HTTP methods and status codes
- **Resource-oriented**: URLs represent resources, not actions
- **Stateless**: Each request contains all necessary information
- **Idempotent operations**: Safe to retry operations

### 2. Versioning Strategy

- **URL versioning**: `/api/v1/` prefix for version control
- **Backward compatibility**: Maintain compatibility within major versions
- **Deprecation policy**: Clear migration path for breaking changes

### 3. Content Negotiation

- **JSON by default**: Standard JSON for request/response bodies
- **Multiple formats**: Support for YAML, MessagePack
- **Compression**: Gzip compression for large responses

## Distributed Systems Design

### 1. Gossip Protocol

Implementation based on SWIM (Scalable Weakly-consistent Infection-style Process Group Membership):

- **Failure detection**: Probabilistic failure detection
- **Membership updates**: Efficient dissemination of membership changes
- **Network partitions**: Graceful handling of split-brain scenarios

### 2. Consistency Model

- **Eventual consistency**: Cluster state converges eventually
- **Conflict resolution**: Last-writer-wins with vector clocks
- **Partition tolerance**: System remains available during partitions

### 3. Service Discovery

- **Health-based routing**: Only route to healthy instances
- **Load balancing**: Distribute load across available nodes
- **Service registration**: Automatic registration and deregistration

## Testing Strategy

### 1. Unit Testing

- **Component isolation**: Each component tested in isolation
- **Property-based testing**: QuickCheck-style property testing
- **Mock dependencies**: Isolated testing with mocks

### 2. Integration Testing

- **End-to-end scenarios**: Complete workflows tested
- **Performance testing**: Latency and throughput validation
- **Failure injection**: Chaos engineering principles

### 3. Concurrency Testing

- **Race condition detection**: Loom for deterministic testing
- **Stress testing**: High-load scenarios
- **Deadlock detection**: Automated deadlock detection

## Future Considerations

### 1. Performance Optimizations

- **SIMD instructions**: Vectorized operations for batch processing
- **Custom allocators**: Specialized allocators for specific use cases
- **Hardware acceleration**: GPU acceleration for specific workloads

### 2. Feature Enhancements

- **Persistent storage**: Durable event storage options
- **Compression**: Event compression for storage efficiency
- **Encryption**: End-to-end encryption for sensitive data

### 3. Ecosystem Integration

- **Observability**: OpenTelemetry integration
- **Service mesh**: Istio/Linkerd integration
- **Cloud native**: Kubernetes operator

### 4. Language Bindings

- **C FFI**: C-compatible interface for other languages
- **Python bindings**: PyO3-based Python integration
- **WebAssembly**: WASM compilation for browser/edge deployment

## Implementation Details

### 1. Critical Code Paths

**Event Publishing Hot Path:**
```rust
// Zero-allocation event publishing
pub fn publish(&self, translator: impl EventTranslator<T>) -> Result<u64, DisruptorError> {
    let sequence = self.ring_buffer.next()?;
    let event = self.ring_buffer.get_mut(sequence)?;
    translator.translate_to(event, sequence);
    self.ring_buffer.publish(sequence);
    Ok(sequence)
}
```

**Consumer Processing Hot Path:**
```rust
// Batch processing for efficiency
fn process_events(&mut self) -> Result<(), DisruptorError> {
    let available_sequence = self.sequence_barrier.wait_for(self.sequence + 1)?;

    while self.sequence < available_sequence {
        self.sequence += 1;
        let event = self.ring_buffer.get(self.sequence)?;
        let end_of_batch = self.sequence == available_sequence;
        self.event_handler.on_event(event, self.sequence, end_of_batch);
    }

    self.sequence_barrier.signal();
    Ok(())
}
```

### 2. Memory Barriers and Ordering

**Critical Memory Ordering Points:**
- **Producer cursor updates**: `Ordering::Release` to publish events
- **Consumer sequence reads**: `Ordering::Acquire` to read published events
- **Gating sequence updates**: `Ordering::SeqCst` for coordination

### 3. Configuration Management

**Hierarchical Configuration:**
```yaml
# Complete configuration example
server:
  bind_addr: "0.0.0.0:8080"
  cluster_mode: true

cluster:
  node_id: "node-1"
  bind_addr: "0.0.0.0:7946"
  seed_nodes: ["10.0.0.1:7946"]
  gossip_interval_ms: 200

disruptor:
  default_buffer_size: 1024
  default_wait_strategy: "blocking"

logging:
  level: "info"
  format: "json"
```

## Security Considerations

### 1. Network Security

- **TLS encryption**: All cluster communication encrypted
- **Authentication**: Node authentication using certificates
- **Authorization**: Role-based access control for API endpoints

### 2. Memory Safety

- **Bounds checking**: All array accesses bounds-checked
- **Integer overflow**: Checked arithmetic in critical paths
- **Use-after-free**: Prevented by Rust's ownership system

### 3. Input Validation

- **API input validation**: All API inputs validated
- **Configuration validation**: Configuration files validated on load
- **Event data validation**: Optional event schema validation

## Monitoring and Observability

### 1. Metrics Collection

**Core Metrics:**
- **Throughput**: Events per second
- **Latency**: P50, P95, P99 latencies
- **Buffer utilization**: Ring buffer fill percentage
- **Error rates**: Error counts by type

### 2. Distributed Tracing

- **Request tracing**: End-to-end request tracing
- **Event correlation**: Trace events across cluster nodes
- **Performance profiling**: Detailed performance analysis

### 3. Health Checks

**Multi-level Health Checks:**
- **Process health**: Basic process liveness
- **Component health**: Individual component status
- **Cluster health**: Cluster-wide health assessment

---

This design document provides a comprehensive overview of BadBatch's architecture and design decisions. It serves as a reference for developers and architects working with or contributing to the project, covering everything from high-level architecture to implementation details and operational considerations.
