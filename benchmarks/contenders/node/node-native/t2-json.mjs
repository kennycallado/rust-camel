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
// - Route semantics — the warm-tick timer form
//   `timer:bench?period=10&repeatCount=10000&delay=0` (bench-consol-tick
//   task 2.6 + conductor first-fire ruling): the SAME per-exchange
//   pipeline fires IMMEDIATELY at t0, then every 10 ms (10000
//   exchanges total), then the fixture idles until killed:
//   set_body (canonical JSON document, exactly SIZE bytes)
//     -> WINDOW START     (t0 — AFTER the body supply, which is
//                          EXCLUDED from the window, e_opus ruling D3)
//     -> unmarshal json   (JSON.parse: the body IS the parsed value)
//     -> filter           (id == "bench")
//     -> transform        (insert "bench": true into the PARSED map)
//     -> marshal json     (the SINGLE serialization: JSON.stringify)
//     -> WINDOW CLOSE     (duration = now − t0; BENCH_LATENCY append)
//     -> output assert    (exact SIZE+13 length AND parsed semantic
//                          equality) — OUTSIDE the window (e_opus
//                          ruling D4); failure exits non-zero BEFORE
//                          the marker (after that tick's record)
//     -> marker           BENCH_ROUTE_READY bytes=<len>, latched to
//                          the FIRST completed exchange
// - Tick mode protocol B: every tick brackets the CORE pipeline (t0
//   after the body supply, record immediately after marshal) and
//   appends `BENCH_LATENCY <tick> <duration_ns>` to the latency file —
//   one record per tick = one full pipeline. The file path comes from
//   `BENCH_LATENCY_FILE` (set EXPLICITLY per cell by the harness node
//   wiring; the canonical path below is only a standalone-run
//   fallback) and is truncated at startup.
// - Canonical body: `{"id":"bench","seq":<tick>,"fill":"<K×'b'>"}` with
//   K = SIZE - (prefix 20 + tick digits + infix 9 + suffix 2). Built
//   ONCE at startup with seq frozen at CANONICAL_SELFTEST_TICK — the
//   rust lib's frozen-constant shape — and reused verbatim every tick,
//   so the measured window contains only exchange processing (matching
//   lib/JVM). The startup BENCH_INPUT_SHA256 golden is that same body
//   — the same digest every contender logs.
// - The +13 output delta is exactly the inserted `,"bench":true`
//   member. INPUT parity is what the digest proves — cross-runtime
//   OUTPUT byte-parity is NOT claimed (serializers differ in member
//   order; the scenario README documents the caveat).
// - The marker line is printed exactly once — latched to the FIRST
//   completed exchange, at its original code-path position (after the
//   output assert; both are OUTSIDE the measured window, so the first
//   tick's latency record precedes the marker — the marker is a
//   trailing-log concern per e_opus ruling D3). After repeatCount the
//   script idles like the rust fixture (`ctrl_c().await`) — the
//   smoke/harness kills it externally; everything here is ASCII so JS
//   string length equals the UTF-8 byte length throughout.

import { appendFileSync, mkdirSync, writeFileSync } from "node:fs";
import { dirname } from "node:path";
import { createHash } from "node:crypto";

const VALID_PAYLOAD_SIZES = [1024, 32768, 262144, 1048576];

// Canonical body constants — byte-for-byte the payload.rs values.
const CANONICAL_SELFTEST_TICK = 0;
const CANONICAL_PREFIX = '{"id":"bench","seq":'; // 20 bytes
const CANONICAL_FILL_INFIX = ',"fill":"'; // 9 bytes
const CANONICAL_SUFFIX = '"}'; // 2 bytes

// Exact byte delta added by the transform: the `,"bench":true` member.
const BENCH_MEMBER_DELTA = 13;

// timer:bench?period=10&repeatCount=10000&delay=0 — period/repeatCount
// identical across all tick-mode fixtures; delay=0 is the immediate
// first fire below.
const PERIOD_MS = 10;
const REPEAT_COUNT = 10000;

