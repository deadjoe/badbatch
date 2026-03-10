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

import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.time.Duration;
import java.util.ArrayList;
import java.util.Comparator;
import java.util.List;
import java.util.Locale;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.concurrent.atomic.AtomicLong;
import java.util.stream.Collectors;

public final class HeadToHead
{
    private static final int MPSC_PRODUCER_COUNT = 3;
    private static final Duration DEFAULT_TIMEOUT = Duration.ofSeconds(300);

    private HeadToHead()
    {
    }

    public static void main(final String[] args) throws Exception
    {
        final CliConfig cliConfig = CliConfig.fromArgs(args);
        final ResolvedConfig config = cliConfig.resolve();
        final HarnessResult result = runScenario(config);
        final String json = result.toJson();

        if (config.outputPath != null)
        {
            writeOutput(config.outputPath, json);
        }

        System.out.println(json);

        if (!result.summary.checksumValidAll)
        {
            System.exit(2);
        }
    }

    private static HarnessResult runScenario(final ResolvedConfig config) throws Exception
    {
        switch (config.scenario)
        {
            case UNICAST:
                return runUnicast(config);
            case UNICAST_BATCH:
                return runUnicastBatch(config);
            case MPSC_BATCH:
                return runMpscBatch(config);
            case PIPELINE:
                return runPipeline(config);
            default:
                throw new IllegalStateException("Unsupported scenario: " + config.scenario);
        }
    }

    private static HarnessResult runUnicast(final ResolvedConfig config) throws Exception
    {
        final long expectedChecksum = arithmeticChecksum(config.eventsTotal);
        final List<RoundRecord> runs = new ArrayList<>(config.totalRounds());

        for (int round = 0; round < config.totalRounds(); round++)
        {
            final AtomicInteger readyCount = new AtomicInteger();
            final AtomicLong processed = new AtomicLong();
            final AtomicLong checksum = new AtomicLong();
            final SummingHandler handler = new SummingHandler(processed, checksum, readyCount);

            final RingBuffer<ComparisonEvent> ringBuffer =
                    RingBuffer.createSingleProducer(
                            ComparisonEvent::new,
                            config.bufferSize,
                            createWaitStrategy(config.waitStrategy));
            final SequenceBarrier barrier = ringBuffer.newBarrier();
            final BatchEventProcessor<ComparisonEvent> processor =
                    new BatchEventProcessorBuilder().build(ringBuffer, barrier, handler);
            ringBuffer.addGatingSequences(processor.getSequence());

            final ExecutorService executor = Executors.newSingleThreadExecutor(DaemonThreadFactory.INSTANCE);
            executor.submit(processor);
            waitForCount(readyCount, 1, "unicast consumer");

            final long start = System.nanoTime();
            for (long value = 0; value < config.eventsTotal; value++)
            {
                final long sequence = ringBuffer.next();
                final ComparisonEvent event = ringBuffer.get(sequence);
                event.value = value;
                event.stage1Value = 0L;
                event.stage2Value = 0L;
                event.stage3Value = 0L;
                ringBuffer.publish(sequence);
            }
            waitForCount(processed, config.eventsTotal, "unicast completion");
            final long elapsed = System.nanoTime() - start;

            runs.add(RoundRecord.from(
                    round + 1,
                    phaseForRound(round, config.warmupRounds),
                    elapsed,
                    config.eventsTotal,
                    processed.get(),
                    expectedChecksum,
                    checksum.get()));

            haltProcessor(processor, executor);
        }

        return HarnessResult.from(config, runs);
    }

