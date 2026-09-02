package com.rustcamel.bench;

import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardOpenOption;
import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicLong;
import org.apache.camel.AggregationStrategy;
import org.apache.camel.Exchange;
import org.apache.camel.Processor;
import org.apache.camel.builder.RouteBuilder;
import org.apache.camel.main.Main;

/**
 * Pair A entrypoint (hardcoded Java-DSL route) for the split-aggregate
 * scenario (OpenSpec change {@code bench-missing-cells} task 2.4).
 * Mirrors the t2-json App.java pairing shape (Pair A, no {@code
 * camel-yaml-dsl} on the classpath — classpath isolation is a pairing
 * fairness invariant) but implements the split-aggregate Protocol-B
 * route (design D3): timer -> process(build canonical 100-item array +
 * log BENCH_INPUT_SHA256) -> split(jsonpath "$", sequential — the
 * Splitter is sequential unless an executorService is configured) ->
 * to(direct:agg-in) per fragment; direct:agg-in -> setHeader(constant
 * correlation) -> aggregate(completionSize=100, list-append strategy,
 * forceCompletionOnStop=false) -> process(assert completion) -> log
 * marker.
 *
 * <p>The canonical input is the fixed JSON array {@code
 * ["b0","b1",...,"b99"]} — exactly 591 bytes, byte-identical across
 * every split-aggregate contender (scenario README golden table); its
 * digest is logged before the marker so payload skew is observable.
 *
 * <p>Completion contract (spec): the marker fires ONLY from the
 * aggregator's completion path, and only when the aggregated collection
 * holds exactly {@value #BENCH_ITEMS} items AND the aggregator-stamped
 * {@code CamelAggregatedSize} property agrees. {@code
 * forceCompletionOnStop} stays false — an incomplete bucket emits NO
 * marker and the cell fails by the harness marker deadline. An assert
 * failure throws inside the processor — the route dies BEFORE the
 * marker, so the cell fails.
 *
 * <p>Tick mode (OpenSpec change {@code bench-consol-tick} task 2.4,
 * mirroring the consolidated lib crate's task 2.2 and the
 * xsd-validation-bridge DSL reference): repeating warm timer {@code
 * timer:bench?period=10&repeatCount=10000&delay=0}; the canonical array is
 * prebuilt ONCE (digest logged once at startup — per-exchange SHA
 * printing would spam 10000 lines) and set per exchange via {@code
 * setBody(constant(...))}. The tick start is carried ROUTE-LOCALLY
 * (AtomicLong nanoTime slot), NOT on the exchange — the lib crate
 * verified empirically (task 2.2) that neither extensions nor
 * properties reliably survive the split+aggregate boundary; ticks are
 * sequential (10 ms period vs a sub-millisecond pipeline) so the shared
 * slot cannot straddle ticks. The trailing step of the MAIN (timer)
 * route appends {@code BENCH_LATENCY <id> <duration_ns>} to the {@code
 * BENCH_LATENCY_FILE} path; the aggregator consumer route is NOT
 * instrumented (blessed contract, tasks.md 2.3 ruling: its work happens
 * inside the main route's synchronous direct-dispatch window). The
 * marker keeps its completion-path position but is latched to the FIRST
 * completed bucket — exactly one marker line per process lifetime.
 *
 * <p>No self-instrumentation for cold-start — timing and RSS are
 * captured by the harness from OUTSIDE this process; the per-tick
 * latency writer is the Protocol-B warm record, not self-timing.
 */
public final class App {
    private App() {
    }

    public static void main(String[] args) throws Exception {
        final String array = canonicalArray();
        if (array.length() != CANONICAL_ARRAY_BYTES) {
            throw new IllegalStateException(
                    "split-aggregate array length " + array.length()
                            + " != expected " + CANONICAL_ARRAY_BYTES);
        }
        System.out.println("BENCH_INPUT_SHA256=" + sha256Hex(array));

        // Tick-mode latency sink — truncate at startup (no
        // stale records leak across runs).
        final Path latencyFile = Path.of(latencyFilePath());
        Files.createDirectories(latencyFile.getParent());
        Files.writeString(latencyFile, "",
                StandardOpenOption.CREATE, StandardOpenOption.TRUNCATE_EXISTING);

        Main main = new Main();
        final AtomicBoolean markerEmitted = new AtomicBoolean(false);
        final AtomicLong tickCounter = new AtomicLong(0);
        final AtomicLong tickStartNanos = new AtomicLong(0);
        main.configure().addRoutesBuilder(new RouteBuilder() {
            @Override
            public void configure() {
                // Outer route (design D3): one tick sets the canonical
                // array and fans it out. The split fragments are the
                // bare strings "b0".."b99"; each fragment exchange is
                // sent to direct:agg-in. Tick mode (bench-consol-tick
                // 2.4): the tick start is stored in the route-local
                // AtomicLong slot (exchange-scoped state does not
                // survive the split+aggregate boundary — lib lesson,
                // task 2.2); the trailing processor of THIS route
                // writes the per-tick record after the split scope
                // completes.
                from("timer:bench?period=10&repeatCount=10000&delay=0")
                        .setBody(constant(array))
                        .process(exchange ->
                                tickStartNanos.set(System.nanoTime()))
                        .split(jsonpath("$"))
                        .to("direct:agg-in")
                        .end()
                        .process(exchange -> {
                            long id = tickCounter.incrementAndGet();
                            long durationNs = System.nanoTime() - tickStartNanos.get();
                            String line = "BENCH_LATENCY " + id + " " + durationNs + "\n";
                            try {
                                Files.writeString(latencyFile, line,
                                        StandardCharsets.UTF_8,
                                        StandardOpenOption.APPEND);
                            } catch (Exception e) {
                                // Swallow — harness detects missing records.
                            }
                        });

                // Aggregation route: constant correlation key, complete
                // at exactly 100 items. forceCompletionOnStop stays at
                // its DEFAULT false (Camel 4.8's DSL method is no-arg
                // and would SET it true) — an incomplete bucket emits
                // NO marker and the cell fails by the harness marker
                // deadline. NOT latency-instrumented (blessed
                // contract: its work happens inside the main route's
                // synchronous direct-dispatch window). The marker is
                // latched to the FIRST completed bucket — tick mode
                // completes one bucket per tick, the marker contract is
                // exactly one line.
                from("direct:agg-in")
                        .setHeader(BENCH_CORRELATION_HEADER, constant(BENCH_CORRELATION))
                        .aggregate(header(BENCH_CORRELATION_HEADER), appendToList())
                        .completionSize(BENCH_ITEMS)
                        .process(assertCompletion())
                        .process(exchange -> {
                            if (markerEmitted.compareAndSet(false, true)) {
                                System.out.println("BENCH_ROUTE_READY items="
                                        + exchange.getProperty(AGGREGATED_SIZE_PROPERTY));
                            }
                        });
            }
        });
        main.run(args);
    }

