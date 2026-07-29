package com.lmax.disruptor.headtohead;

import com.lmax.disruptor.BatchEventProcessor;
import com.lmax.disruptor.BatchEventProcessorBuilder;
import com.lmax.disruptor.BusySpinWaitStrategy;
import com.lmax.disruptor.EventHandler;
import com.lmax.disruptor.RingBuffer;
import com.lmax.disruptor.SequenceBarrier;
import com.lmax.disruptor.WaitStrategy;
import com.lmax.disruptor.YieldingWaitStrategy;

import java.io.BufferedWriter;
import java.lang.management.ManagementFactory;
import java.math.BigInteger;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.time.Duration;
import java.time.Instant;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;
import java.util.Locale;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicLong;
import java.util.concurrent.atomic.AtomicReference;
import java.util.concurrent.locks.LockSupport;

/**
 * Standalone open-loop tail-latency harness for LMAX Disruptor.
 *
 * <p>The contract is frozen in {@code docs/f5_tail_latency_protocol.md}.
 * Latency begins at the planned send time, not the actual publish time.
 */
public final class TailLatency
{
    private static final long NANOS_PER_SECOND = 1_000_000_000L;
    private static final long MAX_SCHEDULABLE_RATE = Long.MAX_VALUE / NANOS_PER_SECOND;
    private static final long DEFAULT_EVENTS_TOTAL = 2_000_000L;
    private static final long DEFAULT_WARMUP_EVENTS = 1_000_000L;
    private static final int DEFAULT_BUFFER_SIZE = 65_536;
    private static final long DEFAULT_CALIBRATION_EVENTS = 100_000_000L;
    private static final long DEFAULT_CALIBRATION_DURATION_MS = 2_000L;
    private static final long[] DEFAULT_LOAD_LEVELS = {50L, 70L, 90L};
    private static final long DEFAULT_TIMEOUT_SECONDS = 300L;
    private static final double VALID_RUN_THRESHOLD = 0.95;
    private static final long MIN_RECORDED_SAMPLES = 100_000L;
    private static final long QUICK_EVENTS_TOTAL = 110_000L;
    private static final long QUICK_WARMUP_EVENTS = 10_000L;
    private static final long QUICK_CALIBRATION_EVENTS = 1_000_000L;
    private static final int LOGICAL_PAYLOAD_BYTES = 32;
    private static final long DEFAULT_INJECT_AT_MEASURED_PCT = 25L;

    private TailLatency()
    {
    }

    public static void main(final String[] args) throws Exception
    {
        final Config config;
        try
        {
            config = Config.parse(args);
        }
        catch (final IllegalArgumentException error)
        {
            System.err.println("error: " + error.getMessage());
            printHelp();
            System.exit(2);
            return;
        }

        final Provenance provenance = Provenance.current();
        if (!provenance.valid())
        {
            System.err.println(
                    "warning: invalid build provenance; results will be hard-invalidated");
        }

        if (config.calibrateOnly)
        {
            final double ownMax = calibrate(config);
            final boolean artifactValid = provenance.valid()
                    && config.allocationObservationValid();
            final String json = Json.emit(
                    config, null, ownMax, List.of(), provenance, artifactValid);
            writeJson(config.outputPath, json);
            System.out.print(json);
            if (!artifactValid)
            {
                System.err.println("invalid calibration: provenance or allocation observation");
                System.exit(3);
            }
            return;
        }

        final double commonMax;
        if (config.maxRate != null)
        {
            commonMax = config.maxRate.doubleValue();
        }
        else if (config.rate != null)
        {
            commonMax = config.rate.doubleValue();
        }
        else
        {
            commonMax = calibrate(config);
        }
        final double ownMax = config.ownMax == null
                ? commonMax
                : config.ownMax.doubleValue();
        final List<Target> targets = config.targets(commonMax);
        if (targets.isEmpty())
        {
            throw new IllegalArgumentException("no positive target rates computed");
        }

        final List<LoadResult> loads = new ArrayList<>(targets.size());
        boolean artifactValid = provenance.valid();
        for (final Target target : targets)
        {
            final LoadResult load = runLoad(
                    config, target, commonMax, ownMax, provenance.valid());
            loads.add(load);
            artifactValid &= load.validRun;
            final Path samplesPath = samplesPathForLoad(
                    Path.of(config.samplesOutput), target.loadPct);
            writeSamples(samplesPath, load.samples);
        }

        final String json = Json.emit(
                config, commonMax, ownMax, loads, provenance, artifactValid);
        writeJson(config.outputPath, json);
        System.out.print(json);
        if (!artifactValid)
        {
            System.err.println(
                    "invalid run: provenance, rate, workload, or pause validation failed");
            System.exit(3);
        }
    }

    private static double calibrate(final Config config) throws Exception
    {
        return switch (config.handlerMode)
        {
            case ALLOCATION_FREE -> calibrateWithHandler(
                    config, new AllocationFreeCalibrationHandler(config.bufferSize));
            case ALLOCATING -> calibrateWithHandler(
                    config,
                    new AllocatingCalibrationHandler(
                            config.retentionWindow, config.bufferSize));
        };
    }

    private static double calibrateWithHandler(
            final Config config,
            final CalibrationHandler handler) throws Exception
    {
        final AffinityTracker affinity = new AffinityTracker(config);
        affinity.pinCurrent(0, "publisher");
        affinity.verify("calibration publisher");
        handler.configure(affinity, 1);

        final RingBuffer<TailEvent> ring = RingBuffer.createSingleProducer(
                TailEvent::new, config.bufferSize, waitStrategy(config.waitKind));
        final SequenceBarrier barrier = ring.newBarrier();
        final BatchEventProcessor<TailEvent> processor =
                new BatchEventProcessorBuilder().build(ring, barrier, handler);
        ring.addGatingSequences(processor.getSequence());
        final Thread consumer = startProcessor(processor, "tail-calibration-consumer");
        await(handler.ready, "calibration consumer ready");
        affinity.verify("calibration consumer");

        final long warmupEvents = config.calibrationWarmupEvents();
        final long calibrationScheduleRate = Math.min(
                MAX_SCHEDULABLE_RATE,
                ceilMultiplyDivide(
                        config.calibrationEvents,
                        NANOS_PER_SECOND,
                        config.calibrationDuration.toNanos()));
        final long calibrationEpoch = System.nanoTime();
        long timingChecksum = publishCalibrationRange(
                ring,
                0L,
                warmupEvents,
                calibrationScheduleRate,
                calibrationEpoch);
        waitForProcessed(handler.processed, warmupEvents, "calibration warmup completion");

        final long started = System.nanoTime();
        long published = 0L;
        while (published < config.calibrationEvents
                && System.nanoTime() - started < config.calibrationDuration.toNanos())
        {
            final long sequence = warmupEvents + published;
            final long claimed = ring.next();
            final TailEvent event = ring.get(claimed);
            event.sequence = sequence;
            event.plannedNs = scheduledNs(sequence, calibrationScheduleRate);
            ring.publish(claimed);
            timingChecksum ^= System.nanoTime();
            published++;
        }
        waitForProcessed(
                handler.processed,
                warmupEvents + published,
                "calibration completion");
        final long elapsed = System.nanoTime() - started;
        haltAndJoin(processor, consumer);
        handler.validateCalibration();
        if (timingChecksum == Long.MIN_VALUE)
        {
            throw new IllegalStateException("unreachable calibration timing checksum");
        }
        if (elapsed <= 0L)
        {
            throw new IllegalStateException("calibration duration was zero");
        }
        config.affinityVerifiedAll = affinity.verifiedAll();
        return published * (double) NANOS_PER_SECOND / elapsed;
    }

