// T3 http-server fixture — node-fastify contender (bench-node task 1.3).
// Same protocol-A contract as ../node-native/route.mjs (see its header
// for the full extraction), with Fastify in front of the route:
// - Bind 0.0.0.0:<port> from BENCH_HTTP_URL (host part ignored — all
//   fixtures in this scenario bind all interfaces).
// - Any method on the bench path -> 200 body `pong` (Fastify renders a
//   returned string as text/plain; charset=utf-8).
// - `BENCH_ROUTE_READY <unix_ms>` once, from the listen callback.
// - `BENCH_HTTP_REQUEST received` + `BENCH_HTTP_REQUEST id=<n>` per
//   bench-path request, logged from the route handler (counter starts
//   at 1; non-bench paths emit nothing and consume no id).
// - BENCH_LATENCY_FILE only honored by creating the empty file (no
//   server-side latency record exists in protocol A).

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
// JVM + rust fixtures accept any body (the smoke's raw nc request
// carries none). Catch-all parser restores identical semantics — the
// route ignores the body anyway.
app.addContentTypeParser("*", { parseAs: "string" }, (_req, body, done) =>
  done(null, body),
);

let requestId = 0;

// Logged inside the route handler only, like the JVM contenders whose
// per-request lines live inside the route processor: non-bench paths
// (fastify default 404) must not emit lines or consume ids.
app.all(benchPath, async () => {
  requestId += 1;
  console.log("BENCH_HTTP_REQUEST received");
  console.log(`BENCH_HTTP_REQUEST id=${requestId}`);
  return "pong";
});

await app.listen({ port, host: "0.0.0.0" });
console.log(`BENCH_ROUTE_READY ${Date.now()}`);
