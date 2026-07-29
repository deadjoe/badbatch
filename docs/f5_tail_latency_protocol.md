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
   the same host and configuration. Run at least three balanced,
   fresh-process calibrations for every language/arm and select the minimum
   observed maximum as that arm's conservative `own_max`; a single peak
   calibration is insufficient. Every replicate must retain full provenance,
   runtime, profiling, and validity evidence. The configured calibration
   duration must not be shorter than the longest planned measured load.
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

Each measured load records `measurement_epoch_unix_ns`, mapping relative raw
sample timestamps to wall-clock Unix nanoseconds, plus
`clock_anchor_uncertainty_ns` from bracketing the wall-clock read with two
monotonic reads. GC/safepoint attribution is invalid when the relevant pause
cannot be aligned after accounting for that uncertainty.

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
L = target_rate / common_max
B = ceil(target_rate * observed_pause_duration_seconds)
A = ceil(B / (1 - L))
```

Here `B` is only the direct backlog accumulated while the consumer is paused;
it is not allocation arm B in section 8. `A` is the total affected population,
including events that arrive while that backlog drains. Omitting the
`1 / (1 - L)` factor is invalid and can undercount the 90%-load population by
10x.

The counterfactual is executable only when:

```text
ceil(N / 1000) <= A <= floor(N / 10)
```

The lower bound places p99.9 inside the conservatively estimated affected
population, but it does not by itself guarantee the separate visibility
threshold: an arm can drain its consumer backlog faster than the end-to-end
`common_max` service-rate bound. The automatic selector therefore chooses the
largest precision-qualified request whose preflight worst-observed affected
count is at most `floor(N / 20)`. This keeps a 2x margin to the final hard
upper bound while providing depth for the p99.9 signature.
The upper bound makes the complete drain-amplified population at least an
order of magnitude smaller than the measured population, rather than allowing
the injection to move the body of the distribution.

The injection point must also leave enough measured time to drain the pause
backlog. Conservatively define:

```text
minimum_drain_time = pause_duration * L / (1 - L)
```

Before calibration, automatically characterize each target host's actual
pause-delivery precision through the same Rust and Java allocation-free handler
paths and the exact formal runtime profile, including CPU placement and Java
heap, collector, JFR, and GC/safepoint logging. Try
`10, 20, 50, 100, 200, 500, 1000, 2000, 5000` microseconds in ascending order,
using five balanced fresh processes per language and candidate. The first
candidate is precision-qualified only when both languages have
`min(observed/requested) >= 1`, `max(observed/requested) <= 1.75`, and
`full range(observed/requested) / median <= 10%`. Stop at the first joint pass.
If none passes, fail before calibration with the complete diagnostics; never
substitute a host-independent timing constant.

Choose the requested pause independently for every load after calibration, at
microsecond resolution, by intersecting the predeclared `A` bounds with that
host's precision-qualified minimum; a single duration must not be reused
across 50%, 70%, and 90%. The selector must also reserve the preflight
overshoot ceiling: a requested duration is admissible only when the affected
population at `1.75 × requested` is no larger than `floor(N / 20)`, rather
than merely fitting under the final `floor(N / 10)` hard gate. This preserves
a second 2x margin for delivery variation on the actual run. If the
intersection is empty, fail with the minimum measured sample count needed to
make it non-empty (`20 ×` the worst-observed affected count at the
precision-qualified minimum). Record both requested and observed pause
nanoseconds.
Recompute `B`, `A`, and drain time from the observed duration, and invalidate
an oversleep that violates either affected-population bound. From the end of
the observed pause through the last planned send, the measured region must
retain at least `minimum_drain_time`; a 2x margin is recommended. The
injection sequence, planned injection time, remaining measured time, and
computed drain allowance must be recorded. Normally the pause belongs in the
first half of the measured region, never near its end.

Acceptance requires all three signals together:

1. Run at least three independent, fresh-process controls **and** at least
   three independent, fresh-process injected runs for every
   language/arm/load combination. Record each side's p50 values, median,
   minimum, maximum, and full range. Before using either dispersion as a
   tolerance, independently require
   `side full range / abs(side median) <= 5%`. If either side fails that
   stability prerequisite, label p50 equivalence **inconclusive** and retain
   its GC/safepoint logs; never let a control- or injected-side spike widen the
   band. When both sides are stable, compare their medians with the
   predeclared tolerance rule
   `max(5% relative difference, control p50 full range in ns,
   injected p50 full range in ns)`. The formula and both replicate counts must
   be frozen before any measurement runs. There is no unmeasured fixed
   nanosecond floor.
2. Injected-run p99.9 is at least 50% of the pause duration and max is at least
   80% of the pause duration, placing both in the pause's time scale.
3. Achieved/target remains near 1.0 under injection, within a predeclared tight
   scheduling tolerance and without a material drop from the median control.
   Every control and injected replicate must independently pass this tight
   rate gate, and the material-drop comparison uses the two side medians.
   Merely passing the general 0.95 validity gate is insufficient.

Max alone never establishes coordinated-omission resistance. The expected
signature is an essentially unchanged median, pause-scale tail and maximum,
and unchanged offered rate.

The report must retain the signed
`injected median p50 - control median p50` delta at every load, both p50
vectors and dispersions, and both achieved/target vectors and medians. A
monotonic or load-correlated signed pattern remains a reported residual
observation even when it lies inside the empirical equivalence band; it must
not be relabeled as timer noise.

The stability prerequisite measures statistical usability, not implementation
quality. A runtime with larger JIT, safepoint, or scheduling dispersion can
therefore produce more inconclusive cells even in the allocation-free arm;
that is not evidence that the runtime is slower. Conversely, if a control's
median latency itself reaches the sustained-delay scale and its achieved rate
drops, label the cell as unable to maintain that offered load in the measured
window (continuous backlog/overload), not as a tail-latency comparison.

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

The first A/B harness extension must also preserve a separate, self-contained
A-equivalence evidence bundle against the immediately preceding
allocation-free harness. It contains at least five adjacent, alternating
fresh-process pairs at identical configuration, raw samples, clean build-time
revisions for both binaries, zero A-arm workload counters, and a
machine-readable report of achieved/target plus p50/p99/p99.9. This is a
measurement-apparatus regression gate, not portable performance evidence, and
must be indexed alongside the final matrix rather than existing only in chat.

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

The allocation-free and allocating handlers are distinct monomorphized code
paths. Their B-minus-A difference may therefore also contain machine-code
layout and inlining effects. That component is not separately identifiable and
does not cancel merely because the report uses a cross-language
difference-in-differences. Allocation or reclamation attribution remains
subject to the boundaries below.

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