    private static LoadResult runLoad(
            final Config config,
            final Target target,
            final double commonMax,
            final double ownMax,
            final boolean provenanceValid) throws Exception
    {
        final AffinityTracker affinity = new AffinityTracker(config);
        affinity.pinCurrent(0, "publisher");
        affinity.verify("measurement publisher");

        final MeasurementState state = new MeasurementState(config, target);
        final MeasurementHandler handler = switch (config.handlerMode)
        {
            case ALLOCATION_FREE -> new AllocationFreeHandler(state);
            case ALLOCATING -> new AllocatingHandler(state, config.retentionWindow);
        };
        handler.configure(affinity, 1);

        final RingBuffer<TailEvent> ring = RingBuffer.createSingleProducer(
                TailEvent::new, config.bufferSize, waitStrategy(config.waitKind));
        final SequenceBarrier barrier = ring.newBarrier();
        final BatchEventProcessor<TailEvent> processor =
                new BatchEventProcessorBuilder().build(ring, barrier, handler);
        ring.addGatingSequences(processor.getSequence());
        final Thread consumer = startProcessor(processor, "tail-measurement-consumer");
        await(state.ready, "measurement consumer ready");
        affinity.verify("measurement consumer");

        final ClockAnchor anchor = ClockAnchor.capture();
        final long epoch = System.nanoTime();
        state.epochNanos = epoch;
        final long epochWallUnixNs = anchor.projectUnixNs(epoch);
        long firstSendNs = -1L;
        long lastSendNs = 0L;

        for (long sequence = 0L; sequence < config.eventsTotal; sequence++)
        {
            final long plannedNs = scheduledNs(sequence, target.targetRate);
            while (System.nanoTime() - epoch < plannedNs)
            {
                Thread.onSpinWait();
            }
            final long claimed = ring.next();
            final TailEvent event = ring.get(claimed);
            event.sequence = sequence;
            event.plannedNs = plannedNs;
            ring.publish(claimed);
            final long sentNs = System.nanoTime() - epoch;
            if (firstSendNs < 0L)
            {
                firstSendNs = sentNs;
            }
            lastSendNs = sentNs;
        }

        if (!state.done.await(DEFAULT_TIMEOUT_SECONDS, TimeUnit.SECONDS))
        {
            processor.halt();
            consumer.join(TimeUnit.SECONDS.toMillis(5L));
            throw new IllegalStateException("timeout waiting for measurement completion");
        }
        haltAndJoin(processor, consumer);
        config.affinityVerifiedAll = affinity.verifiedAll();

        final long sentEvents = Math.max(1L, config.eventsTotal - 1L);
        final long sendElapsedNs = Math.max(0L, lastSendNs - Math.max(0L, firstSendNs));
        final double actualRate = sendElapsedNs == 0L
                ? 0.0
                : sentEvents * (double) NANOS_PER_SECOND / sendElapsedNs;
        final boolean rateValid = actualRate >= target.targetRate * VALID_RUN_THRESHOLD;
        final LatencyStats stats = LatencyStats.from(state.latencyNs);
        final WorkloadStats workload = handler.workloadStats(config);
        final PauseCheck pause = target.injectSleepNs == 0L
                ? null
                : PauseCheck.from(
                        target.targetRate,
                        commonMax,
                        stats,
                        state,
                        scheduledNs(config.eventsTotal - 1L, target.targetRate));
        final boolean workloadValid = workload.valid;
        final boolean validRun = provenanceValid
                && rateValid
                && workloadValid
                && (pause == null || pause.valid());
        return new LoadResult(
                target.loadPct,
                target.targetRate,
                ownMax,
                target.targetRate / ownMax,
                actualRate,
                rateValid,
                workloadValid,
                validRun,
                epochWallUnixNs,
                anchor.uncertaintyNs,
                pause,
                state.samples(),
                stats,
                workload);
    }

    private static long publishCalibrationRange(
            final RingBuffer<TailEvent> ring,
            final long start,
            final long count,
            final long scheduleRate,
            final long epoch)
    {
        long timingChecksum = 0L;
        for (long offset = 0L; offset < count; offset++)
        {
            final long sequence = start + offset;
            final long plannedNs = scheduledNs(sequence, scheduleRate);
            timingChecksum ^= System.nanoTime() - epoch;
            final long claimed = ring.next();
            final TailEvent event = ring.get(claimed);
            event.sequence = sequence;
            event.plannedNs = plannedNs;
            ring.publish(claimed);
            timingChecksum ^= System.nanoTime() - epoch;
        }
        return timingChecksum;
    }

    private static Thread startProcessor(
            final BatchEventProcessor<TailEvent> processor,
            final String name)
    {
        final Thread thread = new Thread(processor, name);
        thread.setDaemon(true);
        thread.start();
        return thread;
    }

    private static void haltAndJoin(
            final BatchEventProcessor<TailEvent> processor,
            final Thread thread) throws InterruptedException
    {
        processor.halt();
        thread.join(TimeUnit.SECONDS.toMillis(5L));
        if (thread.isAlive())
        {
            throw new IllegalStateException("consumer thread did not terminate");
        }
    }

    private static void await(final CountDownLatch latch, final String label)
            throws InterruptedException
    {
        if (!latch.await(DEFAULT_TIMEOUT_SECONDS, TimeUnit.SECONDS))
        {
            throw new IllegalStateException("timeout waiting for " + label);
        }
    }

    private static void waitForProcessed(
            final AtomicLong processed,
            final long expected,
            final String label)
    {
        final long deadline = System.nanoTime()
                + TimeUnit.SECONDS.toNanos(DEFAULT_TIMEOUT_SECONDS);
        while (processed.get() < expected)
        {
            if (System.nanoTime() >= deadline)
            {
                throw new IllegalStateException(
                        "timeout waiting for " + label + ": got " + processed.get());
            }
            Thread.onSpinWait();
        }
    }

    private static WaitStrategy waitStrategy(final WaitKind kind)
    {
        return switch (kind)
        {
            case BUSY_SPIN -> new BusySpinWaitStrategy();
            case YIELDING -> new YieldingWaitStrategy();
        };
    }

    private static long scheduledNs(final long sequence, final long rate)
    {
        final long seconds = sequence / rate;
        final long remainder = sequence % rate;
        return Math.addExact(
                Math.multiplyExact(seconds, NANOS_PER_SECOND),
                Math.multiplyExact(remainder, NANOS_PER_SECOND) / rate);
    }

    private static long ceilMultiplyDivide(
            final long left,
            final long right,
            final long denominator)
    {
        if (left < 0L || right < 0L || denominator <= 0L)
        {
            throw new IllegalArgumentException("ceilMultiplyDivide requires non-negative values");
        }
        final BigInteger numerator = BigInteger.valueOf(left).multiply(BigInteger.valueOf(right));
        final BigInteger divisor = BigInteger.valueOf(denominator);
        final BigInteger value = numerator
                .add(divisor)
                .subtract(BigInteger.ONE)
                .divide(divisor);
        return value.compareTo(BigInteger.valueOf(Long.MAX_VALUE)) > 0
                ? Long.MAX_VALUE
                : value.longValueExact();
    }

    private static long ceilTripleProductDivide(
            final long first,
            final long second,
            final long third,
            final long denominator)
    {
        if (first < 0L || second < 0L || third < 0L || denominator <= 0L)
        {
            throw new IllegalArgumentException(
                    "ceilTripleProductDivide requires non-negative values");
        }
        final BigInteger numerator = BigInteger.valueOf(first)
                .multiply(BigInteger.valueOf(second))
                .multiply(BigInteger.valueOf(third));
        final BigInteger divisor = BigInteger.valueOf(denominator);
        final BigInteger value = numerator
                .add(divisor)
                .subtract(BigInteger.ONE)
                .divide(divisor);
        return value.compareTo(BigInteger.valueOf(Long.MAX_VALUE)) > 0
                ? Long.MAX_VALUE
                : value.longValueExact();
    }

    private static Path samplesPathForLoad(final Path base, final long loadPct)
    {
        if (loadPct == 0L)
        {
            return base;
        }
        final String name = base.getFileName().toString();
        final int dot = name.lastIndexOf('.');
        final String stem = dot < 0 ? name : name.substring(0, dot);
        final String extension = dot < 0 ? "csv" : name.substring(dot + 1);
        return base.resolveSibling(stem + "-" + loadPct + "." + extension);
    }

