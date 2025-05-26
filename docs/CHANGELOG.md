# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- GitHub Actions CI/CD pipeline
- Automated testing and code quality checks
- Security audit automation
- Dependency update automation
- Performance benchmarking
- Code coverage reporting
- Automated documentation deployment

### Changed
- Updated README.md with GitHub Actions badges
- Improved documentation accuracy

### Fixed
- MSRV compatibility issue with `div_ceil` method

## [0.1.0] - 2024-01-XX

### Added
- Initial release of BadBatch Disruptor Engine
- Complete LMAX Disruptor implementation in Rust
- disruptor-rs inspired optimizations
- REST API interface
- CLI interface with comprehensive commands
- Multiple wait strategies (Blocking, Yielding, BusySpin, Sleeping)
- Thread management with CPU affinity support
- Comprehensive test suite (271 tests)
- Property-based testing with proptest
- Performance benchmarks with criterion.rs
- TLA+ formal verification specifications
- Security audit configuration
- Dependency management with cargo-deny

### Core Features
- **Single Producer Sequencer**: Lock-free single producer implementation
- **Multi Producer Sequencer**: Thread-safe multi producer with bitmap optimization
- **Ring Buffer**: Pre-allocated circular buffer with optimal memory layout
- **Event Processors**: Batch event processing with lifecycle management
- **Wait Strategies**: Multiple strategies for different latency/CPU trade-offs
- **Builder Pattern**: Fluent API for easy configuration
- **Producer API**: Simplified publishing interface with batch support
- **Elegant Consumer**: Modern consumer handling with automatic lifecycle management

### API Features
- **REST API**: Complete HTTP interface for disruptor management
- **Authentication**: API key-based authentication and authorization
- **Metrics**: System and disruptor-specific metrics collection
- **Health Checks**: Comprehensive health monitoring
- **Event Publishing**: Single and batch event publishing endpoints
- **Event Querying**: Event retrieval and sequence management

### CLI Features
- **Disruptor Management**: Create, start, stop, pause, resume disruptors
- **Event Publishing**: Single and batch event publishing
- **Event Generation**: High-performance event generation for testing
- **Monitoring**: Real-time system and disruptor metrics
- **Server Management**: Start standalone servers

### Performance
- **High Throughput**: 20-100M events/second (single producer)
- **Low Latency**: 10-50ns per event (depending on wait strategy)
- **Memory Efficient**: Zero runtime allocation after initialization
- **Cache Friendly**: Optimal memory layout and cache line padding

### Documentation
- Comprehensive README.md with usage examples
- Detailed DESIGN.md for developers and architects
- API documentation with rustdoc
- TLA+ specifications for formal verification
- Performance benchmarking results

### Testing
- 149 unit tests covering all core components
- 110 CLI tests for command-line interface functionality
- 12 integration tests for end-to-end scenarios
- 10 documentation tests for API examples
- Property-based testing for invariant checking
- Security audit with cargo-audit
- Dependency management with cargo-deny

[Unreleased]: https://github.com/deadjoe/badbatch/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/deadjoe/badbatch/releases/tag/v0.1.0
