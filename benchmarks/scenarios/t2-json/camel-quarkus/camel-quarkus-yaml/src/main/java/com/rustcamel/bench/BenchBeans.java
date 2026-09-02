package com.rustcamel.bench;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.ObjectMapper;
import com.fasterxml.jackson.databind.node.ObjectNode;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardOpenOption;
import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicLong;
import jakarta.enterprise.context.ApplicationScoped;
import jakarta.enterprise.inject.Produces;
import jakarta.inject.Named;
import org.apache.camel.Exchange;
import org.apache.camel.Processor;

/**
 * Named bean producers for the t2-json YAML route of
 * camel-quarkus-yaml (Pair B, OpenSpec change bench-missing-cells task
 * 2.2). The route STRUCTURE lives in camel/routes.yaml (parsed at
 * runtime — the property Pair B measures); the custom steps resolve
 * these CDI-produced beans via `process: ref:`.
 *
 * <p>The canonical-body builder/verifier logic is identical to the
 * dsl sibling's BenchRoute (same formula, same golden digests) — the
 * two subprojects are separate build artifacts by design (pairing
 * classpath isolation), so the small per-family duplication is
 * deliberate.
 *
 * <p>Marker contract: one {@code BENCH_ROUTE_READY bytes=<n>} line; an
 * assert failure kills the route before the marker (cell fails). No
 * self-instrumentation for cold-start — the harness owns the clock from
 * outside.
 *
 * <p>Tick mode (OpenSpec change {@code bench-consol-tick} task 2.5,
 * mirroring the standalone fixtures' task 2.4): the repeating warm
 * timer URI lives in camel/routes.yaml; the {@code markStart} bean
 * brackets each exchange (BenchStart property, set right after the
 * body), the {@code emitMarker} bean is latched to the FIRST completed
 * exchange (exactly one marker line per process lifetime), and the
 * {@code writeLatency} bean appends {@code BENCH_LATENCY <id>
 * <duration_ns>} per exchange to the {@code BENCH_LATENCY_FILE} path
 * (env read once at startup; the canonical fallback matches the M2
 * protocol-B reader's path — the sink is truncated at producer time,
 * i.e. before any tick, so no stale records leak across runs).
 */
@ApplicationScoped
public class BenchBeans {

    private static final ObjectMapper MAPPER = new ObjectMapper();

    /// Tick-mode state — the marker latch (exactly one marker line per
    /// process lifetime) and the per-tick record id.
    private final AtomicBoolean markerEmitted = new AtomicBoolean(false);
    private final AtomicLong tickCounter = new AtomicLong(0);

    @Produces
    @Named("benchBody")
    Processor benchBody() {
        // Canonical body prebuilt ONCE at producer time (= startup,
        // before the first tick; digest logged once — per-exchange SHA
        // printing would spam 10000 lines), then set per exchange by a
        // quiet bean — the standalone yaml sibling's AppYaml idiom
        // (task 2.4).
        final int size = benchPayloadBytes();
        final String payload = canonicalJsonBody(size, CANONICAL_SELFTEST_TICK);
        if (payload.length() != size) {
            throw new IllegalStateException(
                    "t2-json input length " + payload.length() + " != expected " + size);
        }
        System.out.println("BENCH_INPUT_SHA256=" + sha256Hex(payload));
        return setCanonicalBody(payload);
    }

    @Produces
    @Named("markStart")
    Processor markStart() {
        return markStartBean();
    }

    @Produces
    @Named("insertBench")
    Processor insertBench() {
        return insertBenchMember();
    }

    @Produces
    @Named("assertOutput")
    Processor assertOutput() {
        return assertOutputBean(benchPayloadBytes());
    }

    @Produces
    @Named("emitMarker")
    Processor emitMarker() {
        return emitMarkerBean(markerEmitted);
    }

    @Produces
    @Named("writeLatency")
    Processor writeLatency() {
        // Tick-mode latency sink — truncate at producer time (startup,
        // before any tick; no stale records leak across runs).
        final Path latencyFile = Path.of(latencyFilePath());
        try {
            Files.createDirectories(latencyFile.getParent());
            Files.writeString(latencyFile, "",
                    StandardOpenOption.CREATE, StandardOpenOption.TRUNCATE_EXISTING);
        } catch (IOException e) {
            throw new IllegalStateException(
                    "t2-json latency sink truncate failed: " + latencyFile, e);
        }
        return writeLatencyBean(latencyFile, tickCounter);
    }

    /// Bracket step: records t_start immediately after the body is set
    /// (same position as the dsl module and the lib crate's task-2.2
    /// route). Long (boxed) so it round-trips through exchange property
    /// type erasure.
    static Processor markStartBean() {
        return exchange ->
                exchange.setProperty("BenchStart", System.nanoTime());
    }

    /// Sets the prebuilt canonical body (built + asserted + digest
    /// logged ONCE at producer time). Per exchange this is the
    /// {@code setBody(constant(...))} equivalent for the YAML route.
    static Processor setCanonicalBody(String payload) {
        return exchange -> exchange.getMessage().setBody(payload);
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
    static Processor assertOutputBean(int size) {
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
    static Processor emitMarkerBean(AtomicBoolean markerEmitted) {
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
    static Processor writeLatencyBean(Path latencyFile, AtomicLong tickCounter) {
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
    /// for this cell (`${cell//\//_}` of
    /// `t2-json/camel-quarkus-yaml-native`) — the harness argv passes
    /// the path as a -D system property, so the env fallback is what
    /// makes the reader find the log (mirrors the standalone fixtures,
    /// task 2.4).
    static String latencyFilePath() {
        String env = System.getenv("BENCH_LATENCY_FILE");
        if (env == null || env.isBlank()) {
            return "/tmp/v3-protocol-b-t2-json_camel-quarkus-yaml-native.log";
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
