// T2 t2-json fixture — node-fastify contender (bench-node task 2.2).
// Same protocol-B contract as ../node-native/t2-json.mjs (see its header
// for the full extraction — env contract, canonical body builder,
// unmarshal → filter → transform → marshal chain, SIZE+13 output
// assert, golden BENCH_INPUT_SHA256, the warm-tick
// timer:bench?period=10&repeatCount=10000&delay=0 shape (immediate
// first fire), BENCH_LATENCY records on the BENCH_LATENCY_FILE sink,
// marker latched to the FIRST completed exchange), with the Fastify
// application booted in front:
// - Module import + `fastify()` construction + route registration +
//   `await app.ready()` run WITHOUT binding any socket — protocol B
//   has no wire protocol (the no-bind rule: a protocol-B fixture
//   binds nothing). `ready()` is the load-bearing call (task 2.1
//   lesson): it drives the full avvio boot (plugin loading, route
//   compilation, handler finalization) that every co-contender pays
//   before its marker (rust ctx.start().await, Camel Main.run).
//   Registration alone would skip exactly the framework tax this cell
//   measures. The boot lands BEFORE the marker, like the rust
//   fixture's ctx.start() before the timer fires.
// - Then the same warm-tick route (period=10, repeatCount=10000,
//   delay=0 — immediate first fire): per tick the pipeline (set_body:
//   the startup-built body → unmarshal → filter → transform → marshal
//   → output assert), bracketed t0 → record, one
//   `BENCH_LATENCY <tick> <duration_ns>` per tick — marker exactly
//   once on the first completed tick, before its record — followed by
//   the same idle-until-killed lifecycle.

import { appendFileSync, mkdirSync, writeFileSync } from "node:fs";
import { dirname } from "node:path";
import { createHash } from "node:crypto";
import Fastify from "fastify";

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
  "/tmp/v3-protocol-b-t2-json_node-fastify.log";

// Mirror of bench-loadgen `payload::canonical_json_body`.
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

// Output assert — exact `size + 13` length AND parsed semantic
// equality; throws BEFORE the marker and the tick's latency record on
// any violation.
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

// One exchange through the route: set_body (the startup-built canonical
// document — frozen seq) -> unmarshal json -> filter -> transform ->
// marshal json -> output assert. Returns the asserted output length;
// throws on any violation.
function runTickPipeline(body) {

  // unmarshal json: the body IS the parsed value from here on.
  const parsed = JSON.parse(body);

  // filter: id == "bench".
  if (parsed.id !== "bench") {
    throw new Error('t2-json filter: id != "bench"');
  }

  // transform: insert the member into the PARSED map.
  parsed.bench = true;

  // marshal json: the SINGLE serialization (JSON.stringify).
  const out = JSON.stringify(parsed);

  // output assert — the marker's code-path position is right here,
  // latched below to the FIRST completed exchange.
  return assertBenchOutput(size, out);
}

// Marker latch — the FIRST completed exchange prints the marker at its
// original code-path position (after the assert), before that tick's
// latency record; later exchanges are silent.
let markerFired = false;
function emitReadyMarker(len) {
  if (markerFired) {
    return;
  }
  markerFired = true;
  console.log(`BENCH_ROUTE_READY bytes=${len}`);
}

// Route start: the Fastify boot FIRST (framework tax before the
// marker), then env resolution, latency-sink truncation, the golden
// input digest — all BEFORE the loop (invalid values abort before any
// marker; a tick failure after this point exits non-zero).
let size;
let tickBody;
try {
  const app = Fastify();

  // Registered before the boot so route compilation lands inside
  // ready(), like a real application. It is never served: nothing is
  // bound and this scenario has no request phase.
  app.all("/bench", async () => "BENCH_ROUTE_READY");

  // Full avvio boot — plugin loading, route compilation, handler
  // finalization — without binding any socket.
  await app.ready();

  size = benchPayloadBytes();

  // The per-tick body, built ONCE (seq frozen at CANONICAL_SELFTEST_TICK
  // — the rust lib's frozen-constant shape): every measured tick parses
  // THIS string; the build itself stays outside the measured window.
  tickBody = canonicalJsonBody(size, CANONICAL_SELFTEST_TICK);

  // Latency file: truncate at startup like every tick-mode fixture
  // (JVM TRUNCATE_EXISTING write of "", rust File::create).
  mkdirSync(dirname(latencyFile), { recursive: true });
  writeFileSync(latencyFile, "");

  // Input provenance: identical bytes across every t2-json contender.
  // Logged once, before the loop — the same startup-built body reused
  // every tick.
  console.log(`BENCH_INPUT_SHA256=${sha256Hex(tickBody)}`);
} catch (err) {
  console.error(`error: ${err.message}`);
  process.exit(1);
}

// Per-tick work: t0 before the FULL pipeline, BENCH_LATENCY record
// after it — one record per tick = one full per-tick pipeline. A
// pipeline failure aborts the process non-zero (no record, no marker).
let tick = 0;
function fireTick() {
  tick += 1;
  const t0 = process.hrtime.bigint();
  let len;
  try {
    len = runTickPipeline(tickBody);
  } catch (err) {
    console.error(`error: t2-json tick ${tick} failed: ${err.message}`);
    process.exit(1);
  }
  const durationNs = Number(process.hrtime.bigint() - t0);
  emitReadyMarker(len);
  appendFileSync(latencyFile, `BENCH_LATENCY ${tick} ${durationNs}\n`);
  if (tick < REPEAT_COUNT) {
    setTimeout(fireTick, PERIOD_MS);
  }
}

// First fire IMMEDIATELY (t0 = now — the delay=0 first-fire parity
// ruling), then fixed 10 ms cadence; repeatCount exhausted -> idle
// until killed, like the rust fixture.
fireTick();
setInterval(() => {}, 1 << 30);
