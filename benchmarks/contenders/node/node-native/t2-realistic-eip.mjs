// T2 t2-realistic-eip fixture — node-native contender (bench-node
// task 2.4). Zero-dependency Node script, protocol B (process-spawn
// shape, like every contender in this scenario).
//
// Contract (extracted from the five existing contenders —
// rust-camel-lib src/main.rs, rust-camel-cli routes/t2-realistic-eip.yaml,
// camel-standalone App.java + routes.yaml, camel-quarkus BenchRoute.java;
// the scenario has NO README and NO smoke dir):
// - NO env contract: the fixture sets its OWN body (no payload axis,
//   no BENCH_INPUT_SHA256 — unlike t2-json / split-aggregate). The
//   harness exports BENCH_PAYLOAD_BYTES globally; the rust fixture
//   never reads it and neither does this one.
// - Route semantics — the warm-tick timer form
//   `timer:bench?period=10&repeatCount=10000&delay=0` (bench-consol-tick
//   task 2.6 + conductor first-fire ruling): the SAME EIP pipeline
//   fires IMMEDIATELY at t0, then every 10 ms (10000 exchanges
//   total), then the fixture idles until killed. Per exchange (fresh,
//   empty body and no headers — the timer trigger creates the
//   exchange):
//     set_body     constant "ping"
//     set_header   source = "bench"
//     filter       simple("${body} == 'ping'")   (always true here —
//                  the body was set to the literal "ping" one step
//                  earlier; evaluated, not assumed)
//       choice
//         when     simple("${header.source} == 'bench'")
//                  -> set_body "pong-bench"    (the taken branch)
//         otherwise -> set_body "pong-other"   (a wrong-branch run
//                    is observable in the marker, not silent)
//     log          "BENCH_ROUTE_READY body=${body}"
//                -> BENCH_ROUTE_READY body=pong-bench   (the marker)
// - Tick mode protocol B: every tick brackets the WHOLE per-tick body
//   (t0 before set_body, record after the log step) and appends
//   `BENCH_LATENCY <tick> <duration_ns>` to the latency file — one
//   record per tick = one full EIP pipeline. The file path comes from
//   `BENCH_LATENCY_FILE` (set EXPLICITLY per cell by the harness node
//   wiring; the canonical path below is only a standalone-run
//   fallback) and is truncated at startup.
// - The marker line is printed exactly once — latched to the FIRST
//   completed exchange, at its original code-path position (the log
//   step, carrying the post-choice body — the scenario's
//   branch-execution proof) and BEFORE that tick's latency record, so
//   the first record strictly follows the marker (the cross-runtime
//   idiom; the harness greps -F for the full string and validates the
//   exact count).
// - Divergence from rust-camel-lib, documented per task discipline:
//   the reference emits TWO lines — a static `BENCH_ROUTE_READY
//   exchange_id=<uuid>` (rust-camel's log step is static-only, a
//   v1-baseline pairing) followed by the dynamic marker. That static
//   line is a rust-camel builder workaround, not chain semantics;
//   the three Simple/YAML/Java fixtures (cli, standalone-dsl/yaml,
//   quarkus) emit ONLY the dynamic log line, which is the shape
//   mirrored here. The harness contract (grep -cF of the full
//   marker string == 1) is identical either way.
// - Structural note: the choice is nested INSIDE the filter scope
//   (the shape of the YAML and both Java fixtures); rust-camel-lib's
//   builder places an empty `.end_filter()` before `.choice()` (an
//   API artifact — `choice` is on RouteBuilder, not FilterBuilder).
//   Under this route the filter predicate is always true, so the two
//   shapes are observationally identical; the log step is outside
//   the filter in every fixture.
// - After repeatCount the script idles like the rust fixture
//   (`tokio::signal::ctrl_c().await`): the smoke/harness kills the
//   process externally.

import { appendFileSync, mkdirSync, writeFileSync } from "node:fs";
import { dirname } from "node:path";

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
  "/tmp/v3-protocol-b-t2-realistic-eip_node-native.log";

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

  // filter: simple("${body} == 'ping'") — evaluated against the
  // exchange, not hardcoded true (a non-matching body skips the
  // choice block entirely; the log below stays unconditional, exactly
  // as in every fixture).
  if (ex.body === "ping") {
    // choice — when: simple("${header.source} == 'bench'").
    if (ex.headers["source"] === "bench") {
      // when branch (the one this route always takes).
      ex.body = "pong-bench";
    } else {
      // otherwise branch — reachable only if the header predicate
      // fails; the marker would then read body=pong-other and the
      // harness grep would miss (observable wrong-branch failure).
      ex.body = "pong-other";
    }
  }

  // log: "BENCH_ROUTE_READY body=${body}" — the single dynamic line
  // carrying the post-choice final body. Latched to the FIRST
  // completed exchange.
  logStep(ex);
}

// Latency file: truncate at startup like every tick-mode fixture (JVM
// TRUNCATE_EXISTING write of "", rust File::create). No env contract
// and no digest axis for this scenario — nothing else runs before the
// loop.
mkdirSync(dirname(latencyFile), { recursive: true });
writeFileSync(latencyFile, "");

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
// until killed, like ctrl_c().await.
fireTick();
setInterval(() => {}, 1 << 30);