// Tick-mode latency sink — read ONCE at startup. The harness node
// wiring injects BENCH_LATENCY_FILE per cell; the canonical path
// mirrors the ${scenario}_${member} cell the M2 protocol-B reader
// derives, so a standalone run still lands where the reader looks.
const latencyFile =
  process.env.BENCH_LATENCY_FILE ??
  "/tmp/v3-protocol-b-t2-json_node-native.log";

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
// the marker and the tick's latency record (mirror of the rust
// `assert_bench_output`).
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

// One exchange through the CORE pipeline (everything INSIDE the
// measured window): unmarshal json -> filter -> transform -> marshal
// json. Returns the marshaled output string; the assert runs OUTSIDE
// the window (e_opus ruling D4). Throws on any pipeline violation.
function runTickPipeline(body) {

  // unmarshal json: the body IS the parsed value from here on.
  const parsed = JSON.parse(body);

  // filter: id == "bench" (a non-matching document fails the route).
  if (parsed.id !== "bench") {
    throw new Error('t2-json filter: id != "bench"');
  }

  // transform: insert the member into the PARSED map and keep the map
  // (never a re-serialized string — marshal is the single serializer).
  parsed.bench = true;

  // marshal json: the SINGLE serialization (JSON.stringify).
  return JSON.stringify(parsed);
}

// Marker latch — the FIRST completed exchange prints the marker at its
// original code-path position (after the post-window assert, itself
// after the window-close latency record); later exchanges are silent.
let markerFired = false;
function emitReadyMarker(len) {
  if (markerFired) {
    return;
  }
  markerFired = true;
  console.log(`BENCH_ROUTE_READY bytes=${len}`);
}

// Route start: resolve env, truncate the latency sink, log the golden
// input digest — all BEFORE the loop (invalid values abort before any
// marker; a tick failure after this point exits non-zero).
let size;
let tickBody;
try {
  size = benchPayloadBytes();

  // The per-tick body, built ONCE (seq frozen at CANONICAL_SELFTEST_TICK
  // — the rust lib's frozen-constant shape): every measured tick parses
  // THIS string; the build itself stays outside the measured window.
  tickBody = canonicalJsonBody(size, CANONICAL_SELFTEST_TICK);

  // Latency file: truncate at startup like every tick-mode fixture
  // (JVM TRUNCATE_EXISTING write of "", rust File::create).
  mkdirSync(dirname(latencyFile), { recursive: true });
  writeFileSync(latencyFile, "");

  // Input provenance: a pure function of (size, tick), identical
  // across every t2-json contender (README golden table) — logged
  // once, for the same startup-built body reused every tick.
  console.log(`BENCH_INPUT_SHA256=${sha256Hex(tickBody)}`);
} catch (err) {
  console.error(`error: ${err.message}`);
  process.exit(1);
}

// Per-tick work: body supply FIRST (outside the window), t0, core
// pipeline, window CLOSE (BENCH_LATENCY record), THEN the post-window
// assert + marker. A pipeline or assert failure aborts the process
// non-zero (assert failure: after that tick's record, before the
// marker — same order as the Java peers).
let tick = 0;
function fireTick() {
  tick += 1;
  // Body supply — set_body equivalent (prebuilt tickBody; the build
  // stays outside the measured window, e_opus ruling D3).
  const body = tickBody;
  const t0 = process.hrtime.bigint();
  let out;
  try {
    out = runTickPipeline(body);
  } catch (err) {
    console.error(`error: t2-json tick ${tick} failed: ${err.message}`);
    process.exit(1);
  }
  const durationNs = Number(process.hrtime.bigint() - t0);
  appendFileSync(latencyFile, `BENCH_LATENCY ${tick} ${durationNs}\n`);
  let len;
  try {
    len = assertBenchOutput(size, out);
  } catch (err) {
    console.error(`error: t2-json tick ${tick} assert failed: ${err.message}`);
    process.exit(1);
  }
  emitReadyMarker(len);
  if (tick < REPEAT_COUNT) {
    setTimeout(fireTick, PERIOD_MS);
  }
}

// First fire IMMEDIATELY (t0 = now — the delay=0 first-fire parity
// ruling), then fixed 10 ms cadence; repeatCount exhausted -> idle
// until killed, like ctrl_c().await.
fireTick();
setInterval(() => {}, 1 << 30);
