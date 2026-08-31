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
// - Route semantics — `timer:bench?repeatCount=1&delay=0` fires the
//   route EXACTLY ONCE, immediately (delay=0 strips Camel's 1s default
//   initial delay), with an empty initial body; then the fixture
//   idles until killed:
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
// - The marker line is printed exactly once (the harness greps -F
//   for the full string and validates the exact count). It carries
//   the post-choice body — the scenario's branch-execution proof.
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
// - After the marker the script idles like the rust fixture
//   (`tokio::signal::ctrl_c().await`): the route has fired once
//   (repeatCount=1); the smoke/harness kills the process externally.

// The timer trigger creates the exchange: empty body, no headers.
// repeatCount=1 & delay=0 — one firing, immediately.
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
// carrying the post-choice final body. Exactly once.
console.log(`BENCH_ROUTE_READY body=${ex.body}`);

// Idle like the rust fixture (`tokio::signal::ctrl_c().await`): the
// route has fired once (repeatCount=1); the smoke/harness kills the
// process externally after the marker.
setInterval(() => {}, 1 << 30);
