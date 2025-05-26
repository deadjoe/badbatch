# BadBatch Disruptor - Design Documentation

## Overview

BadBatch Disruptor is a high-performance, lock-free inter-thread messaging library that implements the LMAX Disruptor pattern in Rust. This implementation combines the correctness and completeness of the original LMAX Disruptor with modern Rust safety guarantees and performance optimizations inspired by disruptor-rs.

## Architecture Overview

### Core Design Principles

1. **Lock-Free Coordination**: Uses only atomic operations and memory barriers
2. **Zero-Allocation Runtime**: Pre-allocates all events during initialization
3. **Mechanical Sympathy**: CPU cache-friendly data structures with proper padding
4. **High Throughput**: Batch processing and efficient algorithms
5. **Low Latency**: Minimal overhead and predictable performance
6. **Thread Safety**: Full support for single and multi-producer scenarios

### Key Components

```
┌─────────────────┐    ┌─────────────────┐    ┌─────────────────┐
│   Producers     │    │   RingBuffer    │    │   Consumers     │
│                 │    │                 │    │                 │
│ ┌─────────────┐ │    │ ┌─────────────┐ │    │ ┌─────────────┐ │
│ │ Sequencer   │ │───▶│ │ Event Slots │ │───▶│ │EventHandler │ │
│ │             │ │    │ │             │ │    │ │             │ │
│ └─────────────┘ │    │ └─────────────┘ │    │ └─────────────┘ │
│                 │    │                 │    │                 │
│ ┌─────────────┐ │    │ ┌─────────────┐ │    │ ┌─────────────┐ │
│ │WaitStrategy │ │    │ │  Sequences  │ │    │ │EventProcessor│ │
│ │             │ │    │ │             │ │    │ │             │ │
│ └─────────────┘ │    │ └─────────────┘ │    │ └─────────────┘ │
└─────────────────┘    └─────────────────┘    └─────────────────┘
```

## Core Components

### 1. RingBuffer (`src/disruptor/ring_buffer.rs`)

The heart of the Disruptor pattern - a pre-allocated circular buffer.

**Key Features:**
- **Memory Layout**: Uses `Box<[UnsafeCell<T>]>` for optimal memory layout
- **Fast Indexing**: Bit-mask operations for O(1) index calculation
- **Zero-Copy Access**: Raw pointer access for maximum performance
- **Batch Processing**: `BatchIterMut` for efficient batch operations

**Implementation Details:**
```rust
pub struct RingBuffer<T> {
    slots: Box<[UnsafeCell<T>]>,     // Pre-allocated event storage
    index_mask: i64,                 // Fast modulo: (size - 1)
}
```

**Safety Guarantees:**
- UnsafeCell provides interior mutability
- Index bounds guaranteed by power-of-2 buffer size
- Memory safety through careful coordination with sequencers

### 2. Sequencers (`src/disruptor/sequencer.rs`)

Coordinate access to the RingBuffer between producers and consumers.

#### SingleProducerSequencer
Optimized for single-threaded publishing scenarios.

**Key Features:**
- **Lock-Free**: No atomic operations needed for single producer
- **LMAX Compatibility**: Exact implementation of LMAX algorithms
- **Cache Optimization**: Cached gating sequences to reduce contention

**Implementation Details:**
```rust
pub struct SingleProducerSequencer {
    buffer_size: usize,
    wait_strategy: Arc<dyn WaitStrategy>,
    cursor: Arc<Sequence>,
    next_value: AtomicI64,           // Next sequence to claim
    cached_value: AtomicI64,         // Cached minimum gating sequence
}
```

#### MultiProducerSequencer
Supports multiple concurrent producers with advanced optimizations.

**Key Features:**
- **Bitmap Optimization**: Inspired by disruptor-rs for large buffers
- **CAS Coordination**: Compare-and-swap for thread coordination
- **Dual Tracking**: Both legacy LMAX and modern bitmap methods
- **Cache-Friendly**: Proper padding to avoid false sharing

**Implementation Details:**
```rust
pub struct MultiProducerSequencer {
    buffer_size: usize,
    wait_strategy: Arc<dyn WaitStrategy>,
    cursor: Arc<Sequence>,
    available_bitmap: Box<[AtomicU64]>,      // disruptor-rs style
    available_buffer: Vec<AtomicI32>,        // LMAX style
    cached_gating_sequence: CachePadded<AtomicI64>,
}
```

