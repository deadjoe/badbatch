package com.lmax.disruptor.headtohead;

import com.lmax.disruptor.BatchEventProcessor;
import com.lmax.disruptor.BatchEventProcessorBuilder;
import com.lmax.disruptor.BusySpinWaitStrategy;
import com.lmax.disruptor.EventHandler;
import com.lmax.disruptor.RingBuffer;
import com.lmax.disruptor.SequenceBarrier;
import com.lmax.disruptor.WaitStrategy;
import com.lmax.disruptor.YieldingWaitStrategy;
import com.lmax.disruptor.util.DaemonThreadFactory;

import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.time.Duration;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;
import java.util.Locale;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.concurrent.atomic.AtomicLong;

/**
 * LMAX Disruptor side of the BadBatch head-to-head harness.
 *
 * <p>Contract must stay aligned with {@code src/bin/h2h_rust.rs}:
 * same scenarios, event payload, checksums, batching, warmup/measured rounds,
 * and JSON summary fields ({@code median_ops_per_sec}, etc.).
 *
 * <p>Java API under test: idiomatic LMAX {@link RingBuffer} + {@link BatchEventProcessor}.
 */
public final class HeadToHead
{
    private static final int MPSC_PRODUCERS = 3;
    private static final Duration DEFAULT_TIMEOUT = Duration.ofSeconds(300);

    private HeadToHead()
    {
    }

    public static void main(final String[] args) throws Exception
    {
        final Config config = Config.parse(args);
        final List<Round> rounds = runScenario(config);
        final Summary summary = Summary.from(rounds, config.warmupRounds);
        final String json = Json.emit(config, rounds, summary);

        if (config.outputPath != null)
        {
            final Path path = Path.of(config.outputPath);
            if (path.getParent() != null)
            {
                Files.createDirectories(path.getParent());
            }
            Files.writeString(path, json, StandardCharsets.UTF_8);
        }
        System.out.print(json);

        if (!summary.checksumValidAll)
        {
            System.exit(2);
        }
    }

    private static List<Round> runScenario(final Config config) throws Exception
    {
        return switch (config.scenario)
        {
            case UNICAST -> runUnicast(config, false);
            case UNICAST_BATCH -> runUnicast(config, true);
            case MPSC_BATCH -> runMpscBatch(config);
            case PIPELINE -> runPipeline(config);
        };
    }

    private static List<Round> runUnicast(final Config config, final boolean batch) throws Exception
    {
        final long expected = arithmeticChecksum(config.eventsTotal);
        final int totalRounds = config.warmupRounds + config.measuredRounds;
        final List<Round> rounds = new ArrayList<>(totalRounds);

        for (int i = 0; i < totalRounds; i++)
        {
            final AtomicInteger ready = new AtomicInteger();
            final AtomicLong processed = new AtomicLong();
            final AtomicLong checksum = new AtomicLong();
            final SummingHandler handler =
                    new SummingHandler(processed, checksum, ready, config.eventsTotal);

            final RingBuffer<ComparisonEvent> ring =
                    RingBuffer.createSingleProducer(
                            ComparisonEvent::new, config.bufferSize, waitStrategy(config.wait));
            final SequenceBarrier barrier = ring.newBarrier();
            final BatchEventProcessor<ComparisonEvent> processor =
                    new BatchEventProcessorBuilder().build(ring, barrier, handler);
            ring.addGatingSequences(processor.getSequence());

            final ExecutorService executor =
                    Executors.newSingleThreadExecutor(DaemonThreadFactory.INSTANCE);
            executor.submit(processor);
            waitForCount(ready, 1, "unicast consumer ready");

            final long start = System.nanoTime();
            if (batch)
            {
                long published = 0L;
                while (published < config.eventsTotal)
                {
                    final int chunk = (int) Math.min(config.batchSize, config.eventsTotal - published);
                    final long hi = ring.next(chunk);
                    final long lo = hi - (chunk - 1L);
                    for (int j = 0; j < chunk; j++)
                    {
                        final ComparisonEvent event = ring.get(lo + j);
                        event.value = published + j;
                        event.stage1Value = 0L;
                        event.stage2Value = 0L;
                        event.stage3Value = 0L;
                    }
                    ring.publish(lo, hi);
                    published += chunk;
                }
            }
            else
            {
                for (long v = 0; v < config.eventsTotal; v++)
                {
                    final long sequence = ring.next();
                    final ComparisonEvent event = ring.get(sequence);
                    event.value = v;
                    event.stage1Value = 0L;
                    event.stage2Value = 0L;
                    event.stage3Value = 0L;
                    ring.publish(sequence);
                }
            }
            waitForCount(processed, config.eventsTotal, "unicast completion");
            final long elapsed = System.nanoTime() - start;

            rounds.add(Round.of(
                    i + 1,
                    i < config.warmupRounds ? "warmup" : "measured",
                    elapsed,
                    config.eventsTotal,
                    checksum.get() == expected));

            processor.halt();
            shutdown(executor);
        }
        return rounds;
    }

