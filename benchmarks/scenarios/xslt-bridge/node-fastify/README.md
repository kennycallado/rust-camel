T4a xslt-bridge fixture for the `node-fastify` contender (bench-node
task 3.2): the same protocol-B contract as `../node-native/` (see its
README for the full extraction — env contract
`BENCH_PAYLOAD`/`BENCH_STYLESHEET`/`BENCH_LATENCY_FILE`, in-process
**saxon-js 2.7.0** transform exempt from the xml-bridge seam,
`timer:bench?period=10&repeatCount=10000` per-tick shape, startup
self-test = xslt3 SEF compile + first transform + output digest,
abort-before-marker on a transform failure) with the **Fastify
5.12.1** application (all three dependencies pinned exactly in
`package.json`; `package-lock.json` is committed — run
`npm ci --omit=dev` before the first run, that is the harness build
step) booted in front.

Engine auditability: `saxon-js` is Saxon-JS 2.7.0 (XSLT 3.0, same
vendor as the JVM's Saxon-HE 12.5 but a DIFFERENT engine — Saxon-JS ≠
Saxon-HE, own JS serializer) paired with `xslt3` 2.7.0 for the
startup SEF compile; the JVM counterparts run Saxon-HE 12.5
in-process (the quarkus cell is won't-measure for T4a — Xalan cannot
compile this stylesheet in native mode). Transform-output parity is
documented in `../node-native/README.md`: cross-runtime byte-parity
is not asserted; the self-test digest
`BENCH_XSLT_SELFTEST_SHA256=<hex>` pins THIS fixture's output against
its committed smoke evidence (`../smoke/node-fastify.log`).

The module import, `fastify()` construction, route registration, and
`await app.ready()` (route.mjs:64 — the full avvio boot, the same
framework tax every co-contender pays before its marker: rust
`ctx.start().await`, Camel `Main.run`) run WITHOUT binding any
socket — this scenario has no wire protocol (no-bind rule for
protocol B), and the registered route (route.mjs:60) is never served.
After the boot and self-test, the fixture arms the timer route: per
tick transform + `BENCH_LATENCY` record + `BENCH_XSLT_TICK` log,
marker `BENCH_ROUTE_READY <unix_ms>` exactly once (route.mjs:139 —
boot strictly before the marker), then idles until killed.
Latency-file default is the harness protocol-B probe path
`/tmp/v3-protocol-b-xslt-bridge_node-fastify.log`; asset defaults
anchor via `import.meta.url` because the harness gives node cells no
per-cell env and no cd.

Observed behavior (task 3.2 evidence, dev host, node v22.23.2):
canonical payload → single
`BENCH_XSLT_SELFTEST_SHA256=17713b3d54921b7d3c1420252685e94eca4689781258268e6c948ae5ae6742d9`
(identical to node-native — same engine, same serializer), one marker
after ready(), then `BENCH_XSLT_TICK id=1..` and `BENCH_LATENCY`
records at the same ~1-2ms/tick as node-native (the transform
dominates; the booted-but-idle Fastify app adds no per-tick cost).
Committed smoke evidence: `../smoke/node-fastify.log`.

Run standalone: `npm ci --omit=dev` then
`BENCH_PAYLOAD=../shared/bench-payload.xml BENCH_STYLESHEET=../shared/identity-transform.xsl BENCH_LATENCY_FILE=/tmp/t4a-node-fastify.log node route.mjs`.
