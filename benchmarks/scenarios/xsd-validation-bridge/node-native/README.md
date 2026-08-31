T4b xsd-validation-bridge fixture for the `node-native` contender
(bench-node task 3.1) — the first node-native fixture with a
dependency: Node stdlib has no XML, so XSD validation lands on
**xmllint-wasm 5.3.0** (pinned exactly in `package.json`;
`package-lock.json` is committed — run `npm ci --omit=dev` before the
first run, that is the harness build step). Engine auditability:
`xmllint-wasm` is libxml2 (the xmllint C library) compiled to
WebAssembly; the JVM counterparts on this scenario's other fixtures
run **Xerces-J 2.12.2** in-process (spec §4.8 pin). libxml2/wasm was
chosen for buildability — pure wasm, no node-gyp native compile —
with the wasm overhead as a documented caveat (below).

Contract (extracted from the existing contenders — rust-camel-lib
main.rs, camel-standalone App.java): env contract `BENCH_PAYLOAD` /
`BENCH_SCHEMA` / `BENCH_LATENCY_FILE` (the same names the rust
fixture reads). Defaults anchor to the fixture's own location via
`import.meta.url` (`../shared/bench-payload.xml`,
`../shared/schema.xsd`) because the harness launches node cells with
no per-cell env and no cd, so the rust fixture's CWD-relative
defaults would not resolve; the latency-file default is the harness
protocol-B probe path
`/tmp/v3-protocol-b-xsd-validation-bridge_node-native.log`. This
fixture is EXEMPT from the compiled `xml-bridge` subprocess seam the
rust fixtures pay: it validates IN-PROCESS against the SAME
byte-pinned shared assets the JVM/rust contenders read (digest
parity by construction — no fixture-local XML copies).

Route shape: `timer:bench?period=10&repeatCount=10000` — per tick:
validate the payload in-process (timed span brackets ONLY the
validation, like the JVM BenchStart property), append
`BENCH_LATENCY <id> <duration_ns>` to the latency file, log
`BENCH_XSD_TICK id=<n>`; marker `BENCH_ROUTE_READY <unix_ms>` exactly
once after route start; then idle until killed. Ticks are sequential
(setTimeout chain) like a Camel timer that never overlaps route
executions on its single consumer thread.

Wasm init placement: the JVM compiles the Xerces schema once per
process at route start; the node counterpart forces the wasm module
fetch + compile + first schema parse at a STARTUP SELF-TEST — one
full validation BEFORE the marker. An invalid payload exits non-zero
with `error: xsd validation failed: <xmllint validity error>` BEFORE
the marker (the abort-before-marker convention of the t2-json node
fixture's output assert), so the self-test doubles as the
invalid-payload guard. Residual caveat: xmllint-wasm's API spawns a
fresh worker thread per `validateXML` call (engine design, v5.3.0),
so per-tick validation pays worker spin-up + wasm instantiation +
schema parse every tick — measured ~42ms/tick on the dev host (first
call ~47ms; the startup self-test absorbs the one-time module
compile). That residual is part of this contender's honest
per-validation cost and is NOT amortized the way Xerces' compiled
`Schema` object is.

Observed behavior (task 3.1 evidence, dev host, node v22.23.2):
canonical payload → single marker
`BENCH_ROUTE_READY 1788167404754`, then `BENCH_XSD_TICK id=1..`,
`BENCH_LATENCY 1 42734026` (ns) records at ~42ms/tick; payload with
the required `<meta>` element removed (temp copy) → exit 1 before
any marker: `error: xsd validation failed: bench-payload.xml:16:
Schemas validity error : Element '{...t4b}section': This element is
not expected. Expected is ( {...t4b}meta ).` Committed smoke
evidence: `smoke/node-native.log` (smoke cell: marker + ≥10 latency
records + ≥10 BENCH_XSD_TICK). The same smoke run executed the live
rust-camel-cli cell against the same shared payload + schema
(marker + 60 ticks) — same validation outcome across runtimes.

Run standalone: `npm ci --omit=dev` then
`BENCH_PAYLOAD=../shared/bench-payload.xml BENCH_SCHEMA=../shared/schema.xsd
BENCH_LATENCY_FILE=/tmp/t4b-node-native.log node route.mjs`.