    private static void writeSamples(final Path path, final Samples samples) throws Exception
    {
        if (path.getParent() != null)
        {
            Files.createDirectories(path.getParent());
        }
        try (BufferedWriter writer = Files.newBufferedWriter(path, StandardCharsets.UTF_8))
        {
            writer.write("sequence,planned_ns,completion_ns,latency_ns\n");
            for (int index = 0; index < samples.latencyNs.length; index++)
            {
                writer.write(Long.toString(samples.firstSequence + index));
                writer.write(',');
                writer.write(Long.toString(samples.plannedNs[index]));
                writer.write(',');
                writer.write(Long.toString(samples.completionNs[index]));
                writer.write(',');
                writer.write(Long.toString(samples.latencyNs[index]));
                writer.write('\n');
            }
        }
    }

    private static void writeJson(final String outputPath, final String json) throws Exception
    {
        if (outputPath == null)
        {
            return;
        }
        final Path path = Path.of(outputPath);
        if (path.getParent() != null)
        {
            Files.createDirectories(path.getParent());
        }
        Files.writeString(path, json, StandardCharsets.UTF_8);
    }

    private static void printHelp()
    {
        System.out.println("""
                Usage: TailLatency [options]

                  --handler-mode <allocation-free|allocating>
                  --retention-window <N>
                  --allocation-bytes-per-event <N>
                  --allocation-measurement-source <token>
                  --wait-strategy <yielding|busy-spin>
                  --buffer-size <N>
                  --events-total <N>
                  --warmup-events <N>
                  --rate <events/s>
                  --load-levels <pct,pct,...>
                  --max-rate <events/s>
                  --own-max <events/s>
                  --calibrate-only
                  --calibration-events <N>
                  --calibration-duration-ms <N>
                  --cpu-list <N,N,...>
                  --inject-sleep-us-by-load <U,U,...>
                  --inject-at-measured-pct <1..99>
                  --gc-log <path>
                  --jfr-file <path>
                  --output <path.json>
                  --samples-output <path.csv>
                  --quick
                """);
    }

    private enum HandlerMode
    {
        ALLOCATION_FREE("allocation-free"),
        ALLOCATING("allocating");

        final String text;

        HandlerMode(final String text)
        {
            this.text = text;
        }

        static HandlerMode parse(final String value)
        {
            return switch (value)
            {
                case "allocation-free" -> ALLOCATION_FREE;
                case "allocating" -> ALLOCATING;
                default -> throw new IllegalArgumentException(
                        "unsupported handler-mode: " + value);
            };
        }
    }

    private enum WaitKind
    {
        BUSY_SPIN("busy-spin"),
        YIELDING("yielding");

        final String text;

        WaitKind(final String text)
        {
            this.text = text;
        }

        static WaitKind parse(final String value)
        {
            return switch (value)
            {
                case "busy-spin" -> BUSY_SPIN;
                case "yielding" -> YIELDING;
                default -> throw new IllegalArgumentException(
                        "unsupported wait-strategy: " + value);
            };
        }
    }

    private static final class Config
    {
        HandlerMode handlerMode = HandlerMode.ALLOCATION_FREE;
        Integer retentionWindow;
        Long allocationBytesPerEvent;
        String allocationMeasurementSource = "unverified";
        WaitKind waitKind = WaitKind.BUSY_SPIN;
        int bufferSize = DEFAULT_BUFFER_SIZE;
        long eventsTotal = DEFAULT_EVENTS_TOTAL;
        long warmupEvents = DEFAULT_WARMUP_EVENTS;
        Long rate;
        long[] loadLevels = DEFAULT_LOAD_LEVELS.clone();
        long calibrationEvents = DEFAULT_CALIBRATION_EVENTS;
        Duration calibrationDuration = Duration.ofMillis(DEFAULT_CALIBRATION_DURATION_MS);
        boolean calibrateOnly;
        Long maxRate;
        Long ownMax;
        List<Integer> cpuList = List.of();
        boolean affinityVerifiedAll;
        String outputPath;
        String samplesOutput;
        long[] injectSleepNsByLoad = new long[0];
        long injectAtMeasuredPct = DEFAULT_INJECT_AT_MEASURED_PCT;
        String gcLog;
        String jfrFile;
        boolean quick;

        static Config parse(final String[] args)
        {
            final Config config = new Config();
            for (int index = 0; index < args.length; index++)
            {
                final String argument = args[index];
                switch (argument)
                {
                    case "--handler-mode" ->
                            config.handlerMode = HandlerMode.parse(value(args, ++index, argument));
                    case "--retention-window" ->
                            config.retentionWindow = positiveInt(value(args, ++index, argument), argument);
                    case "--allocation-bytes-per-event" ->
                            config.allocationBytesPerEvent =
                                    positiveLong(value(args, ++index, argument), argument);
                    case "--allocation-measurement-source" ->
                            config.allocationMeasurementSource =
                                    token(value(args, ++index, argument), argument);
                    case "--wait-strategy" ->
                            config.waitKind = WaitKind.parse(value(args, ++index, argument));
                    case "--buffer-size" ->
                            config.bufferSize = positiveInt(value(args, ++index, argument), argument);
                    case "--events-total" ->
                            config.eventsTotal = positiveLong(value(args, ++index, argument), argument);
                    case "--warmup-events" ->
                            config.warmupEvents = nonNegativeLong(
                                    value(args, ++index, argument), argument);
                    case "--rate" ->
                            config.rate = positiveLong(value(args, ++index, argument), argument);
                    case "--load-levels" ->
                            config.loadLevels = parseLoadLevels(value(args, ++index, argument));
                    case "--calibration-events" ->
                            config.calibrationEvents =
                                    positiveLong(value(args, ++index, argument), argument);
                    case "--calibration-duration-ms" ->
                            config.calibrationDuration = Duration.ofMillis(
                                    positiveLong(value(args, ++index, argument), argument));
                    case "--calibrate-only" -> config.calibrateOnly = true;
                    case "--max-rate" ->
                            config.maxRate = positiveLong(value(args, ++index, argument), argument);
                    case "--own-max" ->
                            config.ownMax = positiveLong(value(args, ++index, argument), argument);
                    case "--cpu-list" ->
                            config.cpuList = parseCpuList(value(args, ++index, argument));
                    case "--output" -> config.outputPath = value(args, ++index, argument);
                    case "--samples-output" ->
                            config.samplesOutput = value(args, ++index, argument);
                    case "--inject-sleep-us-by-load" ->
                            config.injectSleepNsByLoad = parseDurationUsList(
                                    value(args, ++index, argument));
                    case "--inject-at-measured-pct" ->
                            config.injectAtMeasuredPct =
                                    positiveLong(value(args, ++index, argument), argument);
                    case "--gc-log" -> config.gcLog = value(args, ++index, argument);
                    case "--jfr-file" -> config.jfrFile = value(args, ++index, argument);
                    case "--quick" -> config.quick = true;
                    case "--help", "-h" ->
                    {
                        printHelp();
                        System.exit(0);
                    }
                    default -> throw new IllegalArgumentException(
                            "unknown argument: " + argument);
                }
            }
            config.applyQuick();
            config.validate();
            return config;
        }

        private void applyQuick()
        {
            if (!quick)
            {
                return;
            }
            eventsTotal = Math.min(eventsTotal, QUICK_EVENTS_TOTAL);
            warmupEvents = Math.min(warmupEvents, QUICK_WARMUP_EVENTS);
            calibrationEvents = Math.min(calibrationEvents, QUICK_CALIBRATION_EVENTS);
        }