    private static HarnessResult runUnicastBatch(final ResolvedConfig config) throws Exception
    {
        final long expectedChecksum = arithmeticChecksum(config.eventsTotal);
        final List<RoundRecord> runs = new ArrayList<>(config.totalRounds());

        for (int round = 0; round < config.totalRounds(); round++)
        {
            final AtomicInteger readyCount = new AtomicInteger();
            final AtomicLong processed = new AtomicLong();
            final AtomicLong checksum = new AtomicLong();
            final SummingHandler handler = new SummingHandler(processed, checksum, readyCount);

            final RingBuffer<ComparisonEvent> ringBuffer =
                    RingBuffer.createSingleProducer(
                            ComparisonEvent::new,
                            config.bufferSize,
                            createWaitStrategy(config.waitStrategy));
            final SequenceBarrier barrier = ringBuffer.newBarrier();
            final BatchEventProcessor<ComparisonEvent> processor =
                    new BatchEventProcessorBuilder().build(ringBuffer, barrier, handler);
            ringBuffer.addGatingSequences(processor.getSequence());

            final ExecutorService executor = Executors.newSingleThreadExecutor(DaemonThreadFactory.INSTANCE);
            executor.submit(processor);
            waitForCount(readyCount, 1, "unicast batch consumer");

            final long start = System.nanoTime();
            long published = 0L;
            while (published < config.eventsTotal)
            {
                final int chunk = (int) Math.min(config.batchSize, config.eventsTotal - published);
                final long hi = ringBuffer.next(chunk);
                final long lo = hi - (chunk - 1L);
                for (int index = 0; index < chunk; index++)
                {
                    final long value = published + index;
                    final ComparisonEvent event = ringBuffer.get(lo + index);
                    event.value = value;
                    event.stage1Value = 0L;
                    event.stage2Value = 0L;
                    event.stage3Value = 0L;
                }
                ringBuffer.publish(lo, hi);
                published += chunk;
            }
            waitForCount(processed, config.eventsTotal, "unicast batch completion");
            final long elapsed = System.nanoTime() - start;

            runs.add(RoundRecord.from(
                    round + 1,
                    phaseForRound(round, config.warmupRounds),
                    elapsed,
                    config.eventsTotal,
                    processed.get(),
                    expectedChecksum,
                    checksum.get()));

            haltProcessor(processor, executor);
        }

        return HarnessResult.from(config, runs);
    }

    private static HarnessResult runMpscBatch(final ResolvedConfig config) throws Exception
    {
        final long expectedChecksum = arithmeticChecksum(config.eventsTotal);
        final long eventsPerProducer = config.eventsTotal / MPSC_PRODUCER_COUNT;
        final List<RoundRecord> runs = new ArrayList<>(config.totalRounds());

        for (int round = 0; round < config.totalRounds(); round++)
        {
            final AtomicInteger readyCount = new AtomicInteger();
            final AtomicLong processed = new AtomicLong();
            final AtomicLong checksum = new AtomicLong();
            final SummingHandler handler = new SummingHandler(processed, checksum, readyCount);

            final RingBuffer<ComparisonEvent> ringBuffer =
                    RingBuffer.createMultiProducer(
                            ComparisonEvent::new,
                            config.bufferSize,
                            createWaitStrategy(config.waitStrategy));
            final SequenceBarrier barrier = ringBuffer.newBarrier();
            final BatchEventProcessor<ComparisonEvent> processor =
                    new BatchEventProcessorBuilder().build(ringBuffer, barrier, handler);
            ringBuffer.addGatingSequences(processor.getSequence());

            final ExecutorService executor = Executors.newSingleThreadExecutor(DaemonThreadFactory.INSTANCE);
            executor.submit(processor);
            waitForCount(readyCount, 1, "mpsc consumer");

            final CountDownLatch producersReady = new CountDownLatch(MPSC_PRODUCER_COUNT);
            final CountDownLatch startSignal = new CountDownLatch(1);
            final Thread[] producers = new Thread[MPSC_PRODUCER_COUNT];

            for (int producerId = 0; producerId < MPSC_PRODUCER_COUNT; producerId++)
            {
                final int index = producerId;
                producers[producerId] = new Thread(() -> {
                    producersReady.countDown();
                    await(startSignal);

                    final long producerStart = eventsPerProducer * index;
                    final long producerEnd = producerStart + eventsPerProducer;
                    long published = producerStart;

                    while (published < producerEnd)
                    {
                        final int chunk = (int) Math.min(config.batchSize, producerEnd - published);
                        final long hi = ringBuffer.next(chunk);
                        final long lo = hi - (chunk - 1L);
                        for (int offset = 0; offset < chunk; offset++)
                        {
                            final long value = published + offset;
                            final ComparisonEvent event = ringBuffer.get(lo + offset);
                            event.value = value;
                            event.stage1Value = 0L;
                            event.stage2Value = 0L;
                            event.stage3Value = 0L;
                        }
                        ringBuffer.publish(lo, hi);
                        published += chunk;
                    }
                }, "head-to-head-mpsc-producer-" + producerId);
                producers[producerId].start();
            }

            producersReady.await();

            final long start = System.nanoTime();
            startSignal.countDown();
            for (Thread producer : producers)
            {
                producer.join();
            }
            waitForCount(processed, config.eventsTotal, "mpsc completion");
            final long elapsed = System.nanoTime() - start;

            runs.add(RoundRecord.from(
                    round + 1,
                    phaseForRound(round, config.warmupRounds),
                    elapsed,
                    config.eventsTotal,
                    processed.get(),
                    expectedChecksum,
                    checksum.get()));

            haltProcessor(processor, executor);
        }

        return HarnessResult.from(config, runs);
    }

