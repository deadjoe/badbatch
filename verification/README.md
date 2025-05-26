# BadBatch Disruptor Formal Verification

This directory contains TLA+ formal verification models for the BadBatch Disruptor implementation. These models mathematically prove the correctness of our concurrent algorithms and help identify potential race conditions, deadlocks, and other concurrency issues.

## Overview

The verification suite consists of three main TLA+ models that verify different aspects of the BadBatch Disruptor:

- **`BadBatchSPMC.tla`** - Single Producer Multi Consumer verification
- **`BadBatchMPMC.tla`** - Multi Producer Multi Consumer verification
- **`BadBatchRingBuffer.tla`** - Ring buffer data structure verification

All models are based on the proven disruptor-rs TLA+ specifications but adapted for BadBatch's specific implementation details.

## What is TLA+?

TLA+ (Temporal Logic of Actions) is a formal specification language for describing and verifying concurrent and distributed systems. It was created by Leslie Lamport and is widely used in industry for verifying critical systems.

**Key Benefits:**
- **Mathematical Proof**: Provides mathematical guarantees of correctness
- **Exhaustive Testing**: Explores all possible execution paths
- **Race Condition Detection**: Finds subtle concurrency bugs
- **Deadlock Prevention**: Verifies liveness properties
- **Design Validation**: Validates algorithms before implementation

## Verification Properties

### Safety Properties (Nothing Bad Happens)

1. **NoDataRaces**: Ensures no simultaneous read/write access to ring buffer slots
2. **TypeOk**: Verifies all variables maintain correct types throughout execution
3. **ConsistentState**: Ensures system state remains consistent across all operations
4. **MemorySafety**: Verifies bounds checking and memory access patterns

### Liveness Properties (Something Good Eventually Happens)

1. **EventualConsumption**: All published events are eventually consumed by all consumers
2. **ProgressGuarantee**: Producers and consumers make progress under fair scheduling
3. **NoStarvation**: No thread is permanently blocked from making progress
4. **Termination**: System can cleanly terminate when requested

## Model Configurations

### SPMC (Single Producer Multi Consumer)

Recommended configuration for quick verification (< 1 minute):

```
MaxPublished <- 10
Size <- 8
Writers <- { "producer" }
Readers <- { "consumer1", "consumer2" }
NULL <- [ model value ]
```

### MPMC (Multi Producer Multi Consumer)

Recommended configuration for comprehensive verification (< 5 minutes):

```
MaxPublished <- 10
Size <- 8
Writers <- { "producer1", "producer2" }
Readers <- { "consumer1", "consumer2" }
NULL <- [ model value ]
```

### Extended Testing

For thorough verification (may take longer):

```
MaxPublished <- 20
Size <- 16
Writers <- { "p1", "p2", "p3" }
Readers <- { "c1", "c2", "c3" }
NULL <- [ model value ]
```

## Running Verification

### Prerequisites

1. **Download TLA+ Tools**:
   ```bash
   cd verification
   curl -L -o tla2tools.jar https://github.com/tlaplus/tlaplus/releases/latest/download/tla2tools.jar
   ```
2. **Java Runtime**: TLA+ requires Java 8 or later

### Quick Start with Automated Script

We provide an automated verification script for easy testing:

```bash
cd verification
chmod +x verify.sh
./verify.sh quick      # Quick verification (< 2 minutes)
./verify.sh extended   # Extended verification (< 10 minutes)
./verify.sh spmc       # SPMC model only
./verify.sh mpmc       # MPMC model only
./verify.sh ringbuffer # RingBuffer model only
```

### Manual Verification

For manual verification using TLA+ command line:

```bash
cd verification
java -Xmx4g -XX:+UseParallelGC -jar tla2tools.jar -config configs/spmc_config.cfg BadBatchSPMC.tla
java -Xmx4g -XX:+UseParallelGC -jar tla2tools.jar -config configs/mpmc_config.cfg BadBatchMPMC.tla
```

### TLA+ Toolbox GUI (Alternative)

