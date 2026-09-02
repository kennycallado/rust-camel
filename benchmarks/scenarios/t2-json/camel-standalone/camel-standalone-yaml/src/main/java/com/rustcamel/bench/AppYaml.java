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
import org.apache.camel.main.Main;

/**
 * Pair B entrypoint for the t2-json scenario (OpenSpec change
 * {@code bench-missing-cells} task 2.2). Mirrors the
 * {@code t2-realistic-eip} AppYaml.java but loads the t2-json
 * routes.yaml (same logical route as the dsl-module {@link App},
 * authored in YAML DSL and parsed at runtime via {@code
 * camel-yaml-dsl}).
 *
 * <p>The route's custom steps (set canonical body, bracket the tick,
 * insert the {@code "bench": true} member on the parsed tree, assert
 * exact output length, gate the marker, write the per-tick latency
 * record) are bound as named beans — the route STRUCTURE stays in the
 * parsed YAML file, which is the property Pair B measures. Kept in this
 * module only: Pair A's classpath carries no beans, no routes.yaml.
 *
 * <p>Marker contract: one {@code BENCH_ROUTE_READY bytes=<n>} line; an
 * assert failure kills the route before the marker (cell fails). No
 * self-instrumentation for cold-start — the harness owns the clock from
 * outside.
 *
 * <p>Tick mode (OpenSpec change {@code bench-consol-tick} task 2.4):
 * repeating warm timer {@code timer:bench?period=10&repeatCount=10000&delay=0}
 * (timer URI lives in routes.yaml); the canonical body is prebuilt ONCE
 * (digest logged once at startup — per-exchange SHA printing would spam
 * 10000 lines) and set per exchange by the {@code benchBody} bean; the
 * marker bean is latched to the FIRST completed exchange (exactly one
 * marker line per process lifetime); the {@code markStart}/{@code
 * writeLatency} beans bracket each exchange and append {@code
 * BENCH_LATENCY <id> <duration_ns>} to the {@code BENCH_LATENCY_FILE}
 * path (env read once at startup; canonical fallback matches the M2
 * protocol-B reader's path, mirroring the dsl module and the lib crate).
 */
public final class AppYaml {
    private AppYaml() {
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

        final AtomicBoolean markerEmitted = new AtomicBoolean(false);
        final AtomicLong tickCounter = new AtomicLong(0);

        Main main = new Main();
        main.bind("benchBody", setCanonicalBody(payload));
        main.bind("markStart", markStart());
        main.bind("insertBench", insertBenchMember());
        main.bind("assertOutput", assertOutput(size));
        main.bind("emitMarker", emitMarker(markerEmitted));
        main.bind("writeLatency", writeLatency(latencyFile, tickCounter));
        main.configure()
                .withRoutesIncludePattern("classpath:routes.yaml");
        main.run(args);
    }

    /// Sets the prebuilt canonical body (built + asserted + digest
    /// logged ONCE at startup in main). Per exchange this is the
    /// {@code setBody(constant(...))} equivalent for the YAML route.
    static Processor setCanonicalBody(String payload) {
        return exchange -> exchange.getMessage().setBody(payload);
    }

    /// Bracket step: records t_start immediately after the body is set
    /// (same position as the dsl module and the lib crate's task-2.2
    /// route). Long (boxed) so it round-trips through exchange property
    /// type erasure.
    static Processor markStart() {
        return exchange ->
                exchange.setProperty("BenchStart", System.nanoTime());
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

    /// Marker step — fires on the FIRST completed exchange only (tick
    /// mode repeats the route per tick; the marker contract is exactly
    /// one line). Reads the `benchOutLen` header the output assert set,
    /// so an assert failure can never produce the marker.
    static Processor emitMarker(AtomicBoolean markerEmitted) {
        return exchange -> {
            if (markerEmitted.compareAndSet(false, true)) {
                System.out.println("BENCH_ROUTE_READY bytes="
                        + exchange.getMessage().getHeader("benchOutLen"));
            }
        };
    }

    /// Trailing latency step — appends `BENCH_LATENCY <id> <duration_ns>`
    /// to the sink file per exchange (the Protocol-B warm record).
    /// Write failures are swallowed — the harness detects missing
    /// records.
    static Processor writeLatency(Path latencyFile, AtomicLong tickCounter) {
        return exchange -> {
            long id = tickCounter.incrementAndGet();
            long tEnd = System.nanoTime();
            Long tStart = exchange.getProperty("BenchStart", Long.class);
            long durationNs = tEnd - tStart;
            String line = "BENCH_LATENCY " + id + " " + durationNs + "\n";
            try {
                Files.writeString(latencyFile, line,
                        StandardCharsets.UTF_8, StandardOpenOption.APPEND);
            } catch (Exception e) {
                // Swallow — harness detects missing records.
            }
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
    /// for this cell (`${cell//\//_}` of `t2-json/camel-standalone-yaml`)
    /// — the harness argv for this cell is bare, so the default is what
    /// makes the reader find the log (mirrors the lib crate, task 2.2).
    static String latencyFilePath() {
        String env = System.getenv("BENCH_LATENCY_FILE");
        if (env == null || env.isBlank()) {
            return "/tmp/v3-protocol-b-t2-json_camel-standalone-yaml.log";
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