    private static HarnessResult runPipeline(final ResolvedConfig config) throws Exception
    {
        final long expectedChecksum = pipelineChecksum(config.eventsTotal);
        final List<RoundRecord> runs = new ArrayList<>(config.totalRounds());

        for (int round = 0; round < config.totalRounds(); round++)
        {
            final AtomicInteger readyCount = new AtomicInteger();
            final AtomicLong processed = new AtomicLong();
            final AtomicLong checksum = new AtomicLong();

            final RingBuffer<ComparisonEvent> ringBuffer =
                    RingBuffer.createSingleProducer(
                            ComparisonEvent::new,
                            config.bufferSize,
                            createWaitStrategy(config.waitStrategy));

            final Stage1Handler stage1Handler = new Stage1Handler(readyCount);
            final SequenceBarrier stage1Barrier = ringBuffer.newBarrier();
            final BatchEventProcessor<ComparisonEvent> stage1Processor =
                    new BatchEventProcessorBuilder().build(ringBuffer, stage1Barrier, stage1Handler);

            final Stage2Handler stage2Handler = new Stage2Handler(readyCount);
            final SequenceBarrier stage2Barrier = ringBuffer.newBarrier(stage1Processor.getSequence());
            final BatchEventProcessor<ComparisonEvent> stage2Processor =
                    new BatchEventProcessorBuilder().build(ringBuffer, stage2Barrier, stage2Handler);

            final Stage3Handler stage3Handler = new Stage3Handler(processed, checksum, readyCount);
            final SequenceBarrier stage3Barrier = ringBuffer.newBarrier(stage2Processor.getSequence());
            final BatchEventProcessor<ComparisonEvent> stage3Processor =
                    new BatchEventProcessorBuilder().build(ringBuffer, stage3Barrier, stage3Handler);

            ringBuffer.addGatingSequences(stage3Processor.getSequence());

            final ExecutorService executor = Executors.newFixedThreadPool(3, DaemonThreadFactory.INSTANCE);
            executor.submit(stage1Processor);
            executor.submit(stage2Processor);
            executor.submit(stage3Processor);

            waitForCount(readyCount, 3, "pipeline stages");

            final long start = System.nanoTime();
            for (long value = 0; value < config.eventsTotal; value++)
            {
                final long sequence = ringBuffer.next();
                final ComparisonEvent event = ringBuffer.get(sequence);
                event.value = value;
                event.stage1Value = 0L;
                event.stage2Value = 0L;
                event.stage3Value = 0L;
                ringBuffer.publish(sequence);
            }
            waitForCount(processed, config.eventsTotal, "pipeline completion");
            final long elapsed = System.nanoTime() - start;

            runs.add(RoundRecord.from(
                    round + 1,
                    phaseForRound(round, config.warmupRounds),
                    elapsed,
                    config.eventsTotal,
                    processed.get(),
                    expectedChecksum,
                    checksum.get()));

            stage1Processor.halt();
            stage2Processor.halt();
            stage3Processor.halt();
            haltExecutor(executor);
        }

        return HarnessResult.from(config, runs);
    }

    private static void haltProcessor(final BatchEventProcessor<ComparisonEvent> processor, final ExecutorService executor)
            throws InterruptedException
    {
        processor.halt();
        haltExecutor(executor);
    }

    private static void haltExecutor(final ExecutorService executor) throws InterruptedException
    {
        executor.shutdownNow();
        executor.awaitTermination(5, TimeUnit.SECONDS);
    }

