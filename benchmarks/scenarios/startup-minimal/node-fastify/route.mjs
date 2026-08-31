// T1 startup-minimal fixture — node-fastify contender (bench-node task 2.1).
// Same protocol-B marker contract as ../node-native/route.mjs (see its
// header for the full extraction), with the Fastify application booted
// in front:
// - Module import + `fastify()` construction + route registration +
//   `await app.ready()` run WITHOUT binding any socket — protocol B
//   has no wire protocol (the no-bind rule: a protocol-B fixture
//   binds nothing). `ready()` is the load-bearing call: it drives the
//   full avvio boot (plugin loading, route compilation, handler
//   finalization) that every co-contender pays before its marker
//   (camel Main.run, rust ctx.start().await). Registration alone
//   would skip exactly the framework tax this cell measures.
// - Then the same route semantics as every contender: the one-shot
//   route ("timer fires once, immediately") reduced to a single
//   `BENCH_ROUTE_READY` line on stdout, exactly once, then exit 0.
// - No env contract: this scenario reads no BENCH_* variables —
//   timing/RSS are captured externally; the marker timing IS the
//   output.

import Fastify from "fastify";

const app = Fastify();

// Registered before the boot so route compilation lands inside
// ready(), like a real application. It is never served: nothing is
// bound and this scenario has no request phase.
app.all("/bench", async () => "BENCH_ROUTE_READY");

// Full avvio boot — plugin loading, route compilation, handler
// finalization — without binding any socket.
await app.ready();

console.log("BENCH_ROUTE_READY");
