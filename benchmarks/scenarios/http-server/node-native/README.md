T3 http-server fixture for the `node-native` contender: a
zero-dependency Node (`node:http`, ESM, no `package.json` — the
committed script IS the artifact) server implementing the scenario's
protocol-A contract. It binds `0.0.0.0:8080` (port + path taken from
`BENCH_HTTP_URL`, host part ignored — every fixture in this scenario
binds all interfaces), answers any method on `/bench` with `200` +
`pong` (`text/plain; charset=utf-8`), prints `BENCH_ROUTE_READY
<unix_ms>` once from the listen callback, and logs
`BENCH_HTTP_REQUEST received` / `BENCH_HTTP_REQUEST id=<n>` per
request. There is no server-side latency record: protocol A measures
client-side via bench-loadgen; when `BENCH_LATENCY_FILE` is set the
fixture only creates the empty file (no protocol-A line format
exists). This empty-file creation is an http-server-only convention:
the file is touch-created so operators see the probe path, while the
tick scenarios (xsd/xslt) write real records and the startup/t2/eip
cells ignore the env, matching their JVM/rust peers.
Run standalone: `BENCH_HTTP_URL=http://127.0.0.1:8080/bench node
route.mjs`, then `POST /bench` with any body.
