package com.rustcamel.bench;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.ObjectMapper;
import com.fasterxml.jackson.databind.node.ObjectNode;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardOpenOption;
import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicLong;
import org.apache.camel.Processor;
import org.apache.camel.builder.RouteBuilder;
import org.apache.camel.main.Main;
import org.apache.camel.model.dataformat.JsonLibrary;

/**
 * Pair A entrypoint (hardcoded Java-DSL route) for the t2-json scenario
 * (OpenSpec change {@code bench-missing-cells} task 2.2). Mirrors the
 * {@code t2-realistic-eip} App.java (Pair A, no {@code camel-yaml-dsl}
 * on the classpath — classpath isolation is a pairing-fairness
 * invariant) but implements the t2-json Protocol-B route: timer ->
 * process(build canonical body + log BENCH_INPUT_SHA256) ->
 * unmarshal(json) -> filter(jsonpath) -> process(insert {@code "bench":
 * true} member on the parsed tree) -> marshal(json) -> process(assert
 * exact output length) -> log marker.
 *
 * <p>The canonical input {@code {"id":"bench","seq":<tick>,"fill":
 * "<K×'b'>"}} is byte-identical across every t2-json contender
 * (README golden table); its digest is logged before the marker so a
 * payload-skew misconfiguration is observable. {@code K = size -
 * overhead} where overhead is 20 (prefix) + digits(tick) + 9 (infix) +
 * 2 (suffix) = 31 + digits(tick).
 *
 * <p>The transform mutates the PARSED Jackson tree ({@link ObjectNode})
 * in place — {@code marshal} is the single serializer, exactly like the
 * rust-camel-lib fixture's closure over {@code Body::Json}. The output
 * is verified by exact length ({@code size + 13}) and parsed semantics
 * (id/seq/fill/bench) — byte equality is NOT required on output because
 * JVM serde may reorder fields (scenario README, §Transform mechanism).
 *
 * <p>Marker contract: one stdout/stderr line {@code BENCH_ROUTE_READY
 * bytes=<n>} (the harness greps the combined stream and enforces
 * exactly-1). An assert failure throws inside the processor — the route
 * dies BEFORE the marker, so the cell fails.
 *
 * <p>Tick mode (OpenSpec change {@code bench-consol-tick} task 2.4,
 * mirroring the consolidated lib crate's task 2.2 and the
 * xsd-validation-bridge DSL reference): the timer is the repeating
 * warm-tick form {@code timer:bench?period=10&repeatCount=10000&delay=0}; the
 * canonical body is prebuilt ONCE (digest logged once at startup, before
 * the first tick — per-exchange SHA printing would spam 10000 lines) and
 * set per exchange via {@code setBody(constant(...))}. The marker keeps
 * its code-path position (right after the output assert) but is latched
 * to the FIRST completed exchange — exactly one marker line per process
 * lifetime. A {@code BenchStart} exchange property brackets each
 * exchange (set AFTER setBody, mirroring the lib crate); the trailing
 * step appends {@code BENCH_LATENCY <id> <duration_ns>} to the
 * {@code BENCH_LATENCY_FILE} path (env read once at startup; the
 * canonical fallback matches the M2 protocol-B reader's {@code
 * ${cell//\//_}} path, so the bare harness argv needs no env wiring).
 *
 * <p>No self-instrumentation for cold-start — timing and RSS are
 * captured by the harness from OUTSIDE this process; the per-tick
 * latency writer is the Protocol-B warm record, not self-timing.
 */
public final class App {
    private App() {
    }

    private static final ObjectMapper MAPPER = new ObjectMapper();

    public static void main(String[] args) throws Exception {
        final int size = benchPayloadBytes();
        final String payload = canonicalJsonBody(size, CANONICAL_SELFTEST_TICK);
        if (payload.length() != size) {
            throw new IllegalStateException(
                    "t2-json input length " + payload.length() + " != expected " + size);
        }
        System.out.println("BENCH_INPUT_SHA256=" + sha256Hex(payload));

        // Tick-mode latency sink — truncate at startup (no
        // stale records leak across runs).
        final Path latencyFile = Path.of(latencyFilePath());
        Files.createDirectories(latencyFile.getParent());
        Files.writeString(latencyFile, "",
                StandardOpenOption.CREATE, StandardOpenOption.TRUNCATE_EXISTING);

        Main main = new Main();
        final AtomicBoolean markerEmitted = new AtomicBoolean(false);
        final AtomicLong tickCounter = new AtomicLong(0);
        main.configure().addRoutesBuilder(new RouteBuilder() {
            @Override
            public void configure() {
                from("timer:bench?period=10&repeatCount=10000&delay=0")
                        .setBody(constant(payload))
                        // Records t_start immediately after the body is
                        // set (same bracket position as the lib crate's
                        // task-2.2 route). Long (boxed) so it
                        // round-trips through exchange property type
                        // erasure.
                        .process(exchange ->
                                exchange.setProperty("BenchStart", System.nanoTime()))
                        .unmarshal().json(JsonLibrary.Jackson, ObjectNode.class)
                        .filter(jsonpath("$[?(@.id == 'bench')]"))
                        .process(insertBenchMember())
                        .end()
                        .marshal().json(JsonLibrary.Jackson)
                        .process(assertOutput(size))
                        // Marker fires on the FIRST completed exchange
                        // only — tick mode repeats this step per tick,
                        // the marker contract is exactly one line.
                        .process(exchange -> {
                            if (markerEmitted.compareAndSet(false, true)) {
                                System.out.println("BENCH_ROUTE_READY bytes="
                                        + exchange.getMessage().getHeader("benchOutLen"));
                            }
                        })
                        .process(exchange -> {
                            long id = tickCounter.incrementAndGet();
                            long tEnd = System.nanoTime();
                            Long tStart = exchange.getProperty("BenchStart", Long.class);
                            long durationNs = tEnd - tStart;
                            String line = "BENCH_LATENCY " + id + " " + durationNs + "\n";
                            try {
                                Files.writeString(latencyFile, line,
                                        StandardCharsets.UTF_8,
                                        StandardOpenOption.APPEND);
                            } catch (Exception e) {
                                // Swallow — harness detects missing records.
                            }
                        });
            }
        });
        main.run(args);
    }