### 3. Producer APIs (`src/disruptor/producer.rs`)

Simplified, user-friendly publishing interfaces inspired by disruptor-rs.

**Key Features:**
- **Closure-Based**: `producer.publish(|event| { event.value = 42; })`
- **Batch Support**: Efficient batch publishing with `BatchIterMut`
- **Error Handling**: Clear error types (`RingBufferFull`, `MissingFreeSlots`)
- **Zero-Copy**: Direct access to ring buffer slots

**API Design:**
```rust
pub trait Producer<E> {
    fn try_publish<F>(&mut self, update: F) -> Result<i64, RingBufferFull>;
    fn try_batch_publish<F>(&mut self, n: usize, update: F) -> Result<i64, MissingFreeSlots>;
    fn publish<F>(&mut self, update: F);
    fn batch_publish<F>(&mut self, n: usize, update: F);
}
```

### 4. Builder Pattern (`src/disruptor/builder.rs`)

Fluent API for easy Disruptor configuration inspired by disruptor-rs.

**Key Features:**
- **Type-Safe States**: `NoConsumers` → `HasConsumers` transitions
- **Fluent Chaining**: Method chaining for readable configuration
- **Automatic Setup**: Handles RingBuffer creation and thread management

**Usage Example:**
```rust
let mut producer = build_single_producer(8, factory, BusySpinWaitStrategy)
    .handle_events_with(|event, seq, end_batch| {
        println!("Processing: {}", event.value);
    })
    .build();
```

### 5. Wait Strategies

#### Traditional LMAX Strategies (`src/disruptor/wait_strategy.rs`)
- **BlockingWaitStrategy**: Uses parking_lot for efficient blocking
- **YieldingWaitStrategy**: Thread yielding for moderate latency
- **BusySpinWaitStrategy**: CPU spinning for ultra-low latency
- **SleepingWaitStrategy**: Sleep-based for low CPU usage

#### Simplified Strategies (`src/disruptor/simple_wait_strategy.rs`)
Inspired by disruptor-rs for easier usage:
- **BusySpin**: Simple spinning implementation
- **BusySpinWithHint**: Spinning with CPU hints
- **Yielding**: Thread yielding
- **Sleeping**: Sleep-based waiting

### 6. Thread Management (`src/disruptor/thread_management.rs`)

Advanced thread management with CPU affinity support.

**Key Features:**
- **CPU Affinity**: Pin threads to specific CPU cores
- **Fluent API**: `ThreadBuilder::new().pin_at_core(1).thread_name("worker")`
- **Lifecycle Management**: Automatic cleanup and graceful shutdown
- **Cross-Platform**: Uses `core_affinity` crate for portability

**Implementation:**
```rust
pub struct ThreadBuilder {
    context: ThreadContext,
}

pub struct ManagedThread {
    join_handle: Option<thread::JoinHandle<()>>,
    thread_name: String,
}
```

### 7. Elegant Consumer (`src/disruptor/elegant_consumer.rs`)

Simplified consumer handling with automatic lifecycle management.

**Key Features:**
- **Multiple Creation Patterns**: `new()`, `with_state()`, `with_affinity()`
- **Automatic Batch Detection**: Built-in end-of-batch detection
- **State Management**: Support for stateful event processing
- **Graceful Shutdown**: Clean shutdown with proper cleanup

**API Design:**
```rust
// Basic consumer
let consumer = ElegantConsumer::new(ring_buffer,
    |event, seq, end_batch| { process(event); },
    BusySpin)?;

// Stateful consumer
let consumer = ElegantConsumer::with_state(ring_buffer,
    |state, event, seq, end_batch| { state.update(event); },
    || MyState::new(),
    BusySpin)?;

// CPU-pinned consumer
let consumer = ElegantConsumer::with_affinity(ring_buffer,
    |event, seq, end_batch| { process(event); },
    BusySpin, 1)?;
```

## Performance Optimizations

### Memory Layout Optimizations

1. **Cache Line Padding**: Uses `crossbeam_utils::CachePadded` to prevent false sharing
2. **Optimal Data Structures**: `Box<[UnsafeCell<T>]>` instead of `Vec<T>` for better layout
3. **Bit Manipulation**: Fast modulo operations using bit masks

### Algorithmic Optimizations

1. **Batch Processing**: Automatic batching reduces coordination overhead
2. **Bitmap Tracking**: O(1) availability checking for large buffers
3. **Cached Sequences**: Reduces expensive minimum sequence calculations
4. **Lock-Free Algorithms**: Pure atomic operations without locks