    private static List<Round> runMpscBatch(final Config config) throws Exception
    {
        final long expected = arithmeticChecksum(config.eventsTotal);
        final long perProducer = config.eventsTotal / MPSC_PRODUCERS;
        final int totalRounds = config.warmupRounds + config.measuredRounds;
        final List<Round> rounds = new ArrayList<>(totalRounds);

        for (int i = 0; i < totalRounds; i++)
        {
            final AtomicInteger ready = new AtomicInteger();
            final AtomicLong processed = new AtomicLong();
            final AtomicLong checksum = new AtomicLong();
            final SummingHandler handler =
                    new SummingHandler(processed, checksum, ready, config.eventsTotal);

            final RingBuffer<ComparisonEvent> ring =
                    RingBuffer.createMultiProducer(
                            ComparisonEvent::new, config.bufferSize, waitStrategy(config.wait));
            final SequenceBarrier barrier = ring.newBarrier();
            final BatchEventProcessor<ComparisonEvent> processor =
                    new BatchEventProcessorBuilder().build(ring, barrier, handler);
            ring.addGatingSequences(processor.getSequence());

            final ExecutorService executor =
                    Executors.newSingleThreadExecutor(DaemonThreadFactory.INSTANCE);
            executor.submit(processor);
            waitForCount(ready, 1, "mpsc consumer ready");

            final CountDownLatch producersReady = new CountDownLatch(MPSC_PRODUCERS);
            final CountDownLatch startSignal = new CountDownLatch(1);
            final Thread[] producers = new Thread[MPSC_PRODUCERS];

            for (int id = 0; id < MPSC_PRODUCERS; id++)
            {
                final int index = id;
                producers[id] = new Thread(() -> {
                    producersReady.countDown();
                    await(startSignal);
                    final long rangeStart = perProducer * index;
                    final long rangeEnd = rangeStart + perProducer;
                    long published = rangeStart;
                    while (published < rangeEnd)
                    {
                        final int chunk = (int) Math.min(config.batchSize, rangeEnd - published);
                        final long hi = ring.next(chunk);
                        final long lo = hi - (chunk - 1L);
                        for (int j = 0; j < chunk; j++)
                        {
                            final ComparisonEvent event = ring.get(lo + j);
                            event.value = published + j;
                            event.stage1Value = 0L;
                            event.stage2Value = 0L;
                            event.stage3Value = 0L;
                        }
                        ring.publish(lo, hi);
                        published += chunk;
                    }
                }, "h2h-mpsc-" + id);
                producers[id].start();
            }

            producersReady.await();
            final long start = System.nanoTime();
            startSignal.countDown();
            for (final Thread producer : producers)
            {
                producer.join();
            }
            waitForCount(processed, config.eventsTotal, "mpsc completion");
            final long elapsed = System.nanoTime() - start;

            rounds.add(Round.of(
                    i + 1,
                    i < config.warmupRounds ? "warmup" : "measured",
                    elapsed,
                    config.eventsTotal,
                    checksum.get() == expected));

            processor.halt();
            shutdown(executor);
        }
        return rounds;
    }