        private void validate()
        {
            if (Integer.bitCount(bufferSize) != 1)
            {
                throw new IllegalArgumentException("buffer-size must be a power of two");
            }
            if (injectAtMeasuredPct < 1L || injectAtMeasuredPct > 99L)
            {
                throw new IllegalArgumentException(
                        "inject-at-measured-pct must be in 1..=99");
            }
            if (!cpuList.isEmpty() && cpuList.size() < 2)
            {
                throw new IllegalArgumentException("cpu-list requires publisher and consumer");
            }
            if (handlerMode == HandlerMode.ALLOCATION_FREE)
            {
                if (retentionWindow != null)
                {
                    throw new IllegalArgumentException(
                            "retention-window is valid only for allocating mode");
                }
                if (allocationBytesPerEvent != null)
                {
                    throw new IllegalArgumentException(
                            "allocation bytes are valid only for allocating mode");
                }
            }
            else
            {
                if (retentionWindow == null)
                {
                    throw new IllegalArgumentException(
                            "allocating mode requires retention-window");
                }
                if (!calibrateOnly
                        && eventsTotal - warmupEvents < retentionWindow.longValue())
                {
                    throw new IllegalArgumentException(
                            "measured events must reach the full retention-window");
                }
            }
            if (calibrateOnly)
            {
                if (rate != null || maxRate != null || ownMax != null
                        || injectSleepNsByLoad.length != 0 || samplesOutput != null)
                {
                    throw new IllegalArgumentException(
                            "calibrate-only rejects measurement-only flags");
                }
            }
            else
            {
                if (warmupEvents >= eventsTotal)
                {
                    throw new IllegalArgumentException(
                            "warmup-events must be less than events-total");
                }
                final long measured = eventsTotal - warmupEvents;
                if (measured < MIN_RECORDED_SAMPLES)
                {
                    throw new IllegalArgumentException(
                            "at least 100000 measured samples are required");
                }
                if (measured > Integer.MAX_VALUE)
                {
                    throw new IllegalArgumentException(
                            "measured samples must fit a Java array index");
                }
                if (samplesOutput == null)
                {
                    throw new IllegalArgumentException("--samples-output is required");
                }
            }
            for (final long level : loadLevels)
            {
                if (level < 1L || level > 100L)
                {
                    throw new IllegalArgumentException(
                            "load levels must be in 1..=100");
                }
            }
            final int expectedInjectionDurations = rate == null ? loadLevels.length : 1;
            if (injectSleepNsByLoad.length != 0
                    && injectSleepNsByLoad.length != expectedInjectionDurations)
            {
                throw new IllegalArgumentException(
                        "inject-sleep-us-by-load must contain "
                                + expectedInjectionDurations + " values");
            }
            validateSchedulableRate(rate, "--rate");
            validateSchedulableRate(maxRate, "--max-rate");
            validateSchedulableRate(ownMax, "--own-max");
            Math.addExact(calibrationWarmupEvents(), calibrationEvents);
        }

        long calibrationWarmupEvents()
        {
            final long retention = retentionWindow == null ? 0L : retentionWindow.longValue();
            return Math.max(warmupEvents, retention);
        }

        boolean allocationObservationValid()
        {
            return handlerMode == HandlerMode.ALLOCATION_FREE
                    || (allocationBytesPerEvent != null
                    && !"unverified".equals(allocationMeasurementSource));
        }

        List<Target> targets(final double commonMax)
        {
            if (rate != null)
            {
                return List.of(new Target(
                        0L,
                        rate,
                        injectSleepNsByLoad.length == 0 ? 0L : injectSleepNsByLoad[0]));
            }
            final List<Target> targets = new ArrayList<>(loadLevels.length);
            for (int index = 0; index < loadLevels.length; index++)
            {
                final long load = loadLevels[index];
                final long target = Math.round(commonMax * load / 100.0);
                if (target > 0L)
                {
                    targets.add(new Target(
                            load,
                            target,
                            injectSleepNsByLoad.length == 0
                                    ? 0L
                                    : injectSleepNsByLoad[index]));
                }
            }
            return targets;
        }

        private static String value(
                final String[] args,
                final int index,
                final String argument)
        {
            if (index >= args.length)
            {
                throw new IllegalArgumentException("missing value for " + argument);
            }
            return args[index];
        }

        private static long positiveLong(final String text, final String argument)
        {
            final long value = nonNegativeLong(text, argument);
            if (value == 0L)
            {
                throw new IllegalArgumentException(argument + " must be positive");
            }
            return value;
        }

        private static long nonNegativeLong(final String text, final String argument)
        {
            try
            {
                final long value = Long.parseLong(text.replace("_", ""));
                if (value < 0L)
                {
                    throw new IllegalArgumentException(argument + " must be non-negative");
                }
                return value;
            }
            catch (final NumberFormatException error)
            {
                throw new IllegalArgumentException(argument + ": " + error.getMessage(), error);
            }
        }

        private static int positiveInt(final String text, final String argument)
        {
            final long value = positiveLong(text, argument);
            if (value > Integer.MAX_VALUE)
            {
                throw new IllegalArgumentException(argument + " must fit int");
            }
            return (int) value;
        }

        private static void validateSchedulableRate(
                final Long value,
                final String argument)
        {
            if (value != null && value > MAX_SCHEDULABLE_RATE)
            {
                throw new IllegalArgumentException(
                        argument + " exceeds exact nanosecond scheduling limit "
                                + MAX_SCHEDULABLE_RATE);
            }
        }

        private static int nonNegativeInt(final String text, final String argument)
        {
            final long value = nonNegativeLong(text, argument);
            if (value > Integer.MAX_VALUE)
            {
                throw new IllegalArgumentException(argument + " must fit int");
            }
            return (int) value;
        }

        private static String token(final String text, final String argument)
        {
            if (!text.matches("[A-Za-z0-9._-]+"))
            {
                throw new IllegalArgumentException(argument + " must be a simple token");
            }
            return text;
        }

        private static long[] parseLoadLevels(final String text)
        {
            if (text.isEmpty())
            {
                return new long[0];
            }
            final String[] parts = text.split(",");
            final long[] result = new long[parts.length];
            for (int index = 0; index < parts.length; index++)
            {
                result[index] = positiveLong(parts[index], "--load-levels");
            }
            return result;
        }

        private static long[] parseDurationUsList(final String text)
        {
            final long[] micros = parseLoadLevels(text);
            final long[] nanos = new long[micros.length];
            for (int index = 0; index < micros.length; index++)
            {
                nanos[index] = Math.multiplyExact(micros[index], 1_000L);
            }
            return nanos;
        }

        private static List<Integer> parseCpuList(final String text)
        {
            if (text.isEmpty())
            {
                return List.of();
            }
            final String[] parts = text.split(",");
            final List<Integer> cpus = new ArrayList<>(parts.length);
            for (final String part : parts)
            {
                final int cpu = nonNegativeInt(part, "--cpu-list");
                if (cpus.contains(cpu))
                {
                    throw new IllegalArgumentException("cpu-list must be unique");
                }
                cpus.add(cpu);
            }
            return List.copyOf(cpus);
        }
    }

    private record Target(long loadPct, long targetRate, long injectSleepNs)
    {
    }

    private static final class TailEvent
    {
        long sequence;
        long plannedNs;
    }

    private static final class AllocationPayload
    {
        final long value0;
        final long value1;
        final long value2;
        final long value3;

        AllocationPayload(final long sequence)
        {
            value0 = sequence;
            value1 = sequence + 1L;
            value2 = sequence + 2L;
            value3 = sequence + 3L;
        }

        long checksum()
        {
            return value0 + value1 + value2 + value3;
        }
    }

    private abstract static class CalibrationHandler implements EventHandler<TailEvent>
    {
        final CountDownLatch ready = new CountDownLatch(1);
        final AtomicLong processed = new AtomicLong();
        private final long[] plannedNs;
        private final long[] completionNs;
        private final long[] latencyNs;
        private final int scratchMask;
        AffinityTracker affinity;
        int cpuIndex;
        long epochNanos;

        CalibrationHandler(final int bufferSize)
        {
            plannedNs = new long[bufferSize];
            completionNs = new long[bufferSize];
            latencyNs = new long[bufferSize];
            scratchMask = bufferSize - 1;
        }

        void configure(final AffinityTracker affinity, final int cpuIndex)
        {
            this.affinity = affinity;
            this.cpuIndex = cpuIndex;
        }

        @Override
        public final void onStart()
        {
            affinity.pinCurrent(cpuIndex, "consumer");
            epochNanos = System.nanoTime();
            ready.countDown();
        }

        final void record(final TailEvent event, final long sequence)
        {
            final long completion = System.nanoTime() - epochNanos;
            final int index = (int) sequence & scratchMask;
            plannedNs[index] = event.plannedNs;
            completionNs[index] = completion;
            latencyNs[index] = Math.max(0L, completion - event.plannedNs);
        }