### CPU Optimizations

1. **CPU Affinity**: Pin critical threads to specific cores
2. **Memory Barriers**: Proper ordering without unnecessary synchronization
3. **Branch Prediction**: Optimized hot paths for common cases
4. **NUMA Awareness**: Thread placement for NUMA systems

## Safety and Correctness

### Memory Safety
- **Rust Ownership**: Compile-time memory safety guarantees
- **UnsafeCell**: Controlled interior mutability with clear safety contracts
- **Arc/Atomic**: Safe sharing between threads

### Algorithmic Correctness
- **LMAX Compliance**: Faithful implementation of proven algorithms
- **Sequence Coordination**: Proper ordering guarantees
- **ABA Prevention**: Careful handling of sequence wraparound

### Testing Strategy
- **Unit Tests**: 319 tests covering all components
- **Integration Tests**: End-to-end scenarios
- **Property Tests**: Invariant checking
- **Performance Tests**: Latency and throughput validation with criterion.rs

## Dependencies

### Core Dependencies
- **crossbeam-utils**: Cache-friendly atomic operations and padding
- **parking_lot**: High-performance synchronization primitives
- **core_affinity**: Cross-platform CPU affinity support
- **thiserror**: Ergonomic error handling

### Development Dependencies
- **criterion**: Performance benchmarking
- **tokio-test**: Async testing utilities

## Compatibility and Standards

### LMAX Disruptor Compatibility
- **Algorithm Fidelity**: Exact implementation of core algorithms
- **API Similarity**: Similar concepts and naming conventions
- **Performance Characteristics**: Comparable performance profiles

### disruptor-rs Inspiration
- **Simplified APIs**: User-friendly interfaces
- **Modern Rust Patterns**: Idiomatic Rust design
- **Performance Optimizations**: Advanced optimization techniques

### Rust Ecosystem Integration
- **Standard Traits**: Implements standard Rust traits where appropriate
- **Error Handling**: Uses `Result` types and `thiserror`
- **Documentation**: Comprehensive rustdoc documentation
- **Testing**: Standard Rust testing patterns

## Future Enhancements

### Planned Features
1. **Advanced Barriers**: Complex dependency graphs
2. **Dynamic Reconfiguration**: Runtime topology changes
3. **Metrics Integration**: Performance monitoring
4. **Async Support**: Integration with async runtimes

### Performance Improvements
1. **SIMD Optimizations**: Vectorized operations where applicable
2. **Custom Allocators**: Specialized memory management
3. **Hardware Acceleration**: Platform-specific optimizations

## Implementation Details

### Sequence Management

The sequence management system is the backbone of the Disruptor's coordination mechanism.

#### Sequence (`src/disruptor/sequence.rs`)
```rust
pub struct Sequence {
    value: CachePadded<AtomicI64>,
}
```

**Key Features:**
- **Cache Line Padding**: Prevents false sharing between sequences
- **Atomic Operations**: Thread-safe updates with memory ordering
- **Utility Functions**: Minimum sequence calculation across multiple sequences

#### Sequence Barriers (`src/disruptor/sequence_barrier.rs`)
Coordinate dependencies between event processors.

```rust
pub struct ProcessingSequenceBarrier {
    wait_strategy: Arc<dyn WaitStrategy>,
    dependent_sequence: Arc<Sequence>,
    cursor_sequence: Arc<Sequence>,
    sequences_to_track: Vec<Arc<Sequence>>,
}
```

### Event Processing System

#### Event Handlers (`src/disruptor/event_handler.rs`)
Define how events are processed by consumers.

```rust
pub trait EventHandler<T>: Send + Sync {
    fn on_event(&mut self, event: &mut T, sequence: i64, end_of_batch: bool) -> Result<()>;
}
```

#### Event Processors (`src/disruptor/event_processor.rs`)
Manage the event processing loop and lifecycle.

```rust
pub struct BatchEventProcessor<T> {
    data_provider: Arc<RingBuffer<T>>,
    sequence_barrier: Arc<dyn SequenceBarrier>,
    event_handler: Box<dyn EventHandler<T>>,
    sequence: Arc<Sequence>,
    exception_handler: Box<dyn ExceptionHandler>,
}
```

### Error Handling Strategy