    private static List<Round> runPipeline(final Config config) throws Exception
    {
        final long expected = pipelineChecksum(config.eventsTotal);
        final int totalRounds = config.warmupRounds + config.measuredRounds;
        final List<Round> rounds = new ArrayList<>(totalRounds);

        for (int i = 0; i < totalRounds; i++)
        {
            final AtomicInteger ready = new AtomicInteger();
            final AtomicLong processed = new AtomicLong();
            final AtomicLong checksum = new AtomicLong();

            final RingBuffer<ComparisonEvent> ring =
                    RingBuffer.createSingleProducer(
                            ComparisonEvent::new, config.bufferSize, waitStrategy(config.wait));

            final Stage1Handler stage1 = new Stage1Handler(ready);
            final BatchEventProcessor<ComparisonEvent> p1 =
                    new BatchEventProcessorBuilder().build(ring, ring.newBarrier(), stage1);

            final Stage2Handler stage2 = new Stage2Handler(ready);
            final BatchEventProcessor<ComparisonEvent> p2 =
                    new BatchEventProcessorBuilder()
                            .build(ring, ring.newBarrier(p1.getSequence()), stage2);

            final Stage3Handler stage3 =
                    new Stage3Handler(processed, checksum, ready, config.eventsTotal);
            final BatchEventProcessor<ComparisonEvent> p3 =
                    new BatchEventProcessorBuilder()
                            .build(ring, ring.newBarrier(p2.getSequence()), stage3);

            ring.addGatingSequences(p3.getSequence());

            final ExecutorService executor =
                    Executors.newFixedThreadPool(3, DaemonThreadFactory.INSTANCE);
            executor.submit(p1);
            executor.submit(p2);
            executor.submit(p3);
            waitForCount(ready, 3, "pipeline stages ready");

            final long start = System.nanoTime();
            for (long v = 0; v < config.eventsTotal; v++)
            {
                final long sequence = ring.next();
                final ComparisonEvent event = ring.get(sequence);
                event.value = v;
                event.stage1Value = 0L;
                event.stage2Value = 0L;
                event.stage3Value = 0L;
                ring.publish(sequence);
            }
            waitForCount(processed, config.eventsTotal, "pipeline completion");
            final long elapsed = System.nanoTime() - start;

            rounds.add(Round.of(
                    i + 1,
                    i < config.warmupRounds ? "warmup" : "measured",
                    elapsed,
                    config.eventsTotal,
                    checksum.get() == expected));

            p1.halt();
            p2.halt();
            p3.halt();
            shutdown(executor);
        }
        return rounds;
    }

    private static WaitStrategy waitStrategy(final WaitKind kind)
    {
        return switch (kind)
        {
            case BUSY_SPIN -> new BusySpinWaitStrategy();
            case YIELDING -> new YieldingWaitStrategy();
        };
    }

    private static long arithmeticChecksum(final long eventsTotal)
    {
        return eventsTotal * (eventsTotal - 1L) / 2L;
    }

    private static long pipelineChecksum(final long eventsTotal)
    {
        long sum = 0L;
        for (long v = 0; v < eventsTotal; v++)
        {
            sum += v + 11L; // +1 +3 +7
        }
        return sum;
    }

    private static void waitForCount(final AtomicInteger counter, final int expected, final String label)
            throws InterruptedException
    {
        final long deadline = System.nanoTime() + DEFAULT_TIMEOUT.toNanos();
        while (counter.get() < expected)
        {
            if (System.nanoTime() >= deadline)
            {
                throw new IllegalStateException(
                        "timeout " + label + ": expected>=" + expected + " got=" + counter.get());
            }
            Thread.onSpinWait();
        }
    }

    private static void waitForCount(final AtomicLong counter, final long expected, final String label)
            throws InterruptedException
    {
        final long deadline = System.nanoTime() + DEFAULT_TIMEOUT.toNanos();
        while (counter.get() < expected)
        {
            if (System.nanoTime() >= deadline)
            {
                throw new IllegalStateException(
                        "timeout " + label + ": expected>=" + expected + " got=" + counter.get());
            }
            Thread.onSpinWait();
        }
    }