        final void validateCalibration()
        {
            long checksum = 0L;
            for (int index = 0; index < plannedNs.length; index++)
            {
                checksum += plannedNs[index];
                checksum += completionNs[index];
                checksum += latencyNs[index];
            }
            if (checksum == Long.MIN_VALUE)
            {
                throw new IllegalStateException("unreachable calibration scratch checksum");
            }
            validateRetained();
        }

        abstract void validateRetained();
    }

    private static final class AllocationFreeCalibrationHandler extends CalibrationHandler
    {
        AllocationFreeCalibrationHandler(final int bufferSize)
        {
            super(bufferSize);
        }

        @Override
        public void onEvent(
                final TailEvent event,
                final long sequence,
                final boolean endOfBatch)
        {
            record(event, sequence);
            if (endOfBatch)
            {
                processed.lazySet(sequence + 1L);
            }
        }

        @Override
        void validateRetained()
        {
        }
    }

    private static final class AllocatingCalibrationHandler extends CalibrationHandler
    {
        private final AllocationPayload[] retention;
        private int next;

        AllocatingCalibrationHandler(
                final int retentionWindow,
                final int bufferSize)
        {
            super(bufferSize);
            retention = new AllocationPayload[retentionWindow];
        }

        @Override
        public void onEvent(
                final TailEvent event,
                final long sequence,
                final boolean endOfBatch)
        {
            retention[next] = new AllocationPayload(sequence);
            next++;
            if (next == retention.length)
            {
                next = 0;
            }
            record(event, sequence);
            if (endOfBatch)
            {
                processed.lazySet(sequence + 1L);
            }
        }

        @Override
        void validateRetained()
        {
            long checksum = 0L;
            int retained = 0;
            for (final AllocationPayload payload : retention)
            {
                if (payload != null)
                {
                    retained++;
                    checksum += payload.checksum();
                }
            }
            if (retained != retention.length || checksum == 0L)
            {
                throw new IllegalStateException(
                        "allocating calibration did not retain a full observable window");
            }
        }
    }

    private static final class MeasurementState
    {
        final CountDownLatch ready = new CountDownLatch(1);
        final CountDownLatch done = new CountDownLatch(1);
        final long warmupEvents;
        final long finalSequence;
        final long injectSequence;
        final long injectSleepNs;
        final long[] plannedNs;
        final long[] completionNs;
        final long[] latencyNs;
        volatile long epochNanos;
        long pauseStartedNs;
        long pauseCompletedNs;
        long pausePlannedNs;
        long pauseSequence = -1L;

        MeasurementState(final Config config, final Target target)
        {
            warmupEvents = config.warmupEvents;
            finalSequence = config.eventsTotal - 1L;
            final int sampleCount = Math.toIntExact(config.eventsTotal - config.warmupEvents);
            plannedNs = new long[sampleCount];
            completionNs = new long[sampleCount];
            latencyNs = new long[sampleCount];
            if (target.injectSleepNs == 0L)
            {
                injectSequence = -1L;
                injectSleepNs = 0L;
            }
            else
            {
                injectSequence = config.warmupEvents
                        + (config.eventsTotal - config.warmupEvents)
                        * config.injectAtMeasuredPct / 100L;
                injectSleepNs = target.injectSleepNs;
            }
            if (target.targetRate <= 0L)
            {
                throw new IllegalArgumentException("target rate must be positive");
            }
        }

        Samples samples()
        {
            return new Samples(warmupEvents, plannedNs, completionNs, latencyNs);
        }
    }

    private abstract static class MeasurementHandler implements EventHandler<TailEvent>
    {
        final MeasurementState state;
        AffinityTracker affinity;
        int cpuIndex;

        MeasurementHandler(final MeasurementState state)
        {
            this.state = state;
        }

        void configure(final AffinityTracker affinity, final int cpuIndex)
        {
            this.affinity = affinity;
            this.cpuIndex = cpuIndex;
        }

        @Override
        public final void onStart()
        {
            affinity.pinCurrent(cpuIndex, "consumer");
            state.ready.countDown();
        }

        abstract WorkloadStats workloadStats(Config config);
    }

    private static final class AllocationFreeHandler extends MeasurementHandler
    {
        AllocationFreeHandler(final MeasurementState state)
        {
            super(state);
        }

        @Override
        public void onEvent(
                final TailEvent event,
                final long sequence,
                final boolean endOfBatch)
        {
            if (sequence == state.injectSequence)
            {
                final long started = System.nanoTime();
                final long deadline = started + state.injectSleepNs;
                long remaining;
                while ((remaining = deadline - System.nanoTime()) > 0L)
                {
                    LockSupport.parkNanos(remaining);
                }
                state.pauseSequence = sequence;
                state.pausePlannedNs = event.plannedNs;
                state.pauseStartedNs = started - state.epochNanos;
                state.pauseCompletedNs = System.nanoTime() - state.epochNanos;
            }

            final long completionNs = System.nanoTime() - state.epochNanos;
            if (sequence >= state.warmupEvents)
            {
                final int index = (int) (sequence - state.warmupEvents);
                state.plannedNs[index] = event.plannedNs;
                state.completionNs[index] = completionNs;
                state.latencyNs[index] = Math.max(0L, completionNs - event.plannedNs);
            }
            if (sequence == state.finalSequence)
            {
                state.done.countDown();
            }
        }

        @Override
        WorkloadStats workloadStats(final Config config)
        {
            return WorkloadStats.allocationFree();
        }
    }

    private static final class AllocatingHandler extends MeasurementHandler
    {
        private final AllocationPayload[] retention;
        private int next;

        AllocatingHandler(final MeasurementState state, final int retentionWindow)
        {
            super(state);
            retention = new AllocationPayload[retentionWindow];
        }

        @Override
        public void onEvent(
                final TailEvent event,
                final long sequence,
                final boolean endOfBatch)
        {
            if (sequence == state.injectSequence)
            {
                final long started = System.nanoTime();
                final long deadline = started + state.injectSleepNs;
                long remaining;
                while ((remaining = deadline - System.nanoTime()) > 0L)
                {
                    LockSupport.parkNanos(remaining);
                }
                state.pauseSequence = sequence;
                state.pausePlannedNs = event.plannedNs;
                state.pauseStartedNs = started - state.epochNanos;
                state.pauseCompletedNs = System.nanoTime() - state.epochNanos;
            }

            retention[next] = new AllocationPayload(sequence);
            next++;
            if (next == retention.length)
            {
                next = 0;
            }

            final long completionNs = System.nanoTime() - state.epochNanos;
            if (sequence >= state.warmupEvents)
            {
                final int index = (int) (sequence - state.warmupEvents);
                state.plannedNs[index] = event.plannedNs;
                state.completionNs[index] = completionNs;
                state.latencyNs[index] = Math.max(0L, completionNs - event.plannedNs);
            }
            if (sequence == state.finalSequence)
            {
                state.done.countDown();
            }
        }

        @Override
        WorkloadStats workloadStats(final Config config)
        {
            long retainedObjects = 0L;
            long retainedChecksum = 0L;
            for (final AllocationPayload payload : retention)
            {
                if (payload != null)
                {
                    retainedObjects++;
                    retainedChecksum += payload.checksum();
                }
            }
            final long measured = config.eventsTotal - config.warmupEvents;
            final Long bytesPerEvent = config.allocationBytesPerEvent;
            final boolean valid = retainedObjects == retention.length
                    && retainedChecksum != 0L
                    && config.allocationObservationValid();
            return new WorkloadStats(
                    measured,
                    bytesPerEvent == null ? null : Math.multiplyExact(measured, bytesPerEvent),
                    bytesPerEvent,
                    retainedObjects,
                    Math.multiplyExact(retainedObjects, LOGICAL_PAYLOAD_BYTES),
                    bytesPerEvent == null
                            ? null
                            : Math.multiplyExact(retainedObjects, bytesPerEvent),
                    retainedChecksum,
                    valid);
        }
    }

    private record Samples(
            long firstSequence,
            long[] plannedNs,
            long[] completionNs,
            long[] latencyNs)
    {
    }