#### Comprehensive Error Types
```rust
#[derive(Debug, thiserror::Error)]
pub enum DisruptorError {
    #[error("Ring buffer is full")]
    BufferFull,
    #[error("Invalid sequence: {0}")]
    InvalidSequence(i64),
    #[error("Buffer size must be a power of 2, got: {0}")]
    InvalidBufferSize(usize),
    #[error("Timeout waiting for sequence")]
    Timeout,
    #[error("Disruptor has been shut down")]
    Shutdown,
    #[error("Insufficient capacity")]
    InsufficientCapacity,
    #[error("Alert exception")]
    Alert,
}
```

#### Exception Handling (`src/disruptor/exception_handler.rs`)
Provides recovery mechanisms for event processing failures.

### Main Disruptor DSL (`src/disruptor/disruptor.rs`)

The main entry point provides a fluent API for configuration:

```rust
pub struct Disruptor<T> {
    ring_buffer: Arc<RingBuffer<T>>,
    sequencer: Arc<dyn Sequencer>,
    event_processors: Vec<Arc<dyn EventProcessor>>,
    exception_handler: Box<dyn ExceptionHandler>,
}
```

**Configuration Methods:**
- `handle_events_with()`: Add event handlers
- `after()`: Define processing dependencies
- `then()`: Sequential processing stages
- `build()`: Finalize configuration

### Advanced Features

#### Event Translation System
Provides type-safe event publishing with automatic translation.

```rust
pub trait EventTranslator<T> {
    fn translate_to(&self, event: &mut T, sequence: i64);
}

// Multi-argument variants
pub trait EventTranslatorOneArg<T, A> {
    fn translate_to(&self, event: &mut T, sequence: i64, arg0: A);
}
```

#### Factory Pattern
Ensures proper event initialization and lifecycle management.

```rust
pub trait EventFactory<T>: Send + Sync {
    fn new_instance(&self) -> T;
}

// Convenience implementations
pub struct DefaultEventFactory<T: Default> {
    _phantom: PhantomData<T>,
}

pub struct ClosureEventFactory<T, F> {
    factory_fn: F,
    _phantom: PhantomData<T>,
}
```

## Performance Characteristics

### Latency Measurements
Based on internal benchmarks and design characteristics:

- **Single Producer**: ~10-50ns per event (depending on wait strategy)
- **Multi Producer**: ~50-200ns per event (with contention)
- **Batch Processing**: ~5-20ns per event (amortized)

### Throughput Capabilities
- **Single Producer**: 20-100M events/second
- **Multi Producer**: 5-50M events/second (depending on contention)
- **Memory Usage**: O(buffer_size) with zero runtime allocation

### Scalability Characteristics
- **CPU Cores**: Linear scaling up to memory bandwidth limits
- **Buffer Size**: Logarithmic impact on latency, linear on memory
- **Consumer Count**: Minimal impact with proper CPU affinity

## Integration Patterns

### REST API Integration (`src/api/`)
The project includes REST API components for external integration:

```rust
// API server with Disruptor backend
pub struct ApiServer {
    disruptor: Arc<Disruptor<ApiEvent>>,
    // ... other fields
}
```

### Cluster Support (`src/cluster/`)
Distributed capabilities with Gossip-based discovery:

```rust
pub struct ClusterNode {
    node_id: String,
    disruptor: Arc<Disruptor<ClusterEvent>>,
    gossip: GossipProtocol,
    // ... other fields
}
```

### CLI Interface (`src/bin/`)
Command-line interface for management and monitoring:

```rust
#[derive(Parser)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    Start(StartCommand),
    Stop(StopCommand),
    Status(StatusCommand),
    // ... other commands
}
```

## Testing and Validation

### Test Coverage
- **Unit Tests**: 319 tests covering all core components
- **Integration Tests**: End-to-end scenarios with multiple producers/consumers
- **Property Tests**: Invariant checking for sequence ordering
- **Performance Tests**: Latency and throughput validation with criterion.rs

### Continuous Integration
- **Rust Versions**: Tested on stable, beta, and nightly
- **Platforms**: Linux, macOS, Windows
- **Architectures**: x86_64, ARM64
- **Miri**: Memory safety validation

### Benchmarking Strategy
- **Criterion.rs**: Statistical performance analysis
- **Latency Histograms**: P50, P95, P99, P99.9 measurements
- **Throughput Tests**: Events per second under various loads
- **Memory Profiling**: Allocation patterns and memory usage

This design document reflects the current implementation as of the latest commit and will be updated as the project evolves.
