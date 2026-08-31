// T2 t2-realistic-eip fixture — node-fastify contender (bench-node
// task 2.4). Same protocol-B contract as ../node-native/route.mjs
// (see its header for the full extraction — no env contract, no
// payload: the fixture sets its own body; set_body "ping" ->
// set_header source=bench -> filter "${body} == 'ping'" { choice:
// when "${header.source} == 'bench'" -> "pong-bench", otherwise ->
// "pong-other" } -> log "BENCH_ROUTE_READY body=${body}"), with the
// Fastify application booted in front:
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
// - Then the same single route execution (repeatCount=1): chain ->
//   marker `BENCH_ROUTE_READY body=pong-bench` exactly once —
//   followed by the same idle-until-killed lifecycle.

import Fastify from "fastify";

const app = Fastify();

// Registered before the boot so route compilation lands inside
// ready(), like a real application. It is never served: nothing is
// bound and this scenario has no request phase.
app.all("/bench", async () => "BENCH_ROUTE_READY body=pong-bench");

// Full avvio boot — plugin loading, route compilation, handler
// finalization — without binding any socket.
await app.ready();

// The timer trigger creates the exchange: empty body, no headers.
// repeatCount=1 & delay=0 — one firing, immediately.
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
// carrying the post-choice final body. Exactly once.
console.log(`BENCH_ROUTE_READY body=${ex.body}`);

// Idle like the rust fixture: killed externally after the marker.
setInterval(() => {}, 1 << 30);
