// T2 t2-json fixture — node-fastify contender (bench-node task 2.2).
// Same protocol-B contract as ../node-native/route.mjs (see its header
// for the full extraction — env contract, canonical body builder,
// unmarshal → filter → transform → marshal chain, SIZE+13 output
// assert, golden BENCH_INPUT_SHA256), with the Fastify application
// booted in front:
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
// - Then the same single route execution (repeatCount=1): SHA log ->
//   chain -> output assert -> marker, exactly once — followed by the
//   same idle-until-killed lifecycle.

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
// equality; throws BEFORE the marker on any violation.
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
  const app = Fastify();

  // Registered before the boot so route compilation lands inside
  // ready(), like a real application. It is never served: nothing is
  // bound and this scenario has no request phase.
  app.all("/bench", async () => "BENCH_ROUTE_READY");

  // Full avvio boot — plugin loading, route compilation, handler
  // finalization — without binding any socket.
  await app.ready();

  // Route execution (timer:bench?repeatCount=1&delay=0 — fires once):
  const size = benchPayloadBytes();
  const body = canonicalJsonBody(size, CANONICAL_SELFTEST_TICK);

  // Input provenance: identical bytes across every t2-json contender.
  console.log(`BENCH_INPUT_SHA256=${sha256Hex(body)}`);

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

  // output assert, then the marker (exactly once).
  const len = assertBenchOutput(size, out);
  console.log(`BENCH_ROUTE_READY bytes=${len}`);
} catch (err) {
  console.error(`error: ${err.message}`);
  process.exit(1);
}

// Idle like the rust fixture: killed externally after the marker.
setInterval(() => {}, 1 << 30);