1. **Install TLA+ Toolbox**: Download from [TLA+ website](https://lamport.azurewebsites.net/tla/toolbox.html)
2. **Open Specification**: File → Open Spec → Add New Spec
3. **Create Model**: TLC Model Checker → New Model
4. **Configure**: Set constants and properties as per config files
5. **Run**: Click "Run TLC" button

### Current Verification Status

| Model | Status | States Explored | Issues |
|-------|--------|----------------|---------|
| **BadBatchSPMC** | ✅ **PASSED** | 7,197 states, 2,677 distinct | None - All safety and liveness properties verified |
| **BadBatchMPMC** | ✅ **PASSED** | 161,285 states, 48,197 distinct | None - All safety and liveness properties verified |
| **BadBatchRingBuffer** | ✅ **PASSED** | Included in both verifications | No data races, type safety verified |

### Expected Results

**Successful Verification (SPMC Example):**
```text
TLC finished computing initial states: 1 distinct state generated.
TLC finished: 7197 states generated, 2677 distinct states found, 0 states left on queue.
The depth of the complete state graph search is 61.
No errors found.
```

**Successful Verification (MPMC Example):**
```text
TLC finished computing initial states: 1 distinct state generated.
TLC finished: 161285 states generated, 48197 distinct states found, 0 states left on queue.
The depth of the complete state graph search is 61.
No errors found.
```

**If Errors Found:**

- TLA+ will provide a counterexample trace
- Follow the trace to identify the bug
- Fix the implementation and re-verify

## Model Details

### BadBatchSPMC.tla ✅ **VERIFIED**

Models the single producer, multi consumer scenario based on proven disruptor-rs design:

- **Producer Actions**: `BeginWrite`, `EndWrite` with two-phase protocol
- **Consumer Actions**: `BeginRead`, `EndRead` with proper synchronization
- **State Machine**: Simple `Access`/`Advance` states for clean transitions
- **Verification Results**:
  - ✅ NoDataRaces: No concurrent read/write access
  - ✅ TypeOk: All variables maintain correct types
  - ✅ Liveliness: All events eventually consumed by all consumers
  - ✅ 7,197 states explored, 61 depth levels

### BadBatchMPMC.tla ✅ **VERIFIED**

Models the multi producer, multi consumer scenario with proven correctness:

- **Producer Coordination**: Atomic sequence claiming with CAS-based coordination
- **Availability Buffer**: Even/odd round publication tracking (matches LMAX Disruptor)
- **Verification Results**:
  - ✅ NoDataRaces: No concurrent read/write access to ring buffer slots
  - ✅ TypeOk: All variables maintain correct types throughout execution
  - ✅ Liveliness: All events eventually consumed by all consumers
  - ✅ 161,285 states explored, 48,197 distinct states
- **Status**: Fully verified and matches Rust implementation

### BadBatchRingBuffer.tla

Models the core ring buffer data structure:

- **Memory Layout**: Models cache-padded ring buffer slots
- **Index Calculation**: Verifies modulo arithmetic for ring wrapping
- **Access Tracking**: Tracks concurrent read/write operations
- **Bounds Checking**: Ensures array bounds are never violated

## Integration with Development

### Design Phase

Before implementing new features:

1. **Model the Algorithm**: Create TLA+ specification
2. **Verify Properties**: Ensure safety and liveness
3. **Generate Test Cases**: Use TLA+ traces for testing
4. **Implement Code**: Follow the verified specification

### Bug Investigation

When bugs are found:

1. **Reproduce in Model**: Add the scenario to TLA+ model
2. **Find Counterexample**: Let TLA+ find the problematic execution
3. **Analyze Trace**: Understand the root cause
4. **Fix and Re-verify**: Ensure the fix is correct

### Continuous Verification

Include verification in CI/CD:

```bash
# Example script for automated verification
#!/bin/bash
cd verification
for model in *.tla; do
    echo "Verifying $model..."
    tlc -config ${model%.tla}.cfg $model
    if [ $? -ne 0 ]; then
        echo "Verification failed for $model"
        exit 1
    fi
done
echo "All models verified successfully"
```

## Troubleshooting

### Common Issues

1. **State Space Explosion**: Reduce model parameters if verification takes too long
2. **Memory Issues**: Increase JVM heap size for TLA+ Toolbox
3. **Counterexamples**: Carefully analyze the trace to understand the issue
4. **Model Errors**: Check TLA+ syntax and module dependencies

### Performance Tips

- Start with small configurations
- Use symmetry reduction when possible
- Enable state compression for large models
- Use multiple workers for parallel verification

## Lessons Learned

### Key Insights from Verification Process

1. **Complexity vs Correctness**: Initial models were over-engineered with unnecessary states and variables. The proven disruptor-rs approach uses simple `Access`/`Advance` state machines.

2. **MPMC Challenges**: Multi-producer scenarios require careful modeling of:
   - Atomic sequence claiming (CAS operations)
   - Availability buffer with turn-based publication
   - Proper synchronization between claim/write/publish phases

3. **Value of Formal Verification**: TLA+ successfully detected data races that would be difficult to find through traditional testing, validating the investment in formal methods.

4. **Reference Implementation Importance**: Studying proven implementations (disruptor-rs) provided crucial insights for correct modeling.

### Future Work

- **Add Batch Operations**: Model batch publishing and consumption patterns
- **Performance Properties**: Add models for cache efficiency and memory barriers
- **Wait Strategies**: Model different wait strategy implementations
- **Extended Configurations**: Test with larger buffer sizes and more producers/consumers

## Contributing

When modifying the verification models:

1. **Test Changes**: Always verify models after modifications
2. **Document Updates**: Update this README with any new properties
3. **Maintain Consistency**: Ensure models match implementation
4. **Add Test Cases**: Include edge cases and boundary conditions
5. **Study References**: Compare with disruptor-rs and LMAX implementations

## References

- [TLA+ Homepage](https://lamport.azurewebsites.net/tla/tla.html)
- [TLA+ Video Course](https://lamport.azurewebsites.net/video/videos.html)
- [Practical TLA+](https://www.apress.com/gp/book/9781484238288)
- [LMAX Disruptor Paper](https://lmax-exchange.github.io/disruptor/disruptor.html)
- [disruptor-rs TLA+ Models](https://github.com/nicholassm/disruptor-rs/tree/main/verification)