    private record WorkloadStats(
            long allocations,
            Long allocatedBytes,
            Long allocationBytesPerEvent,
            long retainedObjects,
            long estimatedLogicalLiveBytes,
            Long observedAllocatedLiveBytes,
            long retainedChecksum,
            boolean valid)
    {
        static WorkloadStats allocationFree()
        {
            return new WorkloadStats(0L, 0L, null, 0L, 0L, null, 0L, true);
        }
    }

    private record LatencyStats(
            long count,
            double mean,
            long min,
            long p50,
            long p99,
            long p99_9,
            long p99_99,
            long max)
    {
        static LatencyStats from(final long[] input)
        {
            final long[] sorted = input.clone();
            Arrays.sort(sorted);
            if (sorted.length == 0)
            {
                return new LatencyStats(0L, 0.0, 0L, 0L, 0L, 0L, 0L, 0L);
            }
            double sum = 0.0;
            for (final long value : sorted)
            {
                sum += value;
            }
            return new LatencyStats(
                    sorted.length,
                    sum / (double) sorted.length,
                    sorted[0],
                    percentile(sorted, 5_000L),
                    percentile(sorted, 9_900L),
                    percentile(sorted, 9_990L),
                    percentile(sorted, 9_999L),
                    sorted[sorted.length - 1]);
        }

        private static long percentile(final long[] sorted, final long basisPoints)
        {
            final long numerator = Math.multiplyExact(sorted.length, basisPoints);
            final long rank = (numerator + 9_999L) / 10_000L;
            final int index = (int) Math.max(0L, rank - 1L);
            return sorted[Math.min(index, sorted.length - 1)];
        }
    }

    private record PauseCheck(
            long requestedSleepNs,
            long observedSleepNs,
            long injectionSequence,
            long injectionPlannedNs,
            long pauseStartedNs,
            long pauseCompletedNs,
            long expectedBacklogSamples,
            long expectedAffectedSamples,
            long sampleCount,
            long minimumAffectedSamples,
            long maximumAffectedSamples,
            double loadFraction,
            long minimumDrainNs,
            long remainingPlannedNsAfterPause,
            boolean affectedInRange,
            boolean drainAllowanceMet,
            boolean doubleDrainAllowanceMet,
            boolean p99_9Visible,
            boolean maxVisible)
    {
        static PauseCheck from(
                final long targetRate,
                final double commonMax,
                final LatencyStats stats,
                final MeasurementState state,
                final long lastPlannedNs)
        {
            if (state.pauseSequence < 0L)
            {
                throw new IllegalStateException("configured pause was not observed");
            }
            final long requestedSleepNs = state.injectSleepNs;
            final long observedSleepNs = Math.max(
                    0L, state.pauseCompletedNs - state.pauseStartedNs);
            if (observedSleepNs == 0L)
            {
                throw new IllegalStateException("observed injected pause duration was zero");
            }
            final long expectedBacklog = Math.max(
                    1L,
                    ceilMultiplyDivide(targetRate, observedSleepNs, NANOS_PER_SECOND));
            final long minimum = (stats.count + 999L) / 1_000L;
            final long maximum = stats.count / 10L;
            final long common = (long) Math.floor(commonMax);
            if (targetRate >= common)
            {
                throw new IllegalStateException("target rate must remain below common max");
            }
            final long expectedAffected = Math.max(
                    1L,
                    ceilTripleProductDivide(
                            targetRate,
                            observedSleepNs,
                            common,
                            Math.multiplyExact(
                                    NANOS_PER_SECOND,
                                    common - targetRate)));
            final long drain = ceilMultiplyDivide(
                    observedSleepNs, targetRate, common - targetRate);
            final long remaining = Math.max(0L, lastPlannedNs - state.pauseCompletedNs);
            final boolean backlog =
                    expectedAffected >= minimum && expectedAffected <= maximum;
            final boolean drainMet = remaining >= drain;
            final boolean doubleDrain = remaining >= saturatingMultiply(drain, 2L);
            final boolean p999 = backlog && stats.p99_9 >= observedSleepNs * 0.5;
            final boolean max = stats.max >= observedSleepNs * 0.8;
            return new PauseCheck(
                    requestedSleepNs,
                    observedSleepNs,
                    state.pauseSequence,
                    state.pausePlannedNs,
                    state.pauseStartedNs,
                    state.pauseCompletedNs,
                    expectedBacklog,
                    expectedAffected,
                    stats.count,
                    minimum,
                    maximum,
                    targetRate / commonMax,
                    drain,
                    remaining,
                    backlog,
                    drainMet,
                    doubleDrain,
                    p999,
                    max);
        }

        boolean valid()
        {
            return affectedInRange && drainAllowanceMet && p99_9Visible && maxVisible;
        }

        private static long saturatingMultiply(final long value, final long multiplier)
        {
            try
            {
                return Math.multiplyExact(value, multiplier);
            }
            catch (final ArithmeticException error)
            {
                return Long.MAX_VALUE;
            }
        }
    }

    private record LoadResult(
            long loadPct,
            long targetRate,
            double ownMax,
            double ownUtilization,
            double actualRate,
            boolean rateValid,
            boolean workloadValid,
            boolean validRun,
            long measurementEpochUnixNs,
            long clockAnchorUncertaintyNs,
            PauseCheck pause,
            Samples samples,
            LatencyStats latency,
            WorkloadStats workload)
    {
    }

    private record ClockAnchor(long monotonicNs, long wallUnixNs, long uncertaintyNs)
    {
        static ClockAnchor capture()
        {
            final long before = System.nanoTime();
            final Instant wall = Instant.now();
            final long after = System.nanoTime();
            final long span = Math.max(0L, after - before);
            final long midpoint = before + span / 2L;
            final long unixNs = Math.addExact(
                    Math.multiplyExact(wall.getEpochSecond(), NANOS_PER_SECOND),
                    wall.getNano());
            return new ClockAnchor(midpoint, unixNs, (span + 1L) / 2L);
        }

        long projectUnixNs(final long targetMonotonicNs)
        {
            return Math.addExact(wallUnixNs, targetMonotonicNs - monotonicNs);
        }
    }

    private record Provenance(
            String harnessRev,
            String harnessDirty,
            String implementationRev,
            String implementationDirty)
    {
        static Provenance current()
        {
            return new Provenance(
                    TailBuildProvenance.BADBATCH_GIT_REV,
                    TailBuildProvenance.BADBATCH_GIT_DIRTY,
                    TailBuildProvenance.LMAX_GIT_REV,
                    TailBuildProvenance.LMAX_GIT_DIRTY);
        }

        boolean valid()
        {
            return fullRevision(harnessRev)
                    && fullRevision(implementationRev)
                    && "false".equals(harnessDirty)
                    && "false".equals(implementationDirty);
        }

        private static boolean fullRevision(final String value)
        {
            return value != null && value.matches("[0-9a-fA-F]{40}");
        }

        private static String dirtyJson(final String value)
        {
            return switch (value)
            {
                case "true", "false" -> value;
                default -> "null";
            };
        }
    }

    private static final class AffinityTracker
    {
        private final Config config;
        private final AtomicReference<String> failure = new AtomicReference<>();

        AffinityTracker(final Config config)
        {
            this.config = config;
        }

        void pinCurrent(final int cpuIndex, final String role)
        {
            if (config.cpuList.isEmpty() || failure.get() != null)
            {
                return;
            }
            final int cpu = config.cpuList.get(cpuIndex);
            try
            {
                if (!System.getProperty("os.name").toLowerCase(Locale.ROOT).contains("linux"))
                {
                    throw new IllegalStateException("--cpu-list is supported only on Linux");
                }
                final Path threadLink = Files.readSymbolicLink(Path.of("/proc/thread-self"));
                final String tid = threadLink.getFileName().toString();
                final Process taskset = new ProcessBuilder(
                        "taskset", "-pc", Integer.toString(cpu), tid)
                        .redirectErrorStream(true)
                        .start();
                final String output = new String(
                        taskset.getInputStream().readAllBytes(), StandardCharsets.UTF_8).trim();
                final int status = taskset.waitFor();
                final String observed = currentAllowedCpuList();
                if (status != 0 || !Integer.toString(cpu).equals(observed))
                {
                    throw new IllegalStateException(
                            "taskset status=" + status
                                    + " observed=" + observed
                                    + " output=" + output);
                }
            }
            catch (final Exception error)
            {
                failure.compareAndSet(
                        null, role + "->CPU" + cpu + " failed: " + error.getMessage());
            }
        }