    private static void await(final CountDownLatch latch)
    {
        try
        {
            latch.await();
        }
        catch (final InterruptedException e)
        {
            Thread.currentThread().interrupt();
            throw new IllegalStateException(e);
        }
    }

    private static void shutdown(final ExecutorService executor) throws InterruptedException
    {
        executor.shutdownNow();
        executor.awaitTermination(5, TimeUnit.SECONDS);
    }

    // --- handlers ------------------------------------------------------------------

    static final class ComparisonEvent
    {
        long value;
        long stage1Value;
        long stage2Value;
        long stage3Value;
    }

    private static final class SummingHandler implements EventHandler<ComparisonEvent>
    {
        private final AtomicLong processed;
        private final AtomicLong checksum;
        private final AtomicInteger ready;
        private final long eventsTotal;
        private final long finalSequence;
        private long localChecksum;

        SummingHandler(
                final AtomicLong processed,
                final AtomicLong checksum,
                final AtomicInteger ready,
                final long eventsTotal)
        {
            this.processed = processed;
            this.checksum = checksum;
            this.ready = ready;
            this.eventsTotal = eventsTotal;
            this.finalSequence = eventsTotal - 1L;
        }

        @Override
        public void onStart()
        {
            ready.incrementAndGet();
        }

        @Override
        public void onEvent(final ComparisonEvent event, final long sequence, final boolean endOfBatch)
        {
            localChecksum += event.value;
            if (sequence == finalSequence)
            {
                checksum.set(localChecksum);
                processed.set(eventsTotal);
            }
        }
    }

    private static final class Stage1Handler implements EventHandler<ComparisonEvent>
    {
        private final AtomicInteger ready;

        Stage1Handler(final AtomicInteger ready)
        {
            this.ready = ready;
        }

        @Override
        public void onStart()
        {
            ready.incrementAndGet();
        }

        @Override
        public void onEvent(final ComparisonEvent event, final long sequence, final boolean endOfBatch)
        {
            event.stage1Value = event.value + 1L;
        }
    }

    private static final class Stage2Handler implements EventHandler<ComparisonEvent>
    {
        private final AtomicInteger ready;

        Stage2Handler(final AtomicInteger ready)
        {
            this.ready = ready;
        }

        @Override
        public void onStart()
        {
            ready.incrementAndGet();
        }

        @Override
        public void onEvent(final ComparisonEvent event, final long sequence, final boolean endOfBatch)
        {
            event.stage2Value = event.stage1Value + 3L;
        }
    }

    private static final class Stage3Handler implements EventHandler<ComparisonEvent>
    {
        private final AtomicLong processed;
        private final AtomicLong checksum;
        private final AtomicInteger ready;
        private final long eventsTotal;
        private final long finalSequence;
        private long localChecksum;

        Stage3Handler(
                final AtomicLong processed,
                final AtomicLong checksum,
                final AtomicInteger ready,
                final long eventsTotal)
        {
            this.processed = processed;
            this.checksum = checksum;
            this.ready = ready;
            this.eventsTotal = eventsTotal;
            this.finalSequence = eventsTotal - 1L;
        }

        @Override
        public void onStart()
        {
            ready.incrementAndGet();
        }

        @Override
        public void onEvent(final ComparisonEvent event, final long sequence, final boolean endOfBatch)
        {
            event.stage3Value = event.stage2Value + 7L;
            localChecksum += event.stage3Value;
            if (sequence == finalSequence)
            {
                checksum.set(localChecksum);
                processed.set(eventsTotal);
            }
        }
    }

    // --- config / results ----------------------------------------------------------

    enum Scenario
    {
        UNICAST,
        UNICAST_BATCH,
        MPSC_BATCH,
        PIPELINE;

        static Scenario parse(final String s)
        {
            return switch (s)
            {
                case "unicast" -> UNICAST;
                case "unicast_batch" -> UNICAST_BATCH;
                case "mpsc_batch" -> MPSC_BATCH;
                case "pipeline" -> PIPELINE;
                default -> throw new IllegalArgumentException("unsupported scenario: " + s);
            };
        }

