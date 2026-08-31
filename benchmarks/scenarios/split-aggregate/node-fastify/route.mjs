// T2 split-aggregate fixture — node-fastify contender (bench-node
// task 2.3). Same protocol-B contract as ../node-native/route.mjs
// (see its header for the full extraction — fixed 591-byte canonical
// array, sequential split, hand-rolled correlation buckets, pending
// sentinel, completion assert, golden BENCH_INPUT_SHA256), with the
// Fastify application booted in front:
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
//   sequential split -> aggregate -> completion assert -> marker,
//   exactly once — followed by the same idle-until-killed lifecycle.

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
// contract comments).
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

function emitReadyMarker(ex) {
  if (ex.properties[AGGREGATED_SIZE_PROPERTY] === BENCH_ITEMS) {
    console.log(`BENCH_ROUTE_READY items=${BENCH_ITEMS}`);
  }
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
  const array = canonicalSplitArray();

  // Input provenance: identical bytes across every split-aggregate
  // contender (README golden table).
  console.log(`BENCH_INPUT_SHA256=${sha256Hex(array)}`);

  // unmarshal json: the body IS the parsed value from here on.
  const items = JSON.parse(array);
  if (!Array.isArray(items)) {
    throw new Error("split-aggregate: canonical body did not parse to a JSON array");
  }

  // split SEQUENTIAL (parallel: false): the await in this loop IS the
  // split scope — one fragment after another, each response fully
  // processed before the next dispatch.
  for (const item of items) {
    const fragment = { headers: {}, body: item };
    const out = await directAggIn(fragment);
    emitReadyMarker(completionAssert(out));
  }
} catch (err) {
  console.error(`error: ${err.message}`);
  process.exit(1);
}

// Idle like the rust fixture: killed externally after the marker.
setInterval(() => {}, 1 << 30);