        void verify(final String phase)
        {
            final String message = failure.get();
            if (message != null)
            {
                throw new IllegalStateException(
                        "CPU affinity failed before " + phase + ": " + message);
            }
        }

        boolean verifiedAll()
        {
            return !config.cpuList.isEmpty() && failure.get() == null;
        }

        private static String currentAllowedCpuList() throws Exception
        {
            for (final String line : Files.readAllLines(Path.of("/proc/thread-self/status")))
            {
                if (line.startsWith("Cpus_allowed_list:"))
                {
                    return line.substring(line.indexOf(':') + 1).trim();
                }
            }
            throw new IllegalStateException(
                    "Cpus_allowed_list missing from /proc/thread-self/status");
        }
    }

    private static final class Json
    {
        private Json()
        {
        }

        static String emit(
                final Config config,
                final Double commonMax,
                final double ownMax,
                final List<LoadResult> loads,
                final Provenance provenance,
                final boolean artifactValid)
        {
            final StringBuilder out = new StringBuilder(16_384);
            out.append("{\n");
            field(out, 2, "impl", "lmax-ring-buffer-tail-latency", true);
            field(out, 2, "language", "java", true);
            field(out, 2, "run_mode", config.calibrateOnly ? "calibration" : "measurement", true);
            field(out, 2, "scenario", "unicast_tail_latency", true);
            field(out, 2, "arrival_model", "open_loop_fixed_schedule", true);
            field(out, 2, "latency_origin", "planned_send_time", true);
            out.append("  \"raw_sample_columns\": ")
                    .append("[\"sequence\", \"planned_ns\", \"completion_ns\", \"latency_ns\"],\n");
            field(out, 2, "wait_strategy", config.waitKind.text, true);
            field(out, 2, "event_padding", "none", true);
            field(out, 2, "handler_mode", config.handlerMode.text, true);
            nullableNumber(out, 2, "retention_window", config.retentionWindow, true);
            number(out, 2, "logical_payload_bytes", LOGICAL_PAYLOAD_BYTES, true);
            nullableNumber(
                    out, 2, "allocation_payload_bytes", config.allocationBytesPerEvent, true);
            field(
                    out,
                    2,
                    "allocation_measurement_source",
                    config.allocationMeasurementSource,
                    true);
            field(out, 2, "api_path", "ring_buffer_batch_event_processor", true);
            number(out, 2, "buffer_size", config.bufferSize, true);
            number(out, 2, "events_total", config.eventsTotal, true);
            number(out, 2, "warmup_events", config.warmupEvents, true);
            number(
                    out,
                    2,
                    "calibration_warmup_events",
                    config.calibrationWarmupEvents(),
                    true);
            number(out, 2, "calibration_events_limit", config.calibrationEvents, true);
            number(
                    out,
                    2,
                    "calibration_duration_ms",
                    config.calibrationDuration.toMillis(),
                    true);
            decimal(out, 2, "max_rate", commonMax == null ? ownMax : commonMax, true);
            decimal(out, 2, "own_max", ownMax, true);
            nullableDecimal(out, 2, "common_max", commonMax, true);
            decimal(out, 2, "minimum_actual_target_ratio", VALID_RUN_THRESHOLD, true);
            out.append("  \"inject_sleep_us_by_load\": [");
            for (int index = 0; index < config.injectSleepNsByLoad.length; index++)
            {
                if (index > 0)
                {
                    out.append(", ");
                }
                out.append(config.injectSleepNsByLoad[index] / 1_000L);
            }
            out.append("],\n");
            number(
                    out,
                    2,
                    "inject_at_measured_pct",
                    config.injectAtMeasuredPct,
                    true);
            field(out, 2, "provenance_source", "build_time", true);
            bool(out, 2, "provenance_valid", provenance.valid(), true);
            field(out, 2, "git_rev", provenance.harnessRev, true);
            raw(out, 2, "dirty", Provenance.dirtyJson(provenance.harnessDirty), true);
            field(out, 2, "harness_git_rev", provenance.harnessRev, true);
            raw(
                    out,
                    2,
                    "harness_git_dirty",
                    Provenance.dirtyJson(provenance.harnessDirty),
                    true);
            field(out, 2, "implementation_git_rev", provenance.implementationRev, true);
            raw(
                    out,
                    2,
                    "implementation_git_dirty",
                    Provenance.dirtyJson(provenance.implementationDirty),
                    true);
            bool(out, 2, "artifact_valid", artifactValid, true);
            nullableString(out, 2, "gc_log", config.gcLog, true);
            nullableString(out, 2, "jfr_file", config.jfrFile, true);
            appendJvm(out);
            appendAffinity(out, config);
            out.append("  \"loads\": [\n");
            for (int index = 0; index < loads.size(); index++)
            {
                appendLoad(out, config, loads.get(index));
                out.append(index + 1 == loads.size() ? "\n" : ",\n");
            }
            out.append("  ]\n");
            out.append("}\n");
            return out.toString();
        }

        private static void appendJvm(final StringBuilder out)
        {
            out.append("  \"jvm\": {\n");
            field(out, 4, "java_version", System.getProperty("java.version"), true);
            field(out, 4, "vm_name", System.getProperty("java.vm.name"), true);
            field(out, 4, "vm_version", System.getProperty("java.vm.version"), true);
            out.append("    \"input_arguments\": [");
            final List<String> arguments =
                    ManagementFactory.getRuntimeMXBean().getInputArguments();
            for (int index = 0; index < arguments.size(); index++)
            {
                if (index > 0)
                {
                    out.append(", ");
                }
                string(out, arguments.get(index));
            }
            out.append("],\n");
            out.append("    \"gc_names\": [");
            final var collectors = ManagementFactory.getGarbageCollectorMXBeans();
            for (int index = 0; index < collectors.size(); index++)
            {
                if (index > 0)
                {
                    out.append(", ");
                }
                string(out, collectors.get(index).getName());
            }
            out.append("]\n");
            out.append("  },\n");
        }

        private static void appendAffinity(final StringBuilder out, final Config config)
        {
            out.append("  \"cpu_affinity\": {\n");
            out.append("    \"requested_cpu_list\": [");
            for (int index = 0; index < config.cpuList.size(); index++)
            {
                if (index > 0)
                {
                    out.append(", ");
                }
                out.append(config.cpuList.get(index));
            }
            out.append("],\n");
            field(
                    out,
                    4,
                    "mode",
                    config.cpuList.isEmpty() ? "none" : "per-thread",
                    true);
            bool(out, 4, "verified_all", config.affinityVerifiedAll, true);
            out.append("    \"role_cpu_map\": {");
            if (config.cpuList.size() >= 2)
            {
                out.append("\n");
                number(out, 6, "producer", config.cpuList.get(0), true);
                number(out, 6, "consumer", config.cpuList.get(1), false);
                out.append("    ");
            }
            out.append("}\n");
            out.append("  },\n");
        }

        private static void appendLoad(
                final StringBuilder out,
                final Config config,
                final LoadResult load)
        {
            out.append("    {\n");
            number(out, 6, "load_pct", load.loadPct, true);
            number(out, 6, "target_rate", load.targetRate, true);
            decimal(out, 6, "own_max", load.ownMax, true);
            decimal(out, 6, "own_utilization", load.ownUtilization, true);
            decimal(out, 6, "actual_rate", load.actualRate, true);
            decimal(
                    out,
                    6,
                    "actual_target_ratio",
                    load.actualRate / load.targetRate,
                    true);
            bool(out, 6, "rate_valid", load.rateValid, true);
            bool(out, 6, "workload_valid", load.workloadValid, true);
            bool(out, 6, "valid_run", load.validRun, true);
            number(
                    out,
                    6,
                    "measurement_epoch_unix_ns",
                    load.measurementEpochUnixNs,
                    true);
            number(
                    out,
                    6,
                    "clock_anchor_uncertainty_ns",
                    load.clockAnchorUncertaintyNs,
                    true);
            if (load.pause != null)
            {
                appendPause(out, load.pause);
            }
            appendWorkload(out, config, load);
            appendLatency(out, load.latency);
            out.append("    }");
        }

