# F.5 Cross-Language Tail-Latency Protocol

Status: protocol v2, owner-amended and frozen before the Java harness is
implemented. The v1 arrival, rate, validity, and coordinated-omission rules are
unchanged.

This protocol defines the minimum evidence required for a matched BadBatch
Rust versus LMAX Java tail-latency comparison. It is a correctness and
comparability contract, not a performance result. macOS counterfactuals may
validate the harnesses, but comparative performance claims require a separately
approved controlled host.

## 1. Arrival and latency model

Both implementations must use the same open-loop fixed schedule:

- The producer follows planned send times and never waits for the consumer to
  complete the preceding event.
- Latency is measured from the planned send time through consumer completion.
- Producer or ring-buffer delay never resets the event timestamp.
- Raw observations are retained; no outlier may be removed.

This planned-time origin is mandatory. Publish-to-completion or
completion-to-next-publish timing hides coordinated omission and is not
comparable evidence.

## 2. Two-phase rate alignment

Rate selection has two explicit phases.

1. Calibrate the maximum sustainable rate of Rust and Java independently on
   the same host and configuration. A calibrated rate is sustainable only if
   its achieved/target ratio is at least 0.95 and every other validity gate
   passes.
2. Set one common reference rate:

   ```text
   common_max = min(max_rust, max_java)
   ```

   Measure both implementations at the same absolute target rates:

   ```text
   50%, 70%, and 90% of common_max
   ```

Each artifact must record both the percentage and the absolute target rate.
Comparing each implementation at a percentage of its own maximum is
prohibited.

When the allocation workload matrix in section 8 is enabled, calibration
covers every implementation and workload configuration:

```text
rust A, java A, rust B-W, java B-W, rust B-4W, java B-4W
```

The one `common_max` is the minimum of all six calibrated maxima. Every A and B
measurement then uses the same absolute 50%, 70%, and 90% rates derived from
that global minimum. This preserves equal offered rates not only between
languages, but also between A and B; otherwise a B-minus-A comparison would
conflate workload mode with rate.

In this matrix, a label such as "90% load" means 90% of the global
`common_max`; it does not mean that every arm is at 90% of its own capacity.
Every calibration and measurement artifact must also record:

```text
own_max
own_utilization = target_rate / own_max
```

The absolute target rate remains the comparison basis. `own_utilization` is
mandatory interpretation context and must not be used to substitute a
different per-arm offered rate.

## 3. Warmup and sample size

Warmup is determined separately for each runtime from steady-state evidence;
the counts or durations need not match. Java is expected to require a much
longer warmup for JIT stabilization. The warmup rule and evidence must be
frozen before measured samples are inspected.

After warmup, each measured load must contain at least 100,000 raw samples.
That leaves at least ten observations in the highest 0.01%; longer formal runs
are preferred.

## 4. Statistics

Both implementations report, in integer nanoseconds:

- p50
- p99
- p99.9
- p99.99
- max

Percentiles use the same integer nearest-rank rule:

```text
rank = ceil(percentile * sample_count)
value = sorted_samples[max(rank, 1) - 1]
```

Mean and minimum may be reported as diagnostics, but they do not replace the
required statistics.

## 5. Required artifacts

Every measured load preserves a raw CSV with exactly these semantic columns:

```text
sequence,planned_ns,completion_ns,latency_ns
```

Rust and Java summary JSON must use the same field names and units for the
arrival model, latency origin, target and achieved rates, validity gates,
percentiles, sample count, configuration, and provenance. The implementation
name and runtime-specific metadata may differ.

Every artifact records a full Git commit and dirty state that are correct
regardless of launch directory. Unknown provenance or a dirty build
hard-invalidates the load and the process exits non-zero, but the raw CSV and
invalid-marked JSON must still be written for diagnosis.

The Java artifact additionally records the exact JVM version, collector, heap
settings, and relevant runtime flags. GC and safepoint logs are preserved with
timestamps that can be aligned to latency samples.

## 6. General validity and pairing

A load is valid only when:

- recorded samples are at least 100,000;
- achieved/target is at least 0.95;
- provenance is known and clean;
- all implementation-specific correctness gates pass; and
- required raw and summary artifacts are complete.

Rust and Java loads form a comparison pair only when they were run on the same
host/configuration at the same absolute target rate and both are valid. If
either side is invalid, discard the pair from comparative analysis while
retaining its artifacts. Do not substitute a run from another host, another
window, or historical results.

## 7. Coordinated-omission counterfactual

Before formal comparison, each implementation must pass a control-versus-pause
counterfactual using the same fixed schedule and absolute target rate. Inject
one known consumer pause during the measured region.

Let:

```text
N = measured sample count
B = ceil(target_rate * pause_duration_seconds)
```

Here `B` means the estimated pause backlog, not allocation arm B in section 8.

The counterfactual is executable only when:

```text
ceil(N / 1000) <= B <= floor(N / 10)
```

The lower bound gives p99.9 enough affected observations to see the pause. The
upper bound makes the pause-created backlog at least an order of magnitude
smaller than the measured population, rather than allowing the injection to
dominate the run.

The injection point must also leave enough measured time to drain the pause
backlog. Conservatively define:

