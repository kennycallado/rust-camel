// T2 split-aggregate fixture — node-fastify contender (bench-node
// task 2.3). Same protocol-B contract as ../node-native/split-aggregate.mjs
// (see its header for the full extraction — fixed 591-byte canonical
// array built ONCE at startup, sequential split, hand-rolled correlation
// buckets with completion-reset, pending sentinel, completion assert,
// golden BENCH_INPUT_SHA256, the warm-tick
// timer:bench?period=10&repeatCount=10000&delay=0 shape (immediate
// first fire), BENCH_LATENCY records on the BENCH_LATENCY_FILE sink,
// marker latched to the FIRST completed bucket), with the Fastify
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
//   the startup-built array → unmarshal → sequential split →
//   aggregate → completion assert), bracketed t0 → record, one
//   `BENCH_LATENCY <tick> <duration_ns>` per tick — marker exactly
//   once from the first completed bucket (tick 1, fragment 100),
//   before its record — followed by the same idle-until-killed
//   lifecycle.

import { appendFileSync, mkdirSync, writeFileSync } from "node:fs";
import { dirname } from "node:path";
import { createHash } from "node:crypto";
import Fastify from "fastify";

// Canonical array cardinality — and the aggregator completion size.
const BENCH_ITEMS = 100;

// Constant correlation header value/name: every fragment aggregates
// into the same bucket (set_header at the head of the agg route).
const BENCH_CORRELATION = "bench";
const CORRELATION_HEADER = "bench.correlation";

// Exchange property stamped by the completion-assert step AFTER the
// aggregated collection passed the length assert. The marker reads
// THIS property — never the raw aggregator state.
const AGGREGATED_SIZE_PROPERTY = "bench.aggregated.size";

// Aggregator contract properties (camel-processor aggregator.rs).
const CAMEL_AGGREGATED_SIZE = "CamelAggregatedSize";
const CAMEL_AGGREGATOR_PENDING = "CamelAggregatorPending";

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
  "/tmp/v3-protocol-b-split-aggregate_node-fastify.log";

// Mirror of bench-loadgen `payload::canonical_split_aggregate_array`.
function canonicalSplitArray() {
  const items = Array.from({ length: BENCH_ITEMS }, (_, i) => `"b${i}"`);
  return `[${items.join(",")}]`;
}

function sha256Hex(text) {
  return createHash("sha256").update(Buffer.from(text, "utf8")).digest("hex");
}

// Correlation buckets, keyed by the constant correlation header value —
// the hand-rolled aggregator (see ../node-native for the full
// contract comments). One bucket per tick, completed (and reset) at
// exactly BENCH_ITEMS fragments.
const buckets = new Map();

async function directAggIn(fragment) {
  fragment.headers[CORRELATION_HEADER] = BENCH_CORRELATION;
  const key = fragment.headers[CORRELATION_HEADER];
  let bucket = buckets.get(key);
  if (bucket === undefined) {
    bucket = [];
    buckets.set(key, bucket);
  }
  // AggregationStrategy::CollectAll — the list-append strategy.
  bucket.push(fragment.body);

  if (bucket.length < BENCH_ITEMS) {
    return {
      headers: {},
      body: null,
      properties: { [CAMEL_AGGREGATOR_PENDING]: true },
    };
  }

  // Completion closes the bucket (reset) — tick N+1 starts empty.
  buckets.delete(key);
  return {
    headers: {},
    body: [...bucket],
    properties: { [CAMEL_AGGREGATED_SIZE]: bucket.length },
  };
}

function completionAssert(ex) {
  if (ex.properties[CAMEL_AGGREGATOR_PENDING] === true) {
    return ex;
  }
  const arr = ex.body;
  if (!Array.isArray(arr)) {
    throw new Error("split-aggregate completion body is not a JSON array");
  }
  if (arr.length !== BENCH_ITEMS) {
    throw new Error(
      `split-aggregate aggregated collection length ${arr.length} != ${BENCH_ITEMS}`,
    );
  }
  const reported = ex.properties[CAMEL_AGGREGATED_SIZE];
  if (typeof reported !== "number" || reported !== arr.length) {
    throw new Error(
      `split-aggregate CamelAggregatedSize ${reported} != collection length ${arr.length}`,
    );
  }
  ex.properties[AGGREGATED_SIZE_PROPERTY] = arr.length;
  return ex;
}

// Marker step — fires ONLY from the completion path and only ONCE,
// latched to the FIRST completed bucket (tick mode completes one
// bucket per tick).
let markerFired = false;
function emitReadyMarker(ex) {
  if (
    !markerFired &&
    ex.properties[AGGREGATED_SIZE_PROPERTY] === BENCH_ITEMS
  ) {
    markerFired = true;
    console.log(`BENCH_ROUTE_READY items=${BENCH_ITEMS}`);
  }
}

// One exchange through the route: set_body (the startup-built
// canonical array) -> unmarshal json -> split SEQUENTIAL
// (parallel: false): the await in this loop IS the split
// scope — one fragment after another, each response fully processed
// before the next dispatch.
async function runTickPipeline() {
  const array = tickBody;

  // unmarshal json: the body IS the parsed value from here on.
  const items = JSON.parse(array);
  if (!Array.isArray(items)) {
    throw new Error("split-aggregate: canonical body did not parse to a JSON array");
  }

  for (const item of items) {
    const fragment = { headers: {}, body: item };
    const out = await directAggIn(fragment);
    emitReadyMarker(completionAssert(out));
  }
}

// Route start: the Fastify boot FIRST (framework tax before the
// marker), then latency-sink truncation and the golden input digest —
// before the loop.
const tickBody = canonicalSplitArray();
try {
  const app = Fastify();

  // Registered before the boot so route compilation lands inside
  // ready(), like a real application. It is never served: nothing is
  // bound and this scenario has no request phase.
  app.all("/bench", async () => "BENCH_ROUTE_READY");

  // Full avvio boot — plugin loading, route compilation, handler
  // finalization — without binding any socket.
  await app.ready();

  // Latency file: truncate at startup like every tick-mode fixture
  // (JVM TRUNCATE_EXISTING write of "", rust File::create).
  mkdirSync(dirname(latencyFile), { recursive: true });
  writeFileSync(latencyFile, "");

  // Input provenance: identical bytes across every split-aggregate
  // contender (README golden table). Logged once, before the loop —
  // the same startup-built array reused every tick.
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
  runTickPipeline().then(
    () => {
      const durationNs = Number(process.hrtime.bigint() - t0);
      appendFileSync(latencyFile, `BENCH_LATENCY ${tick} ${durationNs}\n`);
      if (tick < REPEAT_COUNT) {
        setTimeout(fireTick, PERIOD_MS);
      }
    },
    (err) => {
      console.error(`error: split-aggregate tick ${tick} failed: ${err.message}`);
      process.exit(1);
    },
  );
}

// First fire IMMEDIATELY (t0 = now — the delay=0 first-fire parity
// ruling), then fixed 10 ms cadence; repeatCount exhausted -> idle
// until killed, like the rust fixture.
fireTick();
setInterval(() => {}, 1 << 30);
