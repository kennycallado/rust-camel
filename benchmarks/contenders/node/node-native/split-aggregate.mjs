// T2 split-aggregate fixture — node-native contender (bench-node
// task 2.3). Zero-dependency Node script, protocol B (process-spawn
// shape, like every contender in this scenario).
//
// Contract (extracted from the existing contenders — rust-camel-lib
// src/main.rs, rust-camel-cli routes/split-aggregate.yaml):
// - The input is FIXED: the canonical array body, exactly 591 bytes —
//   a compact JSON array whose 100 items are the strings "b0".."b99"
//   (item i is `"b" + i`), the byte-identical JS port of bench-loadgen's
//   `payload::canonical_split_aggregate_array`. There is NO env
//   contract: BENCH_PAYLOAD_BYTES is defined for the structured-payload
//   scenarios and is IGNORED here (the harness exports it; the rust
//   fixture never reads it either).
// - Route semantics — the warm-tick timer form
//   `timer:bench?period=10&repeatCount=10000&delay=0` (bench-consol-tick
//   task 2.6 + conductor first-fire ruling): the SAME per-exchange
//   pipeline fires IMMEDIATELY at t0, then every 10 ms (10000
//   exchanges total), then the fixture idles until killed:
//   set_body (canonical array — built ONCE at startup: the measured
//             window contains only exchange processing, matching
//             lib/JVM)
//     -> BENCH_INPUT_SHA256 log   (golden, tick-independent — logged
//                                  ONCE at startup, before the loop)
//     -> unmarshal json           (JSON.parse: the parsed 100-item
//                                  array — a split on a TEXT body is
//                                  a silent no-op, so the parse result
//                                  MUST be an array; failure exits
//                                  BEFORE marker and record)
//     -> split (SEQUENTIAL)       one fragment after another, each
//                                 dispatched to direct:agg-in and its
//                                 response fully processed before the
//                                 next dispatch (parallel(false))
//         -> direct:agg-in        set_header (bench.correlation =
//                                 "bench", constant correlation key)
//                                 -> aggregate (completion_size=100,
//                                    CollectAll = list-append,
//                                    force_completion_on_stop=false;
//                                    completion RESETS the bucket —
//                                    the runtime's bucket-close
//                                    semantics — so the next tick
//                                    starts an empty one)
//                                 -> completion assert (collection
//                                    length == 100 AND consistent with
//                                    CamelAggregatedSize; set
//                                    bench.aggregated.size = 100)
//                                 -> marker ONLY from that path
//     -> marker                   BENCH_ROUTE_READY items=100, latched
//                                 to the FIRST completed bucket
// - Tick mode protocol B: every tick brackets the WHOLE per-tick body
//   (t0 before set_body, record after the split loop) and appends
//   `BENCH_LATENCY <tick> <duration_ns>` to the latency file — one
//   record per tick = one full split+aggregate pipeline. The file path
//   comes from `BENCH_LATENCY_FILE` (set EXPLICITLY per cell by the
//   harness node wiring; the canonical path below is only a
//   standalone-run fallback) and is truncated at startup. One bucket
//   completes per tick; the marker fires inside tick 1 (fragment 100's
//   completion) and the record lands after the tick body, so the first
//   record strictly follows the marker (the cross-runtime idiom).
// - Hand-rolled async coordination is the HONEST POINT of this
//   contender: the aggregator below (correlation buckets, pending
//   sentinel, completion on the 100th fragment) is ~30 lines of plain
//   JS — deliberately NOT an orchestration library, zero dependencies.
// - Pending sentinel: no timeout and no force-completion, so fragments
//   1..99 of the bucket answer `{CamelAggregatorPending: true}` with an
//   empty body and flow through the SAME completion-assert/marker steps
//   (both guard on the properties) — an incomplete bucket can never
//   produce the marker.
// - The marker line is printed exactly once (the harness greps -F and
//   validates the exact count). After repeatCount the script idles
//   like the rust fixture (`ctrl_c().await`) — the smoke/harness kills
//   it externally. Everything here is ASCII, so JS string length
//   equals the UTF-8 byte length throughout.

import { appendFileSync, mkdirSync, writeFileSync } from "node:fs";
import { dirname } from "node:path";
import { createHash } from "node:crypto";