        String wire()
        {
            return name().toLowerCase(Locale.ROOT);
        }
    }

    enum WaitKind
    {
        BUSY_SPIN,
        YIELDING;

        static WaitKind parse(final String s)
        {
            return switch (s)
            {
                case "busy-spin" -> BUSY_SPIN;
                case "yielding" -> YIELDING;
                default -> throw new IllegalArgumentException("unsupported wait-strategy: " + s);
            };
        }

        String wire()
        {
            return this == BUSY_SPIN ? "busy-spin" : "yielding";
        }
    }

    static final class Config
    {
        Scenario scenario;
        WaitKind wait;
        int bufferSize;
        long eventsTotal;
        int batchSize;
        int warmupRounds = 3;
        int measuredRounds = 7;
        String runOrder = "standalone";
        String pairId = "standalone";
        int forkIndex;
        String harnessRev = "unknown";
        String implementationRev = "unknown";
        boolean harnessDirty;
        boolean implementationDirty;
        String implLabel = "lmax-bep";
        String outputPath;
        boolean quick;

        static Config parse(final String[] args)
        {
            final Config c = new Config();
            for (int i = 0; i < args.length; i++)
            {
                switch (args[i])
                {
                    case "--scenario" -> c.scenario = Scenario.parse(args[++i]);
                    case "--wait-strategy" -> c.wait = WaitKind.parse(args[++i]);
                    case "--event-padding" ->
                    {
                        final String p = args[++i];
                        if (!"none".equals(p))
                        {
                            // Java object layout is not slot-padded like BadBatch; only "none" is comparable.
                            if (!"128".equals(p) && !"64".equals(p))
                            {
                                throw new IllegalArgumentException("unsupported event-padding: " + p);
                            }
                            // accept but ignore padding on Java side (documented asymmetry)
                        }
                    }
                    case "--buffer-size" -> c.bufferSize = Integer.parseInt(args[++i]);
                    case "--events-total" -> c.eventsTotal = Long.parseLong(args[++i].replace("_", ""));
                    case "--batch-size" -> c.batchSize = Integer.parseInt(args[++i]);
                    case "--warmup-rounds" -> c.warmupRounds = Integer.parseInt(args[++i]);
                    case "--measured-rounds" -> c.measuredRounds = Integer.parseInt(args[++i]);
                    case "--run-order" -> c.runOrder = args[++i];
                    case "--pair-id" -> c.pairId = args[++i];
                    case "--fork-index" -> c.forkIndex = Integer.parseInt(args[++i]);
                    case "--harness-rev" -> c.harnessRev = args[++i];
                    case "--implementation-rev" -> c.implementationRev = args[++i];
                    case "--harness-dirty" -> c.harnessDirty = Boolean.parseBoolean(args[++i]);
                    case "--implementation-dirty" ->
                            c.implementationDirty = Boolean.parseBoolean(args[++i]);
                    case "--impl-label" -> c.implLabel = args[++i];
                    case "--output" -> c.outputPath = args[++i];
                    case "--quick" -> c.quick = true;
                    case "--help", "-h" ->
                    {
                        System.out.println("Usage: HeadToHead --scenario <...> [options] (mirrors h2h_rust)");
                        System.exit(0);
                    }
                    default -> throw new IllegalArgumentException("unknown argument: " + args[i]);
                }
            }
            if (c.scenario == null)
            {
                throw new IllegalArgumentException("required: --scenario");
            }
            if (c.wait == null)
            {
                c.wait = c.scenario == Scenario.MPSC_BATCH ? WaitKind.BUSY_SPIN : WaitKind.YIELDING;
            }
            if (c.bufferSize == 0)
            {
                c.bufferSize = c.scenario == Scenario.PIPELINE ? 8192 : 65536;
            }
            if (c.eventsTotal == 0)
            {
                if (c.quick)
                {
                    c.eventsTotal = c.scenario == Scenario.MPSC_BATCH ? 3_000_000L : 1_000_000L;
                }
                else
                {
                    c.eventsTotal = c.scenario == Scenario.MPSC_BATCH ? 60_000_000L : 100_000_000L;
                }
            }
            if (c.batchSize == 0)
            {
                c.batchSize =
                        (c.scenario == Scenario.UNICAST || c.scenario == Scenario.PIPELINE) ? 1 : 10;
            }
            if (c.quick)
            {
                c.warmupRounds = Math.min(c.warmupRounds, 1);
                c.measuredRounds = Math.min(c.measuredRounds, 2);
            }
            if (c.scenario == Scenario.MPSC_BATCH && c.eventsTotal % MPSC_PRODUCERS != 0)
            {
                throw new IllegalArgumentException("mpsc events must be divisible by " + MPSC_PRODUCERS);
            }
            return c;
        }
    }

