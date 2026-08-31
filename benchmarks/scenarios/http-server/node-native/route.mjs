// T3 http-server fixture — node-native contender (bench-node task 1.3).
// Zero-dependency `node:http` server, protocol A (the only
// listener-binding scenario in the suite).
//
// Contract (extracted from the existing contenders — camel-quarkus
// jetty route, camel-standalone App.java, rust-camel-lib main.rs):
// - Bind 0.0.0.0:<port> (all interfaces, ADR-0061). The host part of
//   BENCH_HTTP_URL is the client-side address (127.0.0.1) and is
//   deliberately NOT used for binding — every fixture in this scenario
//   binds all interfaces; only port + path are taken from the URL.
// - Any method on the bench path answers `200` + body `pong`
//   (text/plain; charset=utf-8), matching spec §4.10
//   respond(200, body=pong). Unknown paths -> 404, like the
//   path-registered consumers of the other fixtures.
// - `BENCH_ROUTE_READY <unix_ms>` on stdout ONCE, from the listen
//   callback — listener-bound, not first-request (spec §4.10).
// - Per request: `BENCH_HTTP_REQUEST received` then
//   `BENCH_HTTP_REQUEST id=<n>` on stdout (smoke asserts id=1; the
//   counter starts at 1 like every other contender).
// - No server-side latency record: protocol A measures client-side
//   via bench-loadgen; no existing T3 fixture writes latency lines.
//   BENCH_LATENCY_FILE is only honored by creating the (empty) file
//   when set, mirroring the protocol-B rust fixtures' startup
//   File::create, so the env contract stays uniform across scenarios.

import { createServer } from "node:http";
import fs from "node:fs";

const url = new URL(process.env.BENCH_HTTP_URL ?? "http://0.0.0.0:8080/bench");
const port = Number(url.port) || 80;
const benchPath = url.pathname;

if (process.env.BENCH_LATENCY_FILE) {
  fs.writeFileSync(process.env.BENCH_LATENCY_FILE, "");
}

let requestId = 0;

const server = createServer((req, res) => {
  if (req.url !== benchPath) {
    res.statusCode = 404;
    res.end();
    return;
  }
  // Logged only for bench-path requests, like the JVM contenders whose
  // per-request lines live inside the route processor: 404s must not
  // emit lines or consume ids.
  requestId += 1;
  console.log("BENCH_HTTP_REQUEST received");
  console.log(`BENCH_HTTP_REQUEST id=${requestId}`);
  // Explicit Content-Length: without it node sends the body chunked,
  // which diverges from every other contender (single framing) and
  // breaks raw-client parsing that reads the last response line.
  res.writeHead(200, {
    "Content-Type": "text/plain; charset=utf-8",
    "Content-Length": Buffer.byteLength("pong"),
  });
  res.end("pong");
});

server.listen(port, "0.0.0.0", () => {
  console.log(`BENCH_ROUTE_READY ${Date.now()}`);
});
