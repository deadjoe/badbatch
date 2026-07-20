package com.lmax.disruptor;

/**
 * Probe-only producer counters used by the BadBatch head-to-head harness.
 *
 * <p>The canonical Java benchmark compiles the unmodified LMAX source, which never references this
 * class; the harness only configures it off before the run. Diagnostic runs compile a temporary,
 * verified copy of {@code SingleProducerSequencer} that calls the counter methods. Counters are
 * thread-local because the single-producer contract has one publisher.
 */
public final class H2HDiagnostics
{
    private static volatile boolean enabled;
    private static final ThreadLocal<Counters> COUNTERS = ThreadLocal.withInitial(Counters::new);

    private H2HDiagnostics()
    {
    }

    public static void configure(final boolean value)
    {
        enabled = value;
    }

    public static boolean enabled()
    {
        return enabled;
    }

    public static void beginRound()
    {
        COUNTERS.set(new Counters());
    }

    public static void recordProducerBackpressure(final long iterations)
    {
        if (!enabled || iterations == 0L)
        {
            return;
        }
        final Counters counters = COUNTERS.get();
        counters.entries++;
        counters.waitLoopIterations += iterations;
        counters.maxWaitLoopIterations = Math.max(counters.maxWaitLoopIterations, iterations);
    }

    public static Snapshot snapshot()
    {
        final Counters counters = COUNTERS.get();
        return new Snapshot(
                counters.entries,
                counters.waitLoopIterations,
                counters.maxWaitLoopIterations);
    }

    private static final class Counters
    {
        long entries;
        long waitLoopIterations;
        long maxWaitLoopIterations;
    }

    public record Snapshot(
            long entries,
            long waitLoopIterations,
            long maxWaitLoopIterations)
    {
    }
}