    static final class Round
    {
        final int index;
        final String phase;
        final long elapsedNs;
        final long events;
        final double opsPerSec;
        final boolean checksumValid;

        private Round(
                final int index,
                final String phase,
                final long elapsedNs,
                final long events,
                final double opsPerSec,
                final boolean checksumValid)
        {
            this.index = index;
            this.phase = phase;
            this.elapsedNs = elapsedNs;
            this.events = events;
            this.opsPerSec = opsPerSec;
            this.checksumValid = checksumValid;
        }

        static Round of(
                final int index,
                final String phase,
                final long elapsedNs,
                final long events,
                final boolean checksumValid)
        {
            final double ops = events / (elapsedNs / 1_000_000_000.0);
            return new Round(index, phase, elapsedNs, events, ops, checksumValid);
        }
    }

    static final class Summary
    {
        final boolean checksumValidAll;
        final double medianOpsPerSec;
        final double meanOpsPerSec;
        final double minOpsPerSec;
        final double maxOpsPerSec;
        final double stddevOpsPerSec;
        final double cv;

        Summary(
                final boolean checksumValidAll,
                final double medianOpsPerSec,
                final double meanOpsPerSec,
                final double minOpsPerSec,
                final double maxOpsPerSec,
                final double stddevOpsPerSec,
                final double cv)
        {
            this.checksumValidAll = checksumValidAll;
            this.medianOpsPerSec = medianOpsPerSec;
            this.meanOpsPerSec = meanOpsPerSec;
            this.minOpsPerSec = minOpsPerSec;
            this.maxOpsPerSec = maxOpsPerSec;
            this.stddevOpsPerSec = stddevOpsPerSec;
            this.cv = cv;
        }

        static Summary from(final List<Round> rounds, final int warmupRounds)
        {
            final boolean allOk = rounds.stream().allMatch(r -> r.checksumValid);
            final double[] measured =
                    rounds.stream()
                            .skip(warmupRounds)
                            .mapToDouble(r -> r.opsPerSec)
                            .toArray();
            if (measured.length == 0)
            {
                return new Summary(allOk, 0, 0, 0, 0, 0, 0);
            }
            Arrays.sort(measured);
            final double median =
                    measured.length % 2 == 1
                            ? measured[measured.length / 2]
                            : (measured[measured.length / 2 - 1] + measured[measured.length / 2]) / 2.0;
            final double mean = Arrays.stream(measured).average().orElse(0);
            final double min = measured[0];
            final double max = measured[measured.length - 1];
            final double var =
                    Arrays.stream(measured).map(x -> {
                        final double d = x - mean;
                        return d * d;
                    }).average().orElse(0);
            final double stddev = Math.sqrt(var);
            final double cv = mean > 0 ? stddev / mean : 0;
            return new Summary(allOk, median, mean, min, max, stddev, cv);
        }
    }

    static final class Json
    {
        private Json()
        {
        }

