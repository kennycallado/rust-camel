package com.rustcamel.bench;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.ObjectMapper;
import com.fasterxml.jackson.databind.node.ObjectNode;
import java.nio.charset.StandardCharsets;
import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;
import org.apache.camel.Exchange;
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
 * <p>No self-instrumentation — timing and RSS are captured by the
 * harness from OUTSIDE this process. {@code delay=0} on the timer (v1
 * Fix 3) keeps idle wait out of the measurement.
 */
public final class App {
    private App() {
    }

    private static final ObjectMapper MAPPER = new ObjectMapper();

    public static void main(String[] args) throws Exception {
        Main main = new Main();
        final int size = benchPayloadBytes();
        main.configure().addRoutesBuilder(new RouteBuilder() {
            @Override
            public void configure() {
                from("timer:bench?repeatCount=1&delay=0")
                        .process(buildBody(size))
                        .unmarshal().json(JsonLibrary.Jackson, ObjectNode.class)
                        .filter(jsonpath("$[?(@.id == 'bench')]"))
                        .process(insertBenchMember())
                        .end()
                        .marshal().json(JsonLibrary.Jackson)
                        .process(assertOutput(size))
                        .log("BENCH_ROUTE_READY bytes=${header.benchOutLen}");
            }
        });
        main.run(args);
    }

    /// Builds the canonical JSON document for (size, tick) and logs
    /// `BENCH_INPUT_SHA256=<digest>` before any processing. The input
    /// length assert fires here so a size mismatch kills the cell
    /// before the marker.
    static Processor buildBody(int size) {
        return exchange -> {
            String body = canonicalJsonBody(size, CANONICAL_SELFTEST_TICK);
            if (body.length() != size) {
                throw new IllegalStateException(
                        "t2-json input length " + body.length() + " != expected " + size);
            }
            System.out.println("BENCH_INPUT_SHA256=" + sha256Hex(body));
            exchange.getMessage().setBody(body);
        };
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
