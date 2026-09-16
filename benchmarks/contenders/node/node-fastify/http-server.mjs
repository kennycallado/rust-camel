// T3 http-server fixture — node-fastify contender (bench-node task 1.3).
// Same protocol-A contract as ../node-native/http-server.mjs (see its header
// for the full extraction), with Fastify in front of the route:
// - Bind 0.0.0.0:<port> from BENCH_HTTP_URL (host part ignored — all
//   fixtures in this scenario bind all interfaces).
// - Any method on the bench path -> 200 body `pong` (Fastify renders a
//   returned string as text/plain; charset=utf-8).
// - `BENCH_ROUTE_READY <unix_ms>` once, from the listen callback.
// - Minimal-bare per e_opus ruling D1 (2026-09-16; bd rc-h42s6): the
//   fixture emits NO per-request stdout lines (no `received` line, no
//   `id=<n>` counter) — the smoke's id check is WARN-only
//   observability (e_opus ruling D2).
// - BENCH_LATENCY_FILE only honored by creating the empty file (no
//   server-side latency record exists in protocol A).
//
// D5 ruling outcome (e_opus, 2026-09-16; bd rc-h42s6): the catch-all
// content-type parser below is RETAINED. The bench clients (loadgen,
// smoke's raw nc POST) send bodies with NO Content-Type header, and
// Fastify v5 answers 415 to a body with no/unknown Content-Type — so
// removing the parser (drain-neither) is IMPOSSIBLE without breaking
// the fixture against those clients. Consequence: the node family
// does NOT enter m3/m4 this era; the residual confound is DECLARED
// for m2 protocol-A latency. The route ignores the body anyway.

import Fastify from "fastify";
import fs from "node:fs";

const url = new URL(process.env.BENCH_HTTP_URL ?? "http://0.0.0.0:8080/bench");
const port = Number(url.port) || 80;
const benchPath = url.pathname;

if (process.env.BENCH_LATENCY_FILE) {
  fs.writeFileSync(process.env.BENCH_LATENCY_FILE, "");
}

const app = Fastify();

// Fastify v5 answers 415 to a body with no/unknown Content-Type; the
// bench clients (loadgen, smoke nc) send none. RETAINED per e_opus
// ruling D5 (2026-09-16) — see the header comment for the full
// decision record.
app.addContentTypeParser("*", { parseAs: "string" }, (_req, body, done) =>
  done(null, body),
);

app.all(benchPath, async () => "pong");

await app.listen({ port, host: "0.0.0.0" });
console.log(`BENCH_ROUTE_READY ${Date.now()}`);