        static String emit(final Config config, final List<Round> rounds, final Summary summary)
        {
            final StringBuilder sb = new StringBuilder();
            sb.append("{\n");
            field(sb, "impl", config.implLabel, true);
            field(sb, "language", "java", true);
            field(sb, "scenario", config.scenario.wire(), true);
            field(sb, "wait_strategy", config.wait.wire(), true);
            field(sb, "event_padding", "none", true);
            field(sb, "api_path", "lmax-bep", true);
            num(sb, "buffer_size", config.bufferSize, true);
            num(sb, "events_total", config.eventsTotal, true);
            num(sb, "batch_size", config.batchSize, true);
            num(sb, "warmup_rounds", config.warmupRounds, true);
            num(sb, "measured_rounds", config.measuredRounds, true);
            field(sb, "run_order", config.runOrder, true);
            field(sb, "pair_id", config.pairId, true);
            num(sb, "fork_index", config.forkIndex, true);
            field(sb, "harness_git_rev", config.harnessRev, true);
            field(sb, "implementation_rev", config.implementationRev, true);
            bool(sb, "harness_dirty", config.harnessDirty, true, 2);
            bool(sb, "implementation_dirty", config.implementationDirty, true, 2);
            field(sb, "git_rev", config.implementationRev, true);
            sb.append("  \"rounds\": [\n");
            for (int i = 0; i < rounds.size(); i++)
            {
                final Round r = rounds.get(i);
                sb.append("    {\n");
                num(sb, "index", r.index, true, 6);
                field(sb, "phase", r.phase, true, 6);
                num(sb, "elapsed_ns", r.elapsedNs, true, 6);
                num(sb, "events", r.events, true, 6);
                fnum(sb, "ops_per_sec", r.opsPerSec, true, 6);
                bool(sb, "checksum_valid", r.checksumValid, false, 6);
                sb.append(i + 1 == rounds.size() ? "    }\n" : "    },\n");
            }
            sb.append("  ],\n");
            sb.append("  \"summary\": {\n");
            bool(sb, "checksum_valid_all", summary.checksumValidAll, true, 4);
            fnum(sb, "median_ops_per_sec", summary.medianOpsPerSec, true, 4);
            fnum(sb, "mean_ops_per_sec", summary.meanOpsPerSec, true, 4);
            fnum(sb, "min_ops_per_sec", summary.minOpsPerSec, true, 4);
            fnum(sb, "max_ops_per_sec", summary.maxOpsPerSec, true, 4);
            fnum(sb, "stddev_ops_per_sec", summary.stddevOpsPerSec, true, 4);
            fnum(sb, "cv", summary.cv, false, 4);
            sb.append("  }\n");
            sb.append("}\n");
            return sb.toString();
        }

        private static void field(
                final StringBuilder sb, final String k, final String v, final boolean comma)
        {
            field(sb, k, v, comma, 2);
        }

        private static void field(
                final StringBuilder sb,
                final String k,
                final String v,
                final boolean comma,
                final int indent)
        {
            sb.append(" ".repeat(indent))
                    .append('"')
                    .append(k)
                    .append("\": \"")
                    .append(v)
                    .append('"')
                    .append(comma ? ",\n" : "\n");
        }

        private static void num(
                final StringBuilder sb, final String k, final long v, final boolean comma)
        {
            num(sb, k, v, comma, 2);
        }

        private static void num(
                final StringBuilder sb,
                final String k,
                final long v,
                final boolean comma,
                final int indent)
        {
            sb.append(" ".repeat(indent))
                    .append('"')
                    .append(k)
                    .append("\": ")
                    .append(v)
                    .append(comma ? ",\n" : "\n");
        }

        private static void fnum(
                final StringBuilder sb,
                final String k,
                final double v,
                final boolean comma,
                final int indent)
        {
            sb.append(" ".repeat(indent))
                    .append('"')
                    .append(k)
                    .append("\": ")
                    .append(String.format(Locale.US, "%.6f", v))
                    .append(comma ? ",\n" : "\n");
        }

        private static void bool(
                final StringBuilder sb,
                final String k,
                final boolean v,
                final boolean comma,
                final int indent)
        {
            sb.append(" ".repeat(indent))
                    .append('"')
                    .append(k)
                    .append("\": ")
                    .append(v)
                    .append(comma ? ",\n" : "\n");
        }
    }
}
