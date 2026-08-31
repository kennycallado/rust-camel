// T2 t2-json fixture — node-native contender (bench-node task 2.2).
// Zero-dependency Node script, protocol B (process-spawn shape, like
// every contender in this scenario).
//
// Contract (extracted from the existing contenders — rust-camel-lib
// src/main.rs, camel-standalone App.java/AppYaml.java, camel-quarkus
// BenchRoute.java, rust-camel-cli routes/t2-json.yaml):
// - Env contract: `BENCH_PAYLOAD_BYTES` (default 32768), validated
//   against the payload axis 1024|32768|262144|1048576. Invalid values
//   abort with a non-zero exit BEFORE any marker is printed (mirror of
//   the rust fixture's `validate_payload_size` panic).
// - Route semantics — `timer:bench?repeatCount=1&delay=0` fires the
//   route EXACTLY ONCE, then the fixture idles until killed:
//   set_body (canonical JSON document, exactly SIZE bytes)
//     -> BENCH_INPUT_SHA256 log
//     -> unmarshal json   (JSON.parse: the body IS the parsed value)
//     -> filter           (id == "bench")
//     -> transform        (insert "bench": true into the PARSED map)
//     -> marshal json     (the SINGLE serialization: JSON.stringify)
//     -> output assert    (exact SIZE+13 length AND parsed semantic
//                          equality) — failure exits BEFORE the marker
//     -> marker           BENCH_ROUTE_READY bytes=<len>
// - Canonical body builder: `{"id":"bench","seq":<tick>,"fill":"<K×'b'>"}`
//   with tick = 0 (CANONICAL_SELFTEST_TICK) and
//   K = SIZE - (prefix 20 + tick digits + infix 9 + suffix 2), so the
//   document is exactly SIZE bytes. This is the byte-identical JS port
//   of bench-loadgen's `payload::canonical_json_body`; the golden
//   BENCH_INPUT_SHA256 line proves it per run.
// - The +13 output delta is exactly the inserted `,"bench":true`
//   member. INPUT parity is what the digest proves — cross-runtime
//   OUTPUT byte-parity is NOT claimed (serializers differ in member
//   order; the scenario README documents the caveat).
// - The marker line is printed exactly once (the harness greps -F and
//   validates the exact count). After the marker the script idles like
//   the rust fixture (`ctrl_c().await`) — the smoke/harness kills it
//   externally; everything here is ASCII so JS string length equals
//   the UTF-8 byte length throughout.

import { createHash } from "node:crypto";

const VALID_PAYLOAD_SIZES = [1024, 32768, 262144, 1048576];

// Canonical body constants — byte-for-byte the payload.rs values.
const CANONICAL_SELFTEST_TICK = 0;
const CANONICAL_PREFIX = '{"id":"bench","seq":'; // 20 bytes
const CANONICAL_FILL_INFIX = ',"fill":"'; // 9 bytes
const CANONICAL_SUFFIX = '"}'; // 2 bytes

// Exact byte delta added by the transform: the `,"bench":true` member.
const BENCH_MEMBER_DELTA = 13;

// Mirror of bench-loadgen `payload::canonical_json_body`: builds the
// canonical JSON document of exactly `size` bytes for `(size, tick)`.
function canonicalJsonBody(size, tick) {
  const tickStr = String(tick);
  const overhead =
    CANONICAL_PREFIX.length +
    tickStr.length +
    CANONICAL_FILL_INFIX.length +
    CANONICAL_SUFFIX.length;
  if (size < overhead) {
    throw new Error(
      `canonical JSON body needs at least ${overhead} bytes for tick ${tick}, got ${size}`,
    );
  }
  const fill = "b".repeat(size - overhead);
  return (
    CANONICAL_PREFIX + tickStr + CANONICAL_FILL_INFIX + fill + CANONICAL_SUFFIX
  );
}

function sha256Hex(text) {
  return createHash("sha256").update(Buffer.from(text, "utf8")).digest("hex");
}

// Resolve BENCH_PAYLOAD_BYTES (default 32768), validated against the
// payload axis. Invalid values abort before any marker is printed.
// Strict like the rust fixture's `parse::<usize>()`: only pure decimal
// digits are accepted — Number() would silently coerce "32768.0",
// "0x8000", or "3.2768e4" into a valid size.
function benchPayloadBytes() {
  const raw = process.env.BENCH_PAYLOAD_BYTES;
  if (raw === undefined) {
    return 32768;
  }
  const trimmed = raw.trim();
  if (!/^\d+$/.test(trimmed)) {
    throw new Error(
      `BENCH_PAYLOAD_BYTES='${raw}' is not a usize; valid sizes: [${VALID_PAYLOAD_SIZES}]`,
    );
  }
  const parsed = Number(trimmed);
  if (!VALID_PAYLOAD_SIZES.includes(parsed)) {
    throw new Error(
      `BENCH_PAYLOAD_BYTES invalid payload size ${parsed}: must be one of ${VALID_PAYLOAD_SIZES.join(", ")} (bytes)`,
    );
  }
  return parsed;
}

// Output assert — both invariants: exact `size + 13` length AND parsed
// semantic equality (`id == "bench"`, `seq` present, `fill` all 'b',
// `bench == true`). Throws on any violation so the route fails BEFORE
// the marker (mirror of the rust `assert_bench_output`).
function assertBenchOutput(size, text) {
  const expected = size + BENCH_MEMBER_DELTA;
  if (text.length !== expected) {
    throw new Error(
      `t2-json output length ${text.length} != expected ${expected}`,
    );
  }
  const obj = JSON.parse(text);
  if (obj.id !== "bench") {
    throw new Error('t2-json output id != "bench"');
  }
  if (!("seq" in obj)) {
    throw new Error("t2-json output seq member missing");
  }
  const fill = obj.fill;
  if (typeof fill !== "string") {
    throw new Error("t2-json output fill member missing or non-string");
  }
  if (!/^b+$/.test(fill)) {
    throw new Error("t2-json output fill is not all 'b'");
  }
  if (obj.bench !== true) {
    throw new Error("t2-json output bench != true");
  }
  return expected;
}

try {
  const size = benchPayloadBytes();
  const body = canonicalJsonBody(size, CANONICAL_SELFTEST_TICK);

  // Input provenance: a pure function of (size, tick), identical
  // across every t2-json contender (README golden table).
  console.log(`BENCH_INPUT_SHA256=${sha256Hex(body)}`);

  // unmarshal json: the body IS the parsed value from here on.
  const parsed = JSON.parse(body);

  // filter: id == "bench" (a non-matching document fails the route —
  // no transform, no marker).
  if (parsed.id !== "bench") {
    throw new Error('t2-json filter: id != "bench"');
  }

  // transform: insert the member into the PARSED map and keep the map
  // (never a re-serialized string — marshal is the single serializer).
  parsed.bench = true;

  // marshal json: the SINGLE serialization (JSON.stringify).
  const out = JSON.stringify(parsed);

  // output assert, then the marker (exactly once).
  const len = assertBenchOutput(size, out);
  console.log(`BENCH_ROUTE_READY bytes=${len}`);
} catch (err) {
  console.error(`error: ${err.message}`);
  process.exit(1);
}

// Idle like the rust fixture (`tokio::signal::ctrl_c().await`): the
// route has fired once (repeatCount=1); the smoke/harness kills the
// process externally after the marker.
setInterval(() => {}, 1 << 30);