        private static void appendPause(final StringBuilder out, final PauseCheck pause)
        {
            out.append("      \"pause_validation\": {\n");
            number(out, 8, "requested_sleep_ns", pause.requestedSleepNs, true);
            number(out, 8, "observed_sleep_ns", pause.observedSleepNs, true);
            number(out, 8, "injection_sequence", pause.injectionSequence, true);
            number(out, 8, "injection_planned_ns", pause.injectionPlannedNs, true);
            number(out, 8, "pause_started_ns", pause.pauseStartedNs, true);
            number(out, 8, "pause_completed_ns", pause.pauseCompletedNs, true);
            number(
                    out,
                    8,
                    "expected_backlog_samples",
                    pause.expectedBacklogSamples,
                    true);
            number(
                    out,
                    8,
                    "expected_affected_samples",
                    pause.expectedAffectedSamples,
                    true);
            number(out, 8, "sample_count", pause.sampleCount, true);
            number(
                    out,
                    8,
                    "minimum_affected_samples",
                    pause.minimumAffectedSamples,
                    true);
            number(
                    out,
                    8,
                    "maximum_affected_samples",
                    pause.maximumAffectedSamples,
                    true);
            bool(out, 8, "affected_in_range", pause.affectedInRange, true);
            decimal(out, 8, "load_fraction", pause.loadFraction, true);
            number(out, 8, "minimum_drain_ns", pause.minimumDrainNs, true);
            number(
                    out,
                    8,
                    "remaining_planned_ns_after_pause",
                    pause.remainingPlannedNsAfterPause,
                    true);
            bool(out, 8, "drain_allowance_met", pause.drainAllowanceMet, true);
            bool(
                    out,
                    8,
                    "double_drain_allowance_met",
                    pause.doubleDrainAllowanceMet,
                    true);
            bool(out, 8, "p99.9_visible", pause.p99_9Visible, true);
            bool(out, 8, "max_visible", pause.maxVisible, true);
            bool(out, 8, "valid", pause.valid(), false);
            out.append("      },\n");
        }

        private static void appendWorkload(
                final StringBuilder out,
                final Config config,
                final LoadResult load)
        {
            final WorkloadStats workload = load.workload;
            out.append("      \"workload\": {\n");
            field(out, 8, "mode", config.handlerMode.text, true);
            nullableNumber(out, 8, "retention_window", config.retentionWindow, true);
            decimal(
                    out,
                    8,
                    "retention_seconds",
                    config.retentionWindow == null
                            ? 0.0
                            : config.retentionWindow / (double) load.targetRate,
                    true);
            number(out, 8, "logical_payload_bytes", LOGICAL_PAYLOAD_BYTES, true);
            nullableNumber(
                    out,
                    8,
                    "allocation_payload_bytes",
                    workload.allocationBytesPerEvent,
                    true);
            nullableNumber(
                    out,
                    8,
                    "runtime_or_matching_overhead_bytes",
                    workload.allocationBytesPerEvent == null
                            ? null
                            : workload.allocationBytesPerEvent - LOGICAL_PAYLOAD_BYTES,
                    true);
            field(
                    out,
                    8,
                    "allocation_count_source",
                    config.handlerMode == HandlerMode.ALLOCATION_FREE
                            ? "none"
                            : "one_new_bytecode_per_event",
                    true);
            number(out, 8, "allocations", workload.allocations, true);
            nullableNumber(out, 8, "allocated_bytes", workload.allocatedBytes, true);
            nullableDecimal(
                    out,
                    8,
                    "allocation_bytes_per_event",
                    workload.allocationBytesPerEvent == null
                            ? null
                            : workload.allocationBytesPerEvent.doubleValue(),
                    true);
            raw(out, 8, "deallocations", "null", true);
            raw(out, 8, "deallocated_bytes", "null", true);
            number(out, 8, "retained_objects", workload.retainedObjects, true);
            number(
                    out,
                    8,
                    "estimated_logical_live_bytes",
                    workload.estimatedLogicalLiveBytes,
                    true);
            nullableNumber(
                    out,
                    8,
                    "observed_allocated_live_bytes",
                    workload.observedAllocatedLiveBytes,
                    true);
            number(out, 8, "retained_checksum", workload.retainedChecksum, true);
            bool(out, 8, "valid", workload.valid, false);
            out.append("      },\n");
        }

        private static void appendLatency(
                final StringBuilder out,
                final LatencyStats latency)
        {
            out.append("      \"latency_ns\": {\n");
            number(out, 8, "count", latency.count, true);
            decimal(out, 8, "mean", latency.mean, true);
            number(out, 8, "min", latency.min, true);
            number(out, 8, "p50", latency.p50, true);
            number(out, 8, "p99", latency.p99, true);
            number(out, 8, "p99.9", latency.p99_9, true);
            number(out, 8, "p99.99", latency.p99_99, true);
            number(out, 8, "max", latency.max, false);
            out.append("      }\n");
        }

        private static void field(
                final StringBuilder out,
                final int spaces,
                final String key,
                final String value,
                final boolean comma)
        {
            indent(out, spaces);
            string(out, key);
            out.append(": ");
            string(out, value);
            out.append(comma ? ",\n" : "\n");
        }

        private static void nullableString(
                final StringBuilder out,
                final int spaces,
                final String key,
                final String value,
                final boolean comma)
        {
            if (value == null)
            {
                raw(out, spaces, key, "null", comma);
            }
            else
            {
                field(out, spaces, key, value, comma);
            }
        }

        private static void number(
                final StringBuilder out,
                final int spaces,
                final String key,
                final long value,
                final boolean comma)
        {
            raw(out, spaces, key, Long.toString(value), comma);
        }

        private static void nullableNumber(
                final StringBuilder out,
                final int spaces,
                final String key,
                final Number value,
                final boolean comma)
        {
            raw(out, spaces, key, value == null ? "null" : value.toString(), comma);
        }

        private static void decimal(
                final StringBuilder out,
                final int spaces,
                final String key,
                final double value,
                final boolean comma)
        {
            raw(out, spaces, key, String.format(Locale.ROOT, "%.9f", value), comma);
        }

        private static void nullableDecimal(
                final StringBuilder out,
                final int spaces,
                final String key,
                final Double value,
                final boolean comma)
        {
            if (value == null)
            {
                raw(out, spaces, key, "null", comma);
            }
            else
            {
                decimal(out, spaces, key, value, comma);
            }
        }

        private static void bool(
                final StringBuilder out,
                final int spaces,
                final String key,
                final boolean value,
                final boolean comma)
        {
            raw(out, spaces, key, Boolean.toString(value), comma);
        }

        private static void raw(
                final StringBuilder out,
                final int spaces,
                final String key,
                final String value,
                final boolean comma)
        {
            indent(out, spaces);
            string(out, key);
            out.append(": ").append(value).append(comma ? ",\n" : "\n");
        }

        private static void string(final StringBuilder out, final String value)
        {
            out.append('"');
            for (int index = 0; index < value.length(); index++)
            {
                final char character = value.charAt(index);
                switch (character)
                {
                    case '"' -> out.append("\\\"");
                    case '\\' -> out.append("\\\\");
                    case '\b' -> out.append("\\b");
                    case '\f' -> out.append("\\f");
                    case '\n' -> out.append("\\n");
                    case '\r' -> out.append("\\r");
                    case '\t' -> out.append("\\t");
                    default ->
                    {
                        if (character < 0x20)
                        {
                            out.append(String.format(Locale.ROOT, "\\u%04x", (int) character));
                        }
                        else
                        {
                            out.append(character);
                        }
                    }
                }
            }
            out.append('"');
        }

        private static void indent(final StringBuilder out, final int spaces)
        {
            out.append(" ".repeat(spaces));
        }
    }
}