```text
L = target_rate / common_max
minimum_drain_time = pause_duration * L / (1 - L)
```

From the end of the injected pause through the last planned send, the measured
region must retain at least `minimum_drain_time`; a 2x margin is recommended.
For example, a 50 ms pause at the 90% load requires at least 450 ms after the
pause, preferably 900 ms. The injection sequence, planned injection time,
remaining measured time, and computed drain allowance must be recorded.
Normally the pause belongs in the first half of the measured region, never
near its end.

Acceptance requires all three signals together:

1. Injected-run p50 remains within a predeclared equivalence tolerance of the
   control p50. The tolerance must be fixed before the injected result is
   inspected.
2. Injected-run p99.9 is at least 50% of the pause duration and max is at least
   80% of the pause duration, placing both in the pause's time scale.
3. Achieved/target remains near 1.0 under injection, within a predeclared tight
   scheduling tolerance and without a material drop from control. Merely
   passing the general 0.95 validity gate is insufficient.

Max alone never establishes coordinated-omission resistance. The expected
signature is an essentially unchanged median, pause-scale tail and maximum,
and unchanged offered rate.

Run this complete counterfactual independently for A, B-W, and B-4W in both
languages. Each run recomputes the pause-backlog bounds, drain allowance, and
three acceptance signals from its own observations. Passing A does not validate
B: allocation and retention may change service capacity and drain behavior.

## 8. Allocation workload matrix

The formal comparison has one allocation-free arm and two allocation-workload
sub-arms. In every arm, sample recording itself is allocation-free during the
measured region. Apart from the workload allocation and retention described
below, all arrival, rate-alignment, warmup, sample, statistics, artifact,
provenance, validity, pairing, and coordinated-omission rules are identical.

### A. Allocation-free runtime floor

The handler performs only primitive arithmetic and writes into sample arrays
that were fully allocated before measurement. This arm measures the runtime,
JIT, safepoint, scheduling, and harness floor. If the Java GC log shows no
collection in the measured region, this arm provides no evidence for or
against a GC advantage.

### B. Allocation and reclamation workload

The handler allocates exactly one payload per event. Both implementations use
a preallocated retention ring to force the payload to escape and to give it a
fixed logical lifetime:

- Java stores a newly allocated payload object into the next retention slot.
  Overwriting the slot releases the previous reference to the collector.
- Rust stores a newly allocated `Box<Payload>` into the next retention slot.
  Overwriting the slot synchronously drops the previous box.
- The retained payloads are inspected after measurement so an optimizer cannot
  legally eliminate or scalar-replace the allocations.

Run B at two pre-registered retention windows:

```text
W  = buffer_size
4W = 4 * buffer_size
```

For the standard 65,536-slot configuration, these represent one and four ring
rotations of downstream working set. Every result reports the absolute object
count, estimated and observed retained live bytes, and retention duration
`window / target_rate`.

If the Java/Rust gap is stable at W and 4W, it may be summarized as insensitive
over those two tested windows. If the gap changes materially, report it as a
function of the retention window; do not publish one window-independent
number.

### Payload and allocation-byte alignment

The logical payload is fixed at 32 bytes: four 64-bit fields, matching the
existing four-long head-to-head event payload. Payload size is not swept. Every
claim is therefore conditional on this payload size, and reports must state
that larger or smaller payloads were not tested. Payload size is not a neutral
parameter: it scales both allocation rate and the retained live set. Fixing it
keeps the matrix bounded; it does not establish payload-size insensitivity.

Java JFR `allocationSize` includes object header and alignment, while the Rust
counting allocator observes requested `Layout`. Do not assume those values are
comparable from source fields alone. Before formal measurement:

1. measure the Java per-event allocation bytes with JFR;
2. add non-semantic Rust padding so the requested Rust allocation matches the
   measured Java total;
3. freeze the resulting layouts before inspecting tail results; and
4. require measured bytes per event to agree within 5%.

Reports list logical payload bytes, Java runtime overhead, Rust matching
padding, measured bytes per event, allocation rate, and retained live bytes
separately. If the 5% gate fails, the B pair is invalid.

## 9. Interpretation and falsification

Allowed claims are limited to valid paired observations at the same absolute
rate on the same host/configuration.

Arm A is an allocation-free runtime floor. The difference-in-differences
between B and A measures the incremental difference between the two allocation
and reclamation regimes. It is not, by itself, a measurement of GC: it also
contains Java allocation barriers/TLAB/JIT effects and Rust allocator/free
costs.

The following are prohibited:

- extrapolation beyond measured rates;
- cross-host, cross-window, or historical substitution;
- deleting outliers; and
- attributing a tail spike to GC without temporal alignment between the raw
  latency samples and the preserved GC/safepoint logs.

A specific Java tail spike may be attributed to GC only when its raw sample
time overlaps a recorded GC or safepoint pause. An allocation workload without
that temporal alignment supports only a memory-management-regime comparison.

The no-GC advantage hypothesis is falsifiable in arm B: if Java has the same
or better tail latency than Rust at the same absolute offered rate, payload
size, and retention window, the hypothesis is falsified for that measured
configuration. The result must not be generalized to unmeasured hosts, rates,
collectors, heaps, runtime versions, payload sizes, or retention windows.
