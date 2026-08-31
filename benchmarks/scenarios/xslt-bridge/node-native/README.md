T4a xslt-bridge fixture for the `node-native` contender (bench-node
task 3.2) — Node stdlib has no XSLT engine, so the transform lands on
**saxon-js 2.7.0** (pinned exactly in `package.json`;
`package-lock.json` is committed — run `npm ci --omit=dev` before the
first run, that is the harness build step).

Engine auditability: `saxon-js` is **Saxon-JS 2.7.0**, an XSLT 3.0
engine from the same vendor (Saxonica) as the JVM side's **Saxon-HE
12.5** (spec §4.8 pin) but a DIFFERENT engine — Saxon-JS ≠ Saxon-HE:
it is the JavaScript/WASM re-implementation, not the JVM library, and
serializes through its own serializer. The rust fixtures do not run
Saxon in-process at all: they delegate the transform to the compiled
`xml-bridge` subprocess over gRPC mTLS (the bridge tax this scenario
measures; the bridge internally runs Saxon-HE 12.5). This fixture is
EXEMPT from that seam — it transforms IN-PROCESS against the SAME
byte-pinned shared assets (`../shared/bench-payload.xml`,
`../shared/identity-transform.xsl`) the JVM/rust contenders read.

Compiler pairing: the `saxon-js` runtime package executes only
COMPILED stylesheets (SEF — stylesheet export file); it has no XSLT
compiler. The sibling `xslt3` 2.7.0 package (Saxonica's CLI tool,
built on the same saxon-js engine) compiles the shared stylesheet at
fixture startup — this is the engine init slot, the counterpart of
the JVM's once-per-process `Templates` compile at route start (Camel
XsltEndpoint). The SEF is parsed into memory once and reused per tick
(`stylesheetInternal`), so measured ticks pay only the per-call
transform cost — no per-tick recompile. The SEF temp file is deleted
after startup; the stylesheet's source of truth stays under `shared/`
(no fixture-local copy).

Contract (extracted from the existing contenders — rust-camel-lib
main.rs, camel-standalone App.java): env contract `BENCH_PAYLOAD` /
`BENCH_STYLESHEET` / `BENCH_LATENCY_FILE` (the same names the rust
fixture reads). Defaults anchor to the fixture's own location via
`import.meta.url` because the harness launches node cells with no
per-cell env and no cd; the latency-file default is the harness
protocol-B probe path
`/tmp/v3-protocol-b-xslt-bridge_node-native.log` (the harness also
injects `BENCH_LATENCY_FILE` explicitly into the cell argv since the
task 3.1 review — the default only covers standalone runs).

Route shape: `timer:bench?period=10&repeatCount=10000` — per tick:
set_body(shared payload) → XSLT identity transform in-process (timed
span brackets ONLY the transform, like the JVM BenchStart property)
→ append `BENCH_LATENCY <id> <duration_ns>` → log
`BENCH_XSLT_TICK id=<n>`. Ticks are sequential (setTimeout chain),
like a Camel timer on its single consumer thread. Marker
`BENCH_ROUTE_READY <unix_ms>` exactly once, after the self-test.

Transform-output parity: byte-parity across runtimes is NOT asserted
— Saxon-JS and Saxon-HE are different engines with different
serializers, so cross-runtime byte identity may be impossible. The
digest comparison uses the fixture's own stable output vs its smoke
evidence (stability, not cross-runtime identity): the startup
self-test logs `BENCH_XSLT_SELFTEST_SHA256=<hex>` (pre-marker,
startup-only) — the sha256 of the transform output for the canonical
payload — and the committed smoke evidence
(`../smoke/node-native.log`) must carry the same digest. The output
itself is the payload doc copied verbatim minus the XML declaration
(`omit-xml-declaration="yes"` in the stylesheet; the leading comment
and all whitespace text nodes are copied by the identity template).

Observed behavior (task 3.2 evidence, dev host, node v22.23.2):
startup ~2s (node boot + xslt3 SEF compile ~0.7s + first transform),
then one `BENCH_XSLT_SELFTEST_SHA256=17713b3d54921b7d3c1420252685e94eca4689781258268e6c948ae5ae6742d9`,
single marker, and `BENCH_XSLT_TICK`/`BENCH_LATENCY` pairs at ~1-2ms
transform time per 10ms tick. Committed smoke evidence:
`../smoke/node-native.log`.

Run standalone: `npm ci --omit=dev` then
`BENCH_PAYLOAD=../shared/bench-payload.xml BENCH_STYLESHEET=../shared/identity-transform.xsl BENCH_LATENCY_FILE=/tmp/t4a-node-native.log node route.mjs`.