// Canonical array cardinality — and the aggregator completion size.
const BENCH_ITEMS = 100;

// Constant correlation header value/name: every fragment aggregates
// into the same bucket (set_header at the head of the agg route).
const BENCH_CORRELATION = "bench";
const CORRELATION_HEADER = "bench.correlation";

// Exchange property stamped by the completion-assert step AFTER the
// aggregated collection passed the length assert. The marker reads
// THIS property — never the raw aggregator state — so an incomplete
// bucket can never produce the marker.
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
  "/tmp/v3-protocol-b-split-aggregate_node-native.log";

// Mirror of bench-loadgen `payload::canonical_split_aggregate_array`:
// a compact JSON array of BENCH_ITEMS string items, item `i` being
// `"b<i>"`, zero whitespace — exactly 591 bytes.
function canonicalSplitArray() {
  const items = Array.from({ length: BENCH_ITEMS }, (_, i) => `"b${i}"`);
  return `[${items.join(",")}]`;
}

function sha256Hex(text) {
  return createHash("sha256").update(Buffer.from(text, "utf8")).digest("hex");
}

// Correlation buckets, keyed by the constant correlation header value —
// the hand-rolled aggregator. One bucket per tick ("bench"), completed
// (and reset) at exactly BENCH_ITEMS fragments.
const buckets = new Map();

// direct:agg-in — set_header, then aggregate. A fragment dispatched
// here gets its OWN response: the pending sentinel for fragments
// 1..99, the completion payload (aggregated Body::Json array +
// CamelAggregatedSize = bucket length) for the completing fragment.
// Completion RESETS the bucket (the runtime closes it), so the next
// tick aggregates into a fresh one.
async function directAggIn(fragment) {
  // set_header: constant correlation key (deliberate pairing
  // asymmetry is a rust/CLI detail — here, like the lib route, the
  // header IS the correlation key).
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
    // Not yet complete: pending sentinel (empty body). No timeout and
    // no force-completion exist, so this bucket has no other
    // completion path.
    return {
      headers: {},
      body: null,
      properties: { [CAMEL_AGGREGATOR_PENDING]: true },
    };
  }

  // Completion: CollectAll output is one JSON array of every fragment
  // body, carrying CamelAggregatedSize = bucket length. The bucket
  // closes here (reset) — tick N+1 starts empty.
  buckets.delete(key);
  return {
    headers: {},
    body: [...bucket],
    properties: { [CAMEL_AGGREGATED_SIZE]: bucket.length },
  };
}

// COMPLETION ASSERT — pending sentinels pass through untouched; the
// completion payload must be a BENCH_ITEMS-length array consistent
// with its own CamelAggregatedSize. Throws BEFORE the marker and the
// tick's latency record on any violation (mirror of the rust
// `completion_assert`).
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

// Marker step — fires ONLY from the completion path: it reads the
// `bench.aggregated.size` property set by completionAssert, so a
// pending sentinel (incomplete bucket) can never produce the marker
// (mirror of the rust `emit_ready_marker`) — and only ONCE, latched to
// the FIRST completed bucket (tick mode completes one bucket per tick).
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
// (parallel: false): fragment i is dispatched to
// direct:agg-in and its response fully processed (aggregate ->
// completion assert -> marker guard) before fragment i+1 is
// dispatched — the await in this loop IS the split scope.
async function runTickPipeline() {
  const array = tickBody;

  // unmarshal json: the body IS the parsed value from here on. A split
  // on a text body is a silent no-op (zero fragments), so the parsed
  // body MUST be an array — enforced, not assumed.
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

// The per-tick canonical array — built once at startup, reused
// verbatim every tick.
let tickBody;

try {
  // Latency file: truncate at startup like every tick-mode fixture
  // (JVM TRUNCATE_EXISTING write of "", rust File::create).
  mkdirSync(dirname(latencyFile), { recursive: true });
  writeFileSync(latencyFile, "");

  // The 591-byte body, built ONCE (tick-independent): every measured
  // tick parses THIS string; the build stays outside the measured
  // window (lib/JVM shape).
  tickBody = canonicalSplitArray();

  // Input provenance — a pure function of the fixed array, identical
  // across every split-aggregate contender (README golden table).
  // Logged once, before the loop.
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
// until killed, like ctrl_c().await.
fireTick();
setInterval(() => {}, 1 << 30);