    private static RoundPhase phaseForRound(final int roundIndex, final int warmupRounds)
    {
        return roundIndex < warmupRounds ? RoundPhase.WARMUP : RoundPhase.MEASURED;
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
                        "Timed out waiting for " + label + ": expected >= " + expected + ", got " + counter.get());
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
                        "Timed out waiting for " + label + ": expected >= " + expected + ", got " + counter.get());
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
        catch (InterruptedException e)
        {
            Thread.currentThread().interrupt();
            throw new RuntimeException(e);
        }
    }

    private static WaitStrategy createWaitStrategy(final WaitStrategyKind kind)
    {
        switch (kind)
        {
            case BUSY_SPIN:
                return new BusySpinWaitStrategy();
            case YIELDING:
                return new YieldingWaitStrategy();
            default:
                throw new IllegalStateException("Unsupported wait strategy: " + kind);
        }
    }

    private static long arithmeticChecksum(final long count)
    {
        return (count * (count - 1L)) / 2L;
    }

    private static long pipelineChecksum(final long count)
    {
        return arithmeticChecksum(count) + (11L * count);
    }

    private static double opsPerSecond(final long eventsTotal, final long elapsedNs)
    {
        if (elapsedNs == 0L)
        {
            return 0.0;
        }
        return eventsTotal / (elapsedNs / 1_000_000_000.0);
    }

    private static void writeOutput(final Path outputPath, final String json) throws IOException
    {
        final Path parent = outputPath.getParent();
        if (parent != null)
        {
            Files.createDirectories(parent);
        }
        Files.writeString(outputPath, json, StandardCharsets.UTF_8);
    }

    enum Scenario
    {
        UNICAST("unicast", WaitStrategyKind.YIELDING, 65_536, 100_000_000L, 1, 1, 1, 1),
        UNICAST_BATCH("unicast_batch", WaitStrategyKind.YIELDING, 65_536, 100_000_000L, 10, 1, 1, 1),
        MPSC_BATCH("mpsc_batch", WaitStrategyKind.BUSY_SPIN, 65_536, 60_000_000L, 10, MPSC_PRODUCER_COUNT, 1, 1),
        PIPELINE("pipeline", WaitStrategyKind.YIELDING, 8_192, 100_000_000L, 1, 1, 3, 3);

        final String id;
        final WaitStrategyKind defaultWaitStrategy;
        final int defaultBufferSize;
        final long defaultEventsTotal;
        final int defaultBatchSize;
        final int producerCount;
        final int consumerCount;
        final int pipelineStages;

        Scenario(
                final String id,
                final WaitStrategyKind defaultWaitStrategy,
                final int defaultBufferSize,
                final long defaultEventsTotal,
                final int defaultBatchSize,
                final int producerCount,
                final int consumerCount,
                final int pipelineStages)
        {
            this.id = id;
            this.defaultWaitStrategy = defaultWaitStrategy;
            this.defaultBufferSize = defaultBufferSize;
            this.defaultEventsTotal = defaultEventsTotal;
            this.defaultBatchSize = defaultBatchSize;
            this.producerCount = producerCount;
            this.consumerCount = consumerCount;
            this.pipelineStages = pipelineStages;
        }

        static Scenario parse(final String value)
        {
            for (Scenario scenario : values())
            {
                if (scenario.id.equals(value))
                {
                    return scenario;
                }
            }
            throw new IllegalArgumentException("Unsupported scenario: " + value);
        }
    }

    enum WaitStrategyKind
    {
        BUSY_SPIN("busy-spin"),
        YIELDING("yielding");

        final String id;

        WaitStrategyKind(final String id)
        {
            this.id = id;
        }

        static WaitStrategyKind parse(final String value)
        {
            for (WaitStrategyKind kind : values())
            {
                if (kind.id.equals(value))
                {
                    return kind;
                }
            }
            throw new IllegalArgumentException("Unsupported wait strategy: " + value);
        }
    }

    enum RoundPhase
    {
        WARMUP("warmup"),
        MEASURED("measured");

        final String id;

        RoundPhase(final String id)
        {
            this.id = id;
        }
    }

    static final class CliConfig
    {
        private Scenario scenario;
        private WaitStrategyKind waitStrategy;
        private Integer bufferSize;
        private Long eventsTotal;
        private Integer batchSize;
        private int warmupRounds = 3;
        private int measuredRounds = 7;
        private String runOrder = "standalone";
        private Path outputPath;

        static CliConfig fromArgs(final String[] args)
        {
            final CliConfig config = new CliConfig();
            for (int index = 0; index < args.length; index++)
            {
                final String arg = args[index];
                switch (arg)
                {
                    case "--scenario":
                        config.scenario = Scenario.parse(nextArg(args, ++index, arg));
                        break;
                    case "--wait-strategy":
                        config.waitStrategy = WaitStrategyKind.parse(nextArg(args, ++index, arg));
                        break;
                    case "--buffer-size":
                        config.bufferSize = Integer.parseInt(nextArg(args, ++index, arg));
                        break;
                    case "--events-total":
                        config.eventsTotal = Long.parseLong(nextArg(args, ++index, arg));
                        break;
                    case "--batch-size":
                        config.batchSize = Integer.parseInt(nextArg(args, ++index, arg));
                        break;
                    case "--warmup-rounds":
                        config.warmupRounds = Integer.parseInt(nextArg(args, ++index, arg));
                        break;
                    case "--measured-rounds":
                        config.measuredRounds = Integer.parseInt(nextArg(args, ++index, arg));
                        break;
                    case "--run-order":
                        config.runOrder = nextArg(args, ++index, arg);
                        break;
                    case "--output":
                        config.outputPath = Path.of(nextArg(args, ++index, arg));
                        break;
                    case "--help":
                    case "-h":
                        printHelpAndExit();
                        break;
                    default:
                        throw new IllegalArgumentException("Unsupported argument: " + arg);
                }
            }

            if (config.scenario == null)
            {
                throw new IllegalArgumentException("Missing required argument --scenario");
            }
            return config;
        }

        ResolvedConfig resolve()
        {
            final Scenario resolvedScenario = scenario;
            final WaitStrategyKind resolvedWaitStrategy =
                    waitStrategy == null ? resolvedScenario.defaultWaitStrategy : waitStrategy;
            final int resolvedBufferSize =
                    bufferSize == null ? resolvedScenario.defaultBufferSize : bufferSize;
            final long resolvedEventsTotal =
                    eventsTotal == null ? resolvedScenario.defaultEventsTotal : eventsTotal;
            final int resolvedBatchSize =
                    batchSize == null ? resolvedScenario.defaultBatchSize : batchSize;

            if (resolvedBufferSize <= 0 || (resolvedBufferSize & (resolvedBufferSize - 1)) != 0)
            {
                throw new IllegalArgumentException("buffer size must be a power of two");
            }
            if (resolvedBatchSize <= 0)
            {
                throw new IllegalArgumentException("batch size must be > 0");
            }
            if (resolvedEventsTotal <= 0L)
            {
                throw new IllegalArgumentException("events total must be > 0");
            }
            if (measuredRounds <= 0)
            {
                throw new IllegalArgumentException("measured rounds must be > 0");
            }
            if (resolvedScenario == Scenario.MPSC_BATCH && resolvedEventsTotal % MPSC_PRODUCER_COUNT != 0L)
            {
                throw new IllegalArgumentException(
                        "mpsc_batch requires events_total divisible by " + MPSC_PRODUCER_COUNT);
            }

            return new ResolvedConfig(
                    resolvedScenario,
                    resolvedWaitStrategy,
                    resolvedBufferSize,
                    resolvedEventsTotal,
                    resolvedBatchSize,
                    warmupRounds,
                    measuredRounds,
                    runOrder,
                    outputPath);
        }

        private static String nextArg(final String[] args, final int index, final String flag)
        {
            if (index >= args.length)
            {
                throw new IllegalArgumentException("Missing value for " + flag);
            }
            return args[index];
        }
    }

    static final class ResolvedConfig
    {
        final Scenario scenario;
        final WaitStrategyKind waitStrategy;
        final int bufferSize;
        final long eventsTotal;
        final int batchSize;
        final int warmupRounds;
        final int measuredRounds;
        final String runOrder;
        final Path outputPath;

        ResolvedConfig(
                final Scenario scenario,
                final WaitStrategyKind waitStrategy,
                final int bufferSize,
                final long eventsTotal,
                final int batchSize,
                final int warmupRounds,
                final int measuredRounds,
                final String runOrder,
                final Path outputPath)
        {
            this.scenario = scenario;
            this.waitStrategy = waitStrategy;
            this.bufferSize = bufferSize;
            this.eventsTotal = eventsTotal;
            this.batchSize = batchSize;
            this.warmupRounds = warmupRounds;
            this.measuredRounds = measuredRounds;
            this.runOrder = runOrder;
            this.outputPath = outputPath;
        }

        int totalRounds()
        {
            return warmupRounds + measuredRounds;
        }
    }

    static final class ComparisonEvent
    {
        long value;
        long stage1Value;
        long stage2Value;
        long stage3Value;
    }

    static final class SummingHandler implements EventHandler<ComparisonEvent>
    {
        private final AtomicLong processed;
        private final AtomicLong checksum;
        private final AtomicInteger readyCount;

        SummingHandler(final AtomicLong processed, final AtomicLong checksum, final AtomicInteger readyCount)
        {
            this.processed = processed;
            this.checksum = checksum;
            this.readyCount = readyCount;
        }

        @Override
        public void onEvent(final ComparisonEvent event, final long sequence, final boolean endOfBatch)
        {
            checksum.addAndGet(event.value);
            processed.incrementAndGet();
        }

        @Override
        public void onStart()
        {
            readyCount.incrementAndGet();
        }
    }

    static final class Stage1Handler implements EventHandler<ComparisonEvent>
    {
        private final AtomicInteger readyCount;

        Stage1Handler(final AtomicInteger readyCount)
        {
            this.readyCount = readyCount;
        }

        @Override
        public void onEvent(final ComparisonEvent event, final long sequence, final boolean endOfBatch)
        {
            event.stage1Value = event.value + 1L;
        }

        @Override
        public void onStart()
        {
            readyCount.incrementAndGet();
        }
    }

    static final class Stage2Handler implements EventHandler<ComparisonEvent>
    {
        private final AtomicInteger readyCount;

        Stage2Handler(final AtomicInteger readyCount)
        {
            this.readyCount = readyCount;
        }

        @Override
        public void onEvent(final ComparisonEvent event, final long sequence, final boolean endOfBatch)
        {
            event.stage2Value = event.stage1Value + 3L;
        }

        @Override
        public void onStart()
        {
            readyCount.incrementAndGet();
        }
    }

    static final class Stage3Handler implements EventHandler<ComparisonEvent>
    {
        private final AtomicLong processed;
        private final AtomicLong checksum;
        private final AtomicInteger readyCount;

        Stage3Handler(final AtomicLong processed, final AtomicLong checksum, final AtomicInteger readyCount)
        {
            this.processed = processed;
            this.checksum = checksum;
            this.readyCount = readyCount;
        }

        @Override
        public void onEvent(final ComparisonEvent event, final long sequence, final boolean endOfBatch)
        {
            event.stage3Value = event.stage2Value + 7L;
            checksum.addAndGet(event.stage3Value);
            processed.incrementAndGet();
        }

        @Override
        public void onStart()
        {
            readyCount.incrementAndGet();
        }
    }

    static final class RoundRecord
    {
        final int roundIndex;
        final RoundPhase phase;
        final long elapsedNs;
        final double opsPerSec;
        final long eventsExpected;
        final long eventsProcessed;
        final long checksumExpected;
        final long checksumObserved;
        final boolean checksumValid;

        private RoundRecord(
                final int roundIndex,
                final RoundPhase phase,
                final long elapsedNs,
                final double opsPerSec,
                final long eventsExpected,
                final long eventsProcessed,
                final long checksumExpected,
                final long checksumObserved,
                final boolean checksumValid)
        {
            this.roundIndex = roundIndex;
            this.phase = phase;
            this.elapsedNs = elapsedNs;
            this.opsPerSec = opsPerSec;
            this.eventsExpected = eventsExpected;
            this.eventsProcessed = eventsProcessed;
            this.checksumExpected = checksumExpected;
            this.checksumObserved = checksumObserved;
            this.checksumValid = checksumValid;
        }

        static RoundRecord from(
                final int roundIndex,
                final RoundPhase phase,
                final long elapsedNs,
                final long eventsExpected,
                final long eventsProcessed,
                final long checksumExpected,
                final long checksumObserved)
        {
            return new RoundRecord(
                    roundIndex,
                    phase,
                    elapsedNs,
                    opsPerSecond(eventsExpected, elapsedNs),
                    eventsExpected,
                    eventsProcessed,
                    checksumExpected,
                    checksumObserved,
                    eventsExpected == eventsProcessed && checksumExpected == checksumObserved);
        }
    }

    static final class SummaryStats
    {
        final boolean checksumValidAll;
        final double medianOpsPerSec;
        final double meanOpsPerSec;
        final double minOpsPerSec;
        final double maxOpsPerSec;
        final double stddevOpsPerSec;
        final double cv;

        private SummaryStats(
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

        static SummaryStats fromRuns(final List<RoundRecord> runs)
        {
            final boolean checksumValidAll = runs.stream().allMatch(run -> run.checksumValid);
            final List<Double> measured = runs.stream()
                    .filter(run -> run.phase == RoundPhase.MEASURED)
                    .map(run -> run.opsPerSec)
                    .sorted(Comparator.naturalOrder())
                    .collect(Collectors.toList());

            if (measured.isEmpty())
            {
                return new SummaryStats(checksumValidAll, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
            }

            final double mean = measured.stream().mapToDouble(Double::doubleValue).average().orElse(0.0);
            final double median = measured.size() % 2 == 0
                    ? (measured.get(measured.size() / 2 - 1) + measured.get(measured.size() / 2)) / 2.0
                    : measured.get(measured.size() / 2);
            final double min = measured.get(0);
            final double max = measured.get(measured.size() - 1);
            double variance = 0.0;
            for (double value : measured)
            {
                final double delta = value - mean;
                variance += delta * delta;
            }
            variance /= measured.size();
            final double stddev = Math.sqrt(variance);
            final double cv = mean == 0.0 ? 0.0 : stddev / mean;

            return new SummaryStats(checksumValidAll, median, mean, min, max, stddev, cv);
        }
    }

    static final class EnvInfo
    {
        final String os;
        final String arch;
        final String runtime;
        final String runtimeVersion;

        EnvInfo(final String os, final String arch, final String runtime, final String runtimeVersion)
        {
            this.os = os;
            this.arch = arch;
            this.runtime = runtime;
            this.runtimeVersion = runtimeVersion;
        }
    }

    static final class HarnessResult
    {
        final String implName;
        final Scenario scenario;
        final String measurementKind;
        final int bufferSize;
        final WaitStrategyKind waitStrategy;
        final int producerCount;
        final int consumerCount;
        final int pipelineStages;
        final int batchSize;
        final long eventsTotal;
        final int warmupRounds;
        final int measuredRounds;
        final String runOrder;
        final EnvInfo env;
        final List<RoundRecord> runs;
        final SummaryStats summary;

        private HarnessResult(
                final String implName,
                final Scenario scenario,
                final String measurementKind,
                final int bufferSize,
                final WaitStrategyKind waitStrategy,
                final int producerCount,
                final int consumerCount,
                final int pipelineStages,
                final int batchSize,
                final long eventsTotal,
                final int warmupRounds,
                final int measuredRounds,
                final String runOrder,
                final EnvInfo env,
                final List<RoundRecord> runs,
                final SummaryStats summary)
        {
            this.implName = implName;
            this.scenario = scenario;
            this.measurementKind = measurementKind;
            this.bufferSize = bufferSize;
            this.waitStrategy = waitStrategy;
            this.producerCount = producerCount;
            this.consumerCount = consumerCount;
            this.pipelineStages = pipelineStages;
            this.batchSize = batchSize;
            this.eventsTotal = eventsTotal;
            this.warmupRounds = warmupRounds;
            this.measuredRounds = measuredRounds;
            this.runOrder = runOrder;
            this.env = env;
            this.runs = runs;
            this.summary = summary;
        }

        static HarnessResult from(final ResolvedConfig config, final List<RoundRecord> runs)
        {
            return new HarnessResult(
                    "lmax-disruptor",
                    config.scenario,
                    "throughput",
                    config.bufferSize,
                    config.waitStrategy,
                    config.scenario.producerCount,
                    config.scenario.consumerCount,
                    config.scenario.pipelineStages,
                    config.batchSize,
                    config.eventsTotal,
                    config.warmupRounds,
                    config.measuredRounds,
                    config.runOrder,
                    new EnvInfo(
                            System.getProperty("os.name", "unknown"),
                            System.getProperty("os.arch", "unknown"),
                            "java",
                            System.getProperty("java.runtime.version", System.getProperty("java.version", "unknown"))),
                    runs,
                    SummaryStats.fromRuns(runs));
        }

        String toJson()
        {
            final StringBuilder builder = new StringBuilder();
            builder.append("{\n");
            appendStringField(builder, 2, "impl_name", implName, true);
            appendStringField(builder, 2, "scenario", scenario.id, true);
            appendStringField(builder, 2, "measurement_kind", measurementKind, true);
            appendNumberField(builder, 2, "buffer_size", bufferSize, true);
            appendStringField(builder, 2, "wait_strategy", waitStrategy.id, true);
            appendNumberField(builder, 2, "producer_count", producerCount, true);
            appendNumberField(builder, 2, "consumer_count", consumerCount, true);
            appendNumberField(builder, 2, "pipeline_stages", pipelineStages, true);
            appendNumberField(builder, 2, "batch_size", batchSize, true);
            appendNumberField(builder, 2, "events_total", eventsTotal, true);
            appendNumberField(builder, 2, "warmup_rounds", warmupRounds, true);
            appendNumberField(builder, 2, "measured_rounds", measuredRounds, true);
            appendStringField(builder, 2, "run_order", runOrder, true);

            builder.append("  \"env\": {\n");
            appendStringField(builder, 4, "os", env.os, true);
            appendStringField(builder, 4, "arch", env.arch, true);
            appendStringField(builder, 4, "runtime", env.runtime, true);
            appendStringField(builder, 4, "runtime_version", env.runtimeVersion, false);
            builder.append("  },\n");

            builder.append("  \"runs\": [\n");
            for (int index = 0; index < runs.size(); index++)
            {
                final RoundRecord run = runs.get(index);
                builder.append("    {\n");
                appendNumberField(builder, 6, "round_index", run.roundIndex, true);
                appendStringField(builder, 6, "phase", run.phase.id, true);
                appendNumberField(builder, 6, "elapsed_ns", run.elapsedNs, true);
                appendFloatField(builder, 6, "ops_per_sec", run.opsPerSec, true);
                appendNumberField(builder, 6, "events_expected", run.eventsExpected, true);
                appendNumberField(builder, 6, "events_processed", run.eventsProcessed, true);
                appendNumberField(builder, 6, "checksum_expected", run.checksumExpected, true);
                appendNumberField(builder, 6, "checksum_observed", run.checksumObserved, true);
                appendBooleanField(builder, 6, "checksum_valid", run.checksumValid, false);
                if (index + 1 == runs.size())
                {
                    builder.append("    }\n");
                }
                else
                {
                    builder.append("    },\n");
                }
            }
            builder.append("  ],\n");

            builder.append("  \"summary\": {\n");
            appendBooleanField(builder, 4, "checksum_valid_all", summary.checksumValidAll, true);
            appendFloatField(builder, 4, "median_ops_per_sec", summary.medianOpsPerSec, true);
            appendFloatField(builder, 4, "mean_ops_per_sec", summary.meanOpsPerSec, true);
            appendFloatField(builder, 4, "min_ops_per_sec", summary.minOpsPerSec, true);
            appendFloatField(builder, 4, "max_ops_per_sec", summary.maxOpsPerSec, true);
            appendFloatField(builder, 4, "stddev_ops_per_sec", summary.stddevOpsPerSec, true);
            appendFloatField(builder, 4, "cv", summary.cv, false);
            builder.append("  }\n");
            builder.append("}\n");
            return builder.toString();
        }
    }

    private static void appendStringField(
            final StringBuilder builder,
            final int indent,
            final String key,
            final String value,
            final boolean trailing)
    {
        builder.append(" ".repeat(indent))
                .append('"').append(key).append("\": \"")
                .append(escapeJson(value))
                .append('"');
        if (trailing)
        {
            builder.append(',');
        }
        builder.append('\n');
    }

    private static void appendNumberField(
            final StringBuilder builder,
            final int indent,
            final String key,
            final long value,
            final boolean trailing)
    {
        builder.append(" ".repeat(indent))
                .append('"').append(key).append("\": ")
                .append(value);
        if (trailing)
        {
            builder.append(',');
        }
        builder.append('\n');
    }

    private static void appendFloatField(
            final StringBuilder builder,
            final int indent,
            final String key,
            final double value,
            final boolean trailing)
    {
        builder.append(" ".repeat(indent))
                .append('"').append(key).append("\": ")
                .append(String.format(Locale.ROOT, "%.6f", value));
        if (trailing)
        {
            builder.append(',');
        }
        builder.append('\n');
    }

    private static void appendBooleanField(
            final StringBuilder builder,
            final int indent,
            final String key,
            final boolean value,
            final boolean trailing)
    {
        builder.append(" ".repeat(indent))
                .append('"').append(key).append("\": ")
                .append(value ? "true" : "false");
        if (trailing)
        {
            builder.append(',');
        }
        builder.append('\n');
    }

    private static String escapeJson(final String value)
    {
        return value.replace("\\", "\\\\").replace("\"", "\\\"");
    }

    private static void printHelpAndExit()
    {
        System.out.println("Usage: java ... HeadToHead --scenario <SCENARIO> [options]");
        System.out.println();
        System.out.println("Options:");
        System.out.println("  --scenario <unicast|unicast_batch|mpsc_batch|pipeline>");
        System.out.println("  --wait-strategy <yielding|busy-spin>");
        System.out.println("  --buffer-size <N>");
        System.out.println("  --events-total <N>");
        System.out.println("  --batch-size <N>");
        System.out.println("  --warmup-rounds <N>");
        System.out.println("  --measured-rounds <N>");
        System.out.println("  --run-order <rust-then-java|java-then-rust|standalone>");
        System.out.println("  --output <PATH>");
        System.exit(0);
    }
}
