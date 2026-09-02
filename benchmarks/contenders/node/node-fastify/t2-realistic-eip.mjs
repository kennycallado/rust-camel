// T2 t2-realistic-eip fixture — node-fastify contender (bench-node
// task 2.4). Same protocol-B contract as ../node-native/t2-realistic-eip.mjs
// (see its header for the full extraction — no env contract, no
// payload: the fixture sets its own body; set_body "ping" ->
// set_header source=bench -> filter "${body} == 'ping'" { choice:
// when "${header.source} == 'bench'" -> "pong-bench", otherwise ->
// "pong-other" } -> log "BENCH_ROUTE_READY body=${body}"; the
// warm-tick timer:bench?period=10&repeatCount=10000&delay=0 shape
// (immediate first fire), BENCH_LATENCY
// records on the BENCH_LATENCY_FILE sink, marker latched to the FIRST
// completed exchange), with the Fastify application booted in front:
// - Module import + `fastify()` construction + route registration +
//   `await app.ready()` run WITHOUT binding any socket — protocol B
//   has no wire protocol (the no-bind rule: a protocol-B fixture
//   binds nothing). `ready()` is the load-bearing call (task 2.1
//   lesson): it drives the full avvio boot (plugin loading, route
//   compilation, handler finalization) that every co-contender pays
//   before its marker (rust ctx.start().await, Camel Main.run).
//   Registration alone would skip exactly the framework tax this
//   cell measures. The boot lands BEFORE the marker, like the rust
//   fixture's ctx.start() before the timer fires.
// - Then the same warm-tick route (period=10, repeatCount=10000,
//   delay=0 — immediate first fire): per tick the FULL EIP pipeline on
//   a fresh exchange, bracketed t0 → record, one
//   `BENCH_LATENCY <tick> <duration_ns>` per tick — marker exactly
//   once on the first completed tick, before its record — followed by
//   the same idle-until-killed lifecycle.

import { appendFileSync, mkdirSync, writeFileSync } from "node:fs";
import { dirname } from "node:path";
import Fastify from "fastify";

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
  "/tmp/v3-protocol-b-t2-realistic-eip_node-fastify.log";

// Marker latch — the FIRST completed exchange prints the marker at its
// original code-path position (the log step), before that tick's
// latency record; later exchanges are silent.
let markerFired = false;
function logStep(ex) {
  if (markerFired) {
    return;
  }
  markerFired = true;
  console.log(`BENCH_ROUTE_READY body=${ex.body}`);
}

// One exchange through the route: the timer trigger creates the
// exchange (empty body, no headers), then set_body -> set_header ->
// filter -> choice -> log.
function runTickPipeline() {
  const ex = { body: "", headers: {} };

  // set_body: constant "ping".
  ex.body = "ping";

  // set_header: source = "bench".
  ex.headers["source"] = "bench";

  // filter: simple("${body} == 'ping'") — evaluated, not assumed.
  if (ex.body === "ping") {
    // choice — when: simple("${header.source} == 'bench'").
    if (ex.headers["source"] === "bench") {
      ex.body = "pong-bench";
    } else {
      ex.body = "pong-other";
    }
  }

  // log: "BENCH_ROUTE_READY body=${body}" — the single dynamic line
  // carrying the post-choice final body. Latched to the FIRST
  // completed exchange.
  logStep(ex);
}

// Route start: the Fastify boot FIRST (framework tax before the
// marker), then latency-sink truncation — before the loop. No env
// contract and no digest axis for this scenario — nothing else runs.
try {
  const app = Fastify();

  // Registered before the boot so route compilation lands inside
  // ready(), like a real application. It is never served: nothing is
  // bound and this scenario has no request phase.
  app.all("/bench", async () => "BENCH_ROUTE_READY body=pong-bench");

  // Full avvio boot — plugin loading, route compilation, handler
  // finalization — without binding any socket.
  await app.ready();

  // Latency file: truncate at startup like every tick-mode fixture
  // (JVM TRUNCATE_EXISTING write of "", rust File::create).
  mkdirSync(dirname(latencyFile), { recursive: true });
  writeFileSync(latencyFile, "");
} catch (err) {
  console.error(`error: ${err.message}`);
  process.exit(1);
}

// Per-tick work: t0 before the FULL pipeline, BENCH_LATENCY record
// after it — one record per tick = one full per-tick pipeline.
let tick = 0;
function fireTick() {
  tick += 1;
  const t0 = process.hrtime.bigint();
  runTickPipeline();
  const durationNs = Number(process.hrtime.bigint() - t0);
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
