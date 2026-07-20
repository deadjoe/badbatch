package com.lmax.disruptor.headtohead;

import com.lmax.disruptor.BatchEventProcessor;
import com.lmax.disruptor.BatchEventProcessorBuilder;
import com.lmax.disruptor.BusySpinWaitStrategy;
import com.lmax.disruptor.EventHandler;
import com.lmax.disruptor.H2HDiagnostics;
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
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Locale;
import java.util.Map;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.concurrent.atomic.AtomicLong;
import java.util.concurrent.atomic.AtomicReference;

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
        H2HDiagnostics.configure(config.roundDiagnostics);
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
        final AffinityTracker affinity = new AffinityTracker(config);
        affinity.pinCurrent(0, "publisher/coordinator");
        affinity.verify("publisher/coordinator");
        final List<Round> rounds = switch (config.scenario)
        {
            case UNICAST -> runUnicast(config, affinity, false);
            case UNICAST_BATCH -> runUnicast(config, affinity, true);
            case MPSC_BATCH -> runMpscBatch(config, affinity);
            case PIPELINE -> runPipeline(config, affinity);
        };
        config.affinityVerifiedAll = affinity.verifiedAll();
        return rounds;
    }

    private static List<Round> runUnicast(
            final Config config, final AffinityTracker affinity, final boolean batch) throws Exception
    {
        final long expected = arithmeticChecksum(config.eventsTotal);
        final int totalRounds = config.warmupRounds + config.measuredRounds;
        final List<Round> rounds = new ArrayList<>(totalRounds);

        for (int i = 0; i < totalRounds; i++)
        {
            final AtomicInteger ready = new AtomicInteger();
            final AtomicLong processed = new AtomicLong();
            final AtomicLong checksum = new AtomicLong();
            final BatchRecorder batchRecorder =
                    config.roundDiagnostics ? new BatchRecorder("consumer") : null;
            final EventHandler<ComparisonEvent> handler = config.roundDiagnostics
                    ? new DiagnosticSummingHandler(
                            processed, checksum, ready, config.eventsTotal, affinity, 1,
                            "consumer", batchRecorder)
                    : new SummingHandler(
                            processed, checksum, ready, config.eventsTotal, affinity, 1,
                            "consumer");

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
            affinity.verify("unicast consumer");

            if (config.roundDiagnostics)
            {
                H2HDiagnostics.beginRound();
            }
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
            final H2HDiagnostics.Snapshot producerBackpressure =
                    config.roundDiagnostics ? H2HDiagnostics.snapshot() : null;

            processor.halt();
            shutdown(executor);
            final RoundDiagnostics diagnostics = config.roundDiagnostics
                    ? RoundDiagnostics.single(batchRecorder.snapshot(config.eventsTotal),
                            producerBackpressure)
                    : null;
            rounds.add(Round.of(
                    i + 1,
                    i < config.warmupRounds ? "warmup" : "measured",
                    elapsed,
                    config.eventsTotal,
                    checksum.get() == expected,
                    diagnostics));
        }
        return rounds;
    }

    private static List<Round> runMpscBatch(
            final Config config, final AffinityTracker affinity) throws Exception
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
            final BatchRecorder batchRecorder =
                    config.roundDiagnostics ? new BatchRecorder("consumer") : null;
            final EventHandler<ComparisonEvent> handler = config.roundDiagnostics
                    ? new DiagnosticSummingHandler(
                            processed, checksum, ready, config.eventsTotal, affinity, 3,
                            "consumer", batchRecorder)
                    : new SummingHandler(
                            processed, checksum, ready, config.eventsTotal, affinity, 3,
                            "consumer");

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
            affinity.verify("mpsc consumer");

            final CountDownLatch producersReady = new CountDownLatch(MPSC_PRODUCERS);
            final CountDownLatch startSignal = new CountDownLatch(1);
            final Thread[] producers = new Thread[MPSC_PRODUCERS];

            for (int id = 0; id < MPSC_PRODUCERS; id++)
            {
                final int index = id;
                producers[id] = new Thread(() -> {
                    affinity.pinCurrent(index, "producer_" + index);
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
            affinity.verify("mpsc producers");
            final long start = System.nanoTime();
            startSignal.countDown();
            for (final Thread producer : producers)
            {
                producer.join();
            }
            waitForCount(processed, config.eventsTotal, "mpsc completion");
            final long elapsed = System.nanoTime() - start;

            processor.halt();
            shutdown(executor);
            final RoundDiagnostics diagnostics = config.roundDiagnostics
                    ? RoundDiagnostics.multiProducer(batchRecorder.snapshot(config.eventsTotal))
                    : null;
            rounds.add(Round.of(
                    i + 1,
                    i < config.warmupRounds ? "warmup" : "measured",
                    elapsed,
                    config.eventsTotal,
                    checksum.get() == expected,
                    diagnostics));
        }
        return rounds;
    }

    private static List<Round> runPipeline(
            final Config config, final AffinityTracker affinity) throws Exception
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

            final BatchRecorder stage1Recorder =
                    config.roundDiagnostics ? new BatchRecorder("stage_1") : null;
            final EventHandler<ComparisonEvent> stage1 = config.roundDiagnostics
                    ? new DiagnosticStage1Handler(ready, affinity, 1, stage1Recorder)
                    : new Stage1Handler(ready, affinity, 1);
            final BatchEventProcessor<ComparisonEvent> p1 =
                    new BatchEventProcessorBuilder().build(ring, ring.newBarrier(), stage1);

            final BatchRecorder stage2Recorder =
                    config.roundDiagnostics ? new BatchRecorder("stage_2") : null;
            final EventHandler<ComparisonEvent> stage2 = config.roundDiagnostics
                    ? new DiagnosticStage2Handler(ready, affinity, 2, stage2Recorder)
                    : new Stage2Handler(ready, affinity, 2);
            final BatchEventProcessor<ComparisonEvent> p2 =
                    new BatchEventProcessorBuilder()
                            .build(ring, ring.newBarrier(p1.getSequence()), stage2);

            final BatchRecorder stage3Recorder =
                    config.roundDiagnostics ? new BatchRecorder("stage_3") : null;
            final EventHandler<ComparisonEvent> stage3 = config.roundDiagnostics
                    ? new DiagnosticStage3Handler(
                            processed, checksum, ready, config.eventsTotal, affinity, 3,
                            stage3Recorder)
                    : new Stage3Handler(
                            processed, checksum, ready, config.eventsTotal, affinity, 3);
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
            affinity.verify("pipeline stages");

            if (config.roundDiagnostics)
            {
                H2HDiagnostics.beginRound();
            }
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
            final H2HDiagnostics.Snapshot producerBackpressure =
                    config.roundDiagnostics ? H2HDiagnostics.snapshot() : null;

            p1.halt();
            p2.halt();
            p3.halt();
            shutdown(executor);
            final RoundDiagnostics diagnostics = config.roundDiagnostics
                    ? new RoundDiagnostics(
                            List.of(
                                    stage1Recorder.snapshot(config.eventsTotal),
                                    stage2Recorder.snapshot(config.eventsTotal),
                                    stage3Recorder.snapshot(config.eventsTotal)),
                            producerBackpressure,
                            true)
                    : null;
            rounds.add(Round.of(
                    i + 1,
                    i < config.warmupRounds ? "warmup" : "measured",
                    elapsed,
                    config.eventsTotal,
                    checksum.get() == expected,
                    diagnostics));
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
                final String output = new String(taskset.getInputStream().readAllBytes(),
                        StandardCharsets.UTF_8).trim();
                final int status = taskset.waitFor();
                final String observed = currentAllowedCpuList();
                if (status != 0 || !Integer.toString(cpu).equals(observed))
                {
                    throw new IllegalStateException(
                            "taskset status=" + status + " observed=" + observed + " output=" + output);
                }
            }
            catch (final Exception error)
            {
                failure.compareAndSet(null,
                        role + "->CPU" + cpu + " failed: " + error.getMessage());
            }
        }

        void verify(final String phase)
        {
            final String message = failure.get();
            if (message != null)
            {
                throw new IllegalStateException("CPU affinity failed before " + phase + ": " + message);
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
            throw new IllegalStateException("Cpus_allowed_list missing from /proc/thread-self/status");
        }
    }

    // --- handlers ------------------------------------------------------------------

    static final class ComparisonEvent
    {
        long value;
        long stage1Value;
        long stage2Value;
        long stage3Value;
    }

    private static final class Log2Histogram
    {
        private static final int BINS = 64;
        private long count;
        private long sum;
        private long min = Long.MAX_VALUE;
        private long max;
        private final long[] bins = new long[BINS];

        void observe(final long value)
        {
            if (value <= 0L)
            {
                throw new IllegalArgumentException("histogram values must be positive");
            }
            final int bin = 63 - Long.numberOfLeadingZeros(value);
            count++;
            sum += value;
            min = Math.min(min, value);
            max = Math.max(max, value);
            bins[bin]++;
        }

        HistogramSnapshot snapshot()
        {
            return new HistogramSnapshot(
                    count,
                    sum,
                    count == 0L ? 0L : min,
                    max,
                    bins.clone());
        }
    }

    private record HistogramSnapshot(long count, long sum, long min, long max, long[] log2Bins)
    {
    }

    private record ConsumerBatchDiagnostics(
            String role, HistogramSnapshot batchSize, HistogramSnapshot queueDepth)
    {
    }

    private static final class BatchRecorder
    {
        private final String role;
        private final Log2Histogram batchSize = new Log2Histogram();
        private final Log2Histogram queueDepth = new Log2Histogram();

        BatchRecorder(final String role)
        {
            this.role = role;
        }

        void observe(final long size, final long depth)
        {
            batchSize.observe(size);
            queueDepth.observe(depth);
        }

        ConsumerBatchDiagnostics snapshot(final long expectedEvents)
        {
            final HistogramSnapshot sizes = batchSize.snapshot();
            if (sizes.sum() != expectedEvents)
            {
                throw new IllegalStateException(
                        role + " batch diagnostics saw " + sizes.sum()
                                + " events, expected " + expectedEvents);
            }
            return new ConsumerBatchDiagnostics(role, sizes, queueDepth.snapshot());
        }
    }

    private record RoundDiagnostics(
            List<ConsumerBatchDiagnostics> consumers,
            H2HDiagnostics.Snapshot producerBackpressure,
            boolean producerSupported)
    {
        static RoundDiagnostics single(
                final ConsumerBatchDiagnostics consumer,
                final H2HDiagnostics.Snapshot producerBackpressure)
        {
            return new RoundDiagnostics(List.of(consumer), producerBackpressure, true);
        }

        static RoundDiagnostics multiProducer(final ConsumerBatchDiagnostics consumer)
        {
            return new RoundDiagnostics(List.of(consumer), null, false);
        }
    }

    private static final class SummingHandler implements EventHandler<ComparisonEvent>
    {
        private final AtomicLong processed;
        private final AtomicLong checksum;
        private final AtomicInteger ready;
        private final long eventsTotal;
        private final long finalSequence;
        private final AffinityTracker affinity;
        private final int cpuIndex;
        private final String role;
        private long localChecksum;

        SummingHandler(
                final AtomicLong processed,
                final AtomicLong checksum,
                final AtomicInteger ready,
                final long eventsTotal,
                final AffinityTracker affinity,
                final int cpuIndex,
                final String role)
        {
            this.processed = processed;
            this.checksum = checksum;
            this.ready = ready;
            this.eventsTotal = eventsTotal;
            this.finalSequence = eventsTotal - 1L;
            this.affinity = affinity;
            this.cpuIndex = cpuIndex;
            this.role = role;
        }

        @Override
        public void onStart()
        {
            affinity.pinCurrent(cpuIndex, role);
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
        private final AffinityTracker affinity;
        private final int cpuIndex;

        Stage1Handler(final AtomicInteger ready, final AffinityTracker affinity, final int cpuIndex)
        {
            this.ready = ready;
            this.affinity = affinity;
            this.cpuIndex = cpuIndex;
        }

        @Override
        public void onStart()
        {
            affinity.pinCurrent(cpuIndex, "stage_1");
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
        private final AffinityTracker affinity;
        private final int cpuIndex;

        Stage2Handler(final AtomicInteger ready, final AffinityTracker affinity, final int cpuIndex)
        {
            this.ready = ready;
            this.affinity = affinity;
            this.cpuIndex = cpuIndex;
        }

        @Override
        public void onStart()
        {
            affinity.pinCurrent(cpuIndex, "stage_2");
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
        private final AffinityTracker affinity;
        private final int cpuIndex;
        private long localChecksum;

        Stage3Handler(
                final AtomicLong processed,
                final AtomicLong checksum,
                final AtomicInteger ready,
                final long eventsTotal,
                final AffinityTracker affinity,
                final int cpuIndex)
        {
            this.processed = processed;
            this.checksum = checksum;
            this.ready = ready;
            this.eventsTotal = eventsTotal;
            this.finalSequence = eventsTotal - 1L;
            this.affinity = affinity;
            this.cpuIndex = cpuIndex;
        }

        @Override
        public void onStart()
        {
            affinity.pinCurrent(cpuIndex, "stage_3");
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

    // Diagnostic variants duplicate the tiny hot-path handlers so there is no
    // per-event delegate call. Only onBatchStart carries probe work.
    private static final class DiagnosticSummingHandler implements EventHandler<ComparisonEvent>
    {
        private final AtomicLong processed;
        private final AtomicLong checksum;
        private final AtomicInteger ready;
        private final long eventsTotal;
        private final long finalSequence;
        private final AffinityTracker affinity;
        private final int cpuIndex;
        private final String role;
        private final BatchRecorder recorder;
        private long localChecksum;

        DiagnosticSummingHandler(
                final AtomicLong processed,
                final AtomicLong checksum,
                final AtomicInteger ready,
                final long eventsTotal,
                final AffinityTracker affinity,
                final int cpuIndex,
                final String role,
                final BatchRecorder recorder)
        {
            this.processed = processed;
            this.checksum = checksum;
            this.ready = ready;
            this.eventsTotal = eventsTotal;
            this.finalSequence = eventsTotal - 1L;
            this.affinity = affinity;
            this.cpuIndex = cpuIndex;
            this.role = role;
            this.recorder = recorder;
        }

        @Override
        public void onBatchStart(final long batchSize, final long queueDepth)
        {
            recorder.observe(batchSize, queueDepth);
        }

        @Override
        public void onStart()
        {
            affinity.pinCurrent(cpuIndex, role);
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

    private static final class DiagnosticStage1Handler implements EventHandler<ComparisonEvent>
    {
        private final AtomicInteger ready;
        private final AffinityTracker affinity;
        private final int cpuIndex;
        private final BatchRecorder recorder;

        DiagnosticStage1Handler(
                final AtomicInteger ready,
                final AffinityTracker affinity,
                final int cpuIndex,
                final BatchRecorder recorder)
        {
            this.ready = ready;
            this.affinity = affinity;
            this.cpuIndex = cpuIndex;
            this.recorder = recorder;
        }

        @Override
        public void onBatchStart(final long batchSize, final long queueDepth)
        {
            recorder.observe(batchSize, queueDepth);
        }

        @Override
        public void onStart()
        {
            affinity.pinCurrent(cpuIndex, "stage_1");
            ready.incrementAndGet();
        }

        @Override
        public void onEvent(final ComparisonEvent event, final long sequence, final boolean endOfBatch)
        {
            event.stage1Value = event.value + 1L;
        }
    }

    private static final class DiagnosticStage2Handler implements EventHandler<ComparisonEvent>
    {
        private final AtomicInteger ready;
        private final AffinityTracker affinity;
        private final int cpuIndex;
        private final BatchRecorder recorder;

        DiagnosticStage2Handler(
                final AtomicInteger ready,
                final AffinityTracker affinity,
                final int cpuIndex,
                final BatchRecorder recorder)
        {
            this.ready = ready;
            this.affinity = affinity;
            this.cpuIndex = cpuIndex;
            this.recorder = recorder;
        }

        @Override
        public void onBatchStart(final long batchSize, final long queueDepth)
        {
            recorder.observe(batchSize, queueDepth);
        }

        @Override
        public void onStart()
        {
            affinity.pinCurrent(cpuIndex, "stage_2");
            ready.incrementAndGet();
        }

        @Override
        public void onEvent(final ComparisonEvent event, final long sequence, final boolean endOfBatch)
        {
            event.stage2Value = event.stage1Value + 3L;
        }
    }

    private static final class DiagnosticStage3Handler implements EventHandler<ComparisonEvent>
    {
        private final AtomicLong processed;
        private final AtomicLong checksum;
        private final AtomicInteger ready;
        private final long eventsTotal;
        private final long finalSequence;
        private final AffinityTracker affinity;
        private final int cpuIndex;
        private final BatchRecorder recorder;
        private long localChecksum;

        DiagnosticStage3Handler(
                final AtomicLong processed,
                final AtomicLong checksum,
                final AtomicInteger ready,
                final long eventsTotal,
                final AffinityTracker affinity,
                final int cpuIndex,
                final BatchRecorder recorder)
        {
            this.processed = processed;
            this.checksum = checksum;
            this.ready = ready;
            this.eventsTotal = eventsTotal;
            this.finalSequence = eventsTotal - 1L;
            this.affinity = affinity;
            this.cpuIndex = cpuIndex;
            this.recorder = recorder;
        }

        @Override
        public void onBatchStart(final long batchSize, final long queueDepth)
        {
            recorder.observe(batchSize, queueDepth);
        }

        @Override
        public void onStart()
        {
            affinity.pinCurrent(cpuIndex, "stage_3");
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
        boolean roundDiagnostics;
        List<Integer> cpuList = List.of();
        boolean affinityVerifiedAll;

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
                    case "--cpu-list" -> c.cpuList = parseCpuList(args[++i]);
                    case "--round-diagnostics" -> c.roundDiagnostics = true;
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
            final int requiredCpus =
                    (c.scenario == Scenario.MPSC_BATCH || c.scenario == Scenario.PIPELINE) ? 4 : 2;
            if (!c.cpuList.isEmpty() && c.cpuList.size() < requiredCpus)
            {
                throw new IllegalArgumentException(
                        c.scenario.wire() + " requires at least " + requiredCpus + " CPUs");
            }
            return c;
        }

        Map<String, Integer> affinityRoles()
        {
            final Map<String, Integer> roles = new LinkedHashMap<>();
            if (cpuList.isEmpty())
            {
                return roles;
            }
            switch (scenario)
            {
                case UNICAST, UNICAST_BATCH ->
                {
                    roles.put("publisher", cpuList.get(0));
                    roles.put("consumer", cpuList.get(1));
                }
                case MPSC_BATCH ->
                {
                    roles.put("coordinator", cpuList.get(0));
                    roles.put("producer_0", cpuList.get(0));
                    roles.put("producer_1", cpuList.get(1));
                    roles.put("producer_2", cpuList.get(2));
                    roles.put("consumer", cpuList.get(3));
                }
                case PIPELINE ->
                {
                    roles.put("publisher", cpuList.get(0));
                    roles.put("stage_1", cpuList.get(1));
                    roles.put("stage_2", cpuList.get(2));
                    roles.put("stage_3", cpuList.get(3));
                }
            }
            return roles;
        }

        private static List<Integer> parseCpuList(final String value)
        {
            if (value.isEmpty())
            {
                return List.of();
            }
            final List<Integer> cpus = Arrays.stream(value.split(","))
                    .map(Integer::parseInt)
                    .toList();
            if (cpus.isEmpty() || cpus.stream().anyMatch(cpu -> cpu < 0)
                    || cpus.stream().distinct().count() != cpus.size())
            {
                throw new IllegalArgumentException(
                        "cpu-list must contain unique non-negative CPU IDs");
            }
            return cpus;
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
        final RoundDiagnostics diagnostics;

        private Round(
                final int index,
                final String phase,
                final long elapsedNs,
                final long events,
                final double opsPerSec,
                final boolean checksumValid,
                final RoundDiagnostics diagnostics)
        {
            this.index = index;
            this.phase = phase;
            this.elapsedNs = elapsedNs;
            this.events = events;
            this.opsPerSec = opsPerSec;
            this.checksumValid = checksumValid;
            this.diagnostics = diagnostics;
        }

        static Round of(
                final int index,
                final String phase,
                final long elapsedNs,
                final long events,
                final boolean checksumValid,
                final RoundDiagnostics diagnostics)
        {
            final double ops = events / (elapsedNs / 1_000_000_000.0);
            return new Round(index, phase, elapsedNs, events, ops, checksumValid, diagnostics);
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
            if (config.roundDiagnostics)
            {
                bool(sb, "round_diagnostics", true, true, 2);
            }
            field(sb, "run_order", config.runOrder, true);
            field(sb, "pair_id", config.pairId, true);
            num(sb, "fork_index", config.forkIndex, true);
            field(sb, "harness_git_rev", config.harnessRev, true);
            field(sb, "implementation_rev", config.implementationRev, true);
            bool(sb, "harness_dirty", config.harnessDirty, true, 2);
            bool(sb, "implementation_dirty", config.implementationDirty, true, 2);
            field(sb, "git_rev", config.implementationRev, true);
            sb.append("  \"cpu_affinity\": {\n");
            sb.append("    \"requested_cpu_list\": [");
            for (int i = 0; i < config.cpuList.size(); i++)
            {
                if (i > 0)
                {
                    sb.append(", ");
                }
                sb.append(config.cpuList.get(i));
            }
            sb.append("],\n");
            field(sb, "mode", config.cpuList.isEmpty() ? "none" : "per-thread", true, 4);
            bool(sb, "verified_all", config.affinityVerifiedAll, true, 4);
            sb.append("    \"role_cpu_map\": {");
            final List<Map.Entry<String, Integer>> roles =
                    new ArrayList<>(config.affinityRoles().entrySet());
            if (!roles.isEmpty())
            {
                sb.append('\n');
                for (int i = 0; i < roles.size(); i++)
                {
                    final Map.Entry<String, Integer> role = roles.get(i);
                    num(sb, role.getKey(), role.getValue(), i + 1 < roles.size(), 6);
                }
                sb.append("    }\n");
            }
            else
            {
                sb.append("}\n");
            }
            sb.append("  },\n");
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
                bool(sb, "checksum_valid", r.checksumValid, r.diagnostics != null, 6);
                if (r.diagnostics != null)
                {
                    diagnostics(sb, r.diagnostics);
                }
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

        private static void diagnostics(
                final StringBuilder sb, final RoundDiagnostics diagnostics)
        {
            sb.append("      \"diagnostics\": {\n");
            sb.append("        \"batch_processing\": [\n");
            for (int i = 0; i < diagnostics.consumers().size(); i++)
            {
                final ConsumerBatchDiagnostics consumer = diagnostics.consumers().get(i);
                sb.append("          {\n");
                field(sb, "role", consumer.role(), true, 12);
                sb.append("            \"batch_size\": ");
                histogram(sb, consumer.batchSize(), 12);
                sb.append(",\n");
                sb.append("            \"queue_depth\": ");
                histogram(sb, consumer.queueDepth(), 12);
                sb.append('\n');
                sb.append(i + 1 == diagnostics.consumers().size()
                        ? "          }\n"
                        : "          },\n");
            }
            sb.append("        ],\n");
            sb.append("        \"producer_backpressure\": {\n");
            bool(sb, "supported", diagnostics.producerSupported(), true, 10);
            field(
                    sb,
                    "iteration_action",
                    diagnostics.producerSupported()
                            ? "park_nanos_1_request"
                            : "unsupported_multi_producer",
                    true,
                    10);
            final H2HDiagnostics.Snapshot producer = diagnostics.producerBackpressure();
            num(sb, "entries", producer == null ? 0L : producer.entries(), true, 10);
            num(
                    sb,
                    "wait_loop_iterations",
                    producer == null ? 0L : producer.waitLoopIterations(),
                    true,
                    10);
            num(
                    sb,
                    "max_wait_loop_iterations",
                    producer == null ? 0L : producer.maxWaitLoopIterations(),
                    false,
                    10);
            sb.append("        }\n");
            sb.append("      }\n");
        }

        private static void histogram(
                final StringBuilder sb, final HistogramSnapshot histogram, final int indent)
        {
            sb.append("{\n");
            num(sb, "count", histogram.count(), true, indent + 2);
            num(sb, "sum", histogram.sum(), true, indent + 2);
            num(sb, "min", histogram.min(), true, indent + 2);
            num(sb, "max", histogram.max(), true, indent + 2);
            fnum(
                    sb,
                    "mean",
                    histogram.count() == 0L
                            ? 0.0
                            : (double) histogram.sum() / histogram.count(),
                    true,
                    indent + 2);
            sb.append(" ".repeat(indent + 2)).append("\"log2_bins\": [");
            for (int i = 0; i < histogram.log2Bins().length; i++)
            {
                if (i > 0)
                {
                    sb.append(", ");
                }
                sb.append(histogram.log2Bins()[i]);
            }
            sb.append("]\n");
            sb.append(" ".repeat(indent)).append('}');
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