    /// List-append aggregation strategy (README `collect_all`): the
    /// first fragment seeds a fresh list; every later fragment appends
    /// to the accumulating exchange's body. Defensive copy so exchange
    /// reuse can never alias the accumulated list.
    static AggregationStrategy appendToList() {
        return (oldExchange, newExchange) -> {
            List<Object> items = oldExchange == null
                    ? new ArrayList<>()
                    : new ArrayList<>(oldExchange.getMessage().getBody(List.class));
            items.add(newExchange.getMessage().getBody(String.class));
            Exchange acc = oldExchange == null ? newExchange : oldExchange;
            acc.getMessage().setBody(items);
            return acc;
        };
    }

    /// Completion assert — the aggregated collection holds exactly 100
    /// items AND the aggregator-stamped `CamelAggregatedSize` property
    /// agrees (README completion contract). Any failure throws — route
    /// dies, no marker.
    static Processor assertCompletion() {
        return exchange -> {
            Integer aggregated = exchange.getProperty(AGGREGATED_SIZE_PROPERTY, Integer.class);
            List<?> items = exchange.getMessage().getBody(List.class);
            int size = items == null ? -1 : items.size();
            if (aggregated == null || aggregated != BENCH_ITEMS) {
                throw new IllegalStateException(
                        "split-aggregate CamelAggregatedSize " + aggregated
                                + " != expected " + BENCH_ITEMS);
            }
            if (size != BENCH_ITEMS) {
                throw new IllegalStateException(
                        "split-aggregate aggregated list size " + size
                                + " != expected " + BENCH_ITEMS);
            }
        };
    }

    /// Number of split fragments / aggregation bucket size (README
    /// canonical route).
    static final int BENCH_ITEMS = 100;

    /// Correlation header + constant value (README canonical route).
    static final String BENCH_CORRELATION_HEADER = "bench.correlation";
    static final String BENCH_CORRELATION = "bench";

    /// Aggregator-stamped completion property (Apache Camel core
    /// constant `Exchange.AGGREGATED_SIZE`).
    static final String AGGREGATED_SIZE_PROPERTY = "CamelAggregatedSize";

    /// Canonical array size in bytes (["b0",...,"b99"]).
    static final int CANONICAL_ARRAY_BYTES = 591;

    /// Tick-mode latency sink path: `BENCH_LATENCY_FILE` env when set,
    /// else the canonical harness path the M2 protocol-B reader derives
    /// for this cell (`${cell//\//_}` of
    /// `split-aggregate/camel-standalone-dsl`) — the harness argv for
    /// this cell is bare, so the default is what makes the reader find
    /// the log (mirrors the lib crate, task 2.2).
    static String latencyFilePath() {
        String env = System.getenv("BENCH_LATENCY_FILE");
        if (env == null || env.isBlank()) {
            return "/tmp/v3-protocol-b-split-aggregate_camel-standalone-dsl.log";
        }
        return env.trim();
    }

    /// Canonical array builder — same formula as the Rust fixtures'
    /// `split_aggregate_array_golden`: 100 items `b0`..`b99`, compact
    /// JSON (no whitespace), exactly 591 bytes.
    static String canonicalArray() {
        StringBuilder sb = new StringBuilder(CANONICAL_ARRAY_BYTES).append('[');
        for (int i = 0; i < BENCH_ITEMS; i++) {
            if (i > 0) {
                sb.append(',');
            }
            sb.append('"').append('b').append(i).append('"');
        }
        return sb.append(']').toString();
    }

    /// Lowercase hex SHA-256 of the UTF-8 bytes of `data`.
    static String sha256Hex(String data) {
        try {
            MessageDigest md = MessageDigest.getInstance("SHA-256");
            byte[] digest = md.digest(data.getBytes(StandardCharsets.UTF_8));
            StringBuilder sb = new StringBuilder(digest.length * 2);
            for (byte b : digest) {
                sb.append(Character.forDigit((b >> 4) & 0xF, 16));
                sb.append(Character.forDigit(b & 0xF, 16));
            }
            return sb.toString();
        } catch (NoSuchAlgorithmException e) {
            throw new IllegalStateException("SHA-256 MessageDigest unavailable", e);
        }
    }
}
