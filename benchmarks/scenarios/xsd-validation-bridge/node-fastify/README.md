T4b xsd-validation-bridge fixture for the `node-fastify` contender
(bench-node task 3.1): the same protocol-B contract as
`../node-native/` (see its README for the full extraction — env
contract `BENCH_PAYLOAD`/`BENCH_SCHEMA`/`BENCH_LATENCY_FILE`,
in-process **xmllint-wasm 5.3.0** validation exempt from the
xml-bridge seam, `timer:bench?period=10&repeatCount=10000` per-tick
shape, startup self-test = wasm init slot, abort-before-marker on an
invalid payload) with the **Fastify 5.12.1** application (both
dependencies pinned exactly in `package.json`; `package-lock.json`
is committed — run `npm ci --omit=dev` before the first run, that is
the harness build step) booted in front.

Engine auditability: `xmllint-wasm` is libxml2 compiled to
WebAssembly (chosen for buildability — no node-gyp — with wasm
overhead as a documented caveat); the JVM counterparts run **Xerces-J
2.12.2** in-process (spec §4.8 pin). Wasm init placement: the
one-time module fetch + compile + first schema parse sits in the
startup self-test BEFORE the marker — the node counterpart of the
JVM's once-per-process Xerces schema compile at route start; the
residual per-call worker spin-up is this engine's honest
per-validation cost (see ../node-native/README.md for the measured
numbers and the caveat).

The module import, `fastify()` construction, route registration, and
`await app.ready()` (route.mjs:50 — the full avvio boot, the same
framework tax every co-contender pays before its marker: rust
`ctx.start().await`, Camel `Main.run`) run WITHOUT binding any
socket — this scenario has no wire protocol (no-bind rule for
protocol B), and the registered route is never served. After the
boot and self-test, the fixture arms the timer route: per tick
validate + `BENCH_LATENCY` record + `BENCH_XSD_TICK` log, marker
`BENCH_ROUTE_READY <unix_ms>` exactly once (route.mjs:120 — boot
strictly before the marker), then idles until killed. Latency-file
default is the harness protocol-B probe path
`/tmp/v3-protocol-b-xsd-validation-bridge_node-fastify.log`; asset
defaults anchor via `import.meta.url` because the harness gives node
cells no per-cell env and no cd.

Observed behavior (task 3.1 evidence, dev host, node v22.23.2):
canonical payload → single marker
`BENCH_ROUTE_READY 1788167406761` after ready(), then `BENCH_XSD_TICK
id=1..` and `BENCH_LATENCY` records at the same ~42ms/tick as
node-native (the validation dominates; the booted-but-idle Fastify
app adds no per-tick cost); required-`<meta>`-removed temp payload →
exit 1 before any marker with the libxml2 validity error. Committed
smoke evidence: `smoke/node-fastify.log` (marker + ≥10 latency
records + ≥10 BENCH_XSD_TICK).

Run standalone: `npm ci --omit=dev` then
`BENCH_PAYLOAD=../shared/bench-payload.xml BENCH_SCHEMA=../shared/schema.xsd
BENCH_LATENCY_FILE=/tmp/t4b-node-fastify.log node route.mjs`.
