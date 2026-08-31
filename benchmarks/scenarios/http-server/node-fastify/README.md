T3 http-server fixture for the `node-fastify` contender: the same
protocol-A contract as `../node-native/` served through
**Fastify 5.12.1** (pinned exactly in `package.json`; `package-lock.json`
is committed — run `npm ci --omit=dev` before the first run, that is
the harness build step). It binds `0.0.0.0:8080` (port + path from
`BENCH_HTTP_URL`, host part ignored), answers any method on `/bench`
with `200` + `pong`, prints `BENCH_ROUTE_READY <unix_ms>` once from the
listen callback, and logs `BENCH_HTTP_REQUEST received` /
`BENCH_HTTP_REQUEST id=<n>` per request. A catch-all content-type
parser is registered because Fastify v5 otherwise answers `415` to a
body without a Content-Type header (the smoke's raw `nc` request has
none; the JVM/rust fixtures accept any body). No server-side latency
record: protocol A measures client-side; `BENCH_LATENCY_FILE` only
creates the empty file — an http-server-only convention
(touch-created so operators see the probe path; tick scenarios
xsd/xslt write real records; startup/t2/eip cells ignore the env,
matching their JVM/rust peers). Run standalone: `npm ci` then
`BENCH_HTTP_URL=http://127.0.0.1:8080/bench node route.mjs`.