    /// Transform step: structured mutation of the PARSED tree — insert
    /// the `"bench": true` member on the ObjectNode and keep the body
    /// structured. `marshal` downstream is the single serializer.
    static Processor insertBenchMember() {
        return exchange -> {
            ObjectNode node = exchange.getMessage().getBody(ObjectNode.class);
            if (node == null) {
                throw new IllegalStateException(
                        "t2-json body after unmarshal is not an ObjectNode");
            }
            node.put("bench", true);
        };
    }

    /// Output assert — exact `size + 13` length AND parsed semantic
    /// equality (id == "bench", seq present, fill all 'b',
    /// bench == true). Sets `benchOutLen` for the marker on success.
    /// Any failure throws — route dies, no marker.
    static Processor assertOutput(int size) {
        return exchange -> {
            String text = exchange.getMessage().getBody(String.class);
            int expected = size + 13;
            if (text == null) {
                throw new IllegalStateException("t2-json marshaled output is null");
            }
            if (text.length() != expected) {
                throw new IllegalStateException(
                        "t2-json output length " + text.length() + " != expected " + expected);
            }
            JsonNode node = MAPPER.readTree(text);
            if (!node.isObject()) {
                throw new IllegalStateException("t2-json output is not a JSON object");
            }
            if (!"bench".equals(node.path("id").asText())) {
                throw new IllegalStateException("t2-json output id != \"bench\"");
            }
            if (!node.has("seq")) {
                throw new IllegalStateException("t2-json output seq member missing");
            }
            String fill = node.path("fill").asText();
            for (int i = 0; i < fill.length(); i++) {
                if (fill.charAt(i) != 'b') {
                    throw new IllegalStateException("t2-json output fill is not all 'b'");
                }
            }
            if (!node.path("bench").isBoolean() || !node.path("bench").asBoolean()) {
                throw new IllegalStateException("t2-json output bench != true");
            }
            exchange.getMessage().setHeader("benchOutLen", text.length());
        };
    }

    /// Canonical self-test tick — same value as the Rust fixtures and
    /// the harness golden table (`(size, 0)` entries).
    static final long CANONICAL_SELFTEST_TICK = 0L;

    /// Default payload size (bytes) — same default as the harness.
    static final int DEFAULT_PAYLOAD_BYTES = 32768;

    /// Resolve `BENCH_PAYLOAD_BYTES` (default 32768), validated against
    /// the payload axis. Invalid values abort before any marker.
    static int benchPayloadBytes() {
        String raw = System.getenv("BENCH_PAYLOAD_BYTES");
        if (raw == null || raw.isBlank()) {
            return DEFAULT_PAYLOAD_BYTES;
        }
        int parsed;
        try {
            parsed = Integer.parseInt(raw.trim());
        } catch (NumberFormatException e) {
            throw new IllegalArgumentException(
                    "BENCH_PAYLOAD_BYTES='" + raw + "' is not an int; valid sizes: "
                            + "[1024, 32768, 262144, 1048576]", e);
        }
        if (parsed != 1024 && parsed != 32768 && parsed != 262144 && parsed != 1048576) {
            throw new IllegalArgumentException("BENCH_PAYLOAD_BYTES=" + parsed
                    + "; valid sizes: [1024, 32768, 262144, 1048576]");
        }
        return parsed;
    }

    /// Tick-mode latency sink path: `BENCH_LATENCY_FILE` env when set,
    /// else the canonical harness path the M2 protocol-B reader derives
    /// for this cell (`${cell//\//_}` of `t2-json/camel-standalone-dsl`)
    /// — the harness argv for this cell is bare, so the default is what
    /// makes the reader find the log (mirrors the lib crate, task 2.2).
    static String latencyFilePath() {
        String env = System.getenv("BENCH_LATENCY_FILE");
        if (env == null || env.isBlank()) {
            return "/tmp/v3-protocol-b-t2-json_camel-standalone-dsl.log";
        }
        return env.trim();
    }

    /// Canonical body builder — same formula as bench-loadgen's
    /// `canonical_json_body` (task 1.2): UTF-8, zero whitespace, fixed
    /// field order id,seq,fill; `K = size - (31 + digits(tick))` makes
    /// the document exactly `size` bytes.
    static String canonicalJsonBody(int size, long tick) {
        String tickStr = Long.toString(tick);
        // 20-char prefix `{"id":"bench","seq":` + digits(tick)
        // + 9-char infix `,"fill":"` + 2-char suffix `"}`
        int overhead = 20 + tickStr.length() + 9 + 2;
        int fill = size - overhead;
        if (fill < 1) {
            throw new IllegalArgumentException(
                    "size " + size + " too small for tick " + tick);
        }
        return "{\"id\":\"bench\",\"seq\":" + tickStr + ",\"fill\":\""
                + "b".repeat(fill) + "\"}";
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
