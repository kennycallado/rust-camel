# Node contenders — shared runtime dir

Both node contenders (`node-native`, `node-fastify`) share ONE runtime
dir (change `bench-consol-tick` task 1.3): one `package.json` (union of
`fastify` 5.12.1, `saxon-js` 2.7.0, `xslt3` 2.7.0, `xmllint-wasm`
5.3.0), one committed `package-lock.json`, and one `node_modules`
(~21 MB, installed once with `npm ci --omit=dev`) — instead of seven
per-scenario copies.

## Layout

- `node-native/<scenario>.mjs` — bare-runtime entry scripts (Pair A)
- `node-fastify/<scenario>.mjs` — Fastify-fronted entry scripts (Pair B)
- `package.json` / `package-lock.json` — the shared dependency set

The harness (`benchmarks/harness/run.sh`) wires each node cell as
`env BENCH_LATENCY_FILE=<probe path> node <member>/<scenario>.mjs`.
Asset defaults (payload, schema, stylesheet) resolve inside the scripts
relative to their own location into the owning scenario's
`benchmarks/scenarios/<scenario>/shared/` — scenario DATA never left
`benchmarks/scenarios/`. The harness injects the latency file
explicitly; the in-script defaults are the standalone-run fallback.

`npm ci --omit=dev` for any standalone run happens HERE (the runtime
dir), not per scenario.

## Per-fixture contracts

Merged from the original per-scenario fixture READMEs (paths adapted
to the consolidated layout; history references to `bench-node` tasks
kept verbatim).

---

## `node-native/startup-minimal.mjs`

T1 startup-minimal fixture for the `node-native` contender: a
zero-dependency Node script (ESM, no `package.json` — the committed
script IS the artifact) implementing the scenario's protocol-B
process-spawn contract: process start → route execution → marker,
with timing/RSS captured by the harness from OUTSIDE the process
(single clock, GNU time -v) — no self-instrumentation. The scenario's
route is `timer:bench?repeatCount=1&delay=0` → log
`BENCH_ROUTE_READY` (`delay=0` skips Camel's default 1000ms initial
delay; `repeatCount=1` fires exactly once), so the script equivalent
is a single unconditional marker line at startup. It prints
`BENCH_ROUTE_READY` exactly once (the harness validates the exact
marker count) and exits 0. There is no env contract in this scenario:
no BENCH_* variable is read, there is no latency file, and there is
no canonical payload — the marker timing IS the output. Unlike the
JVM/rust fixtures, which idle after the marker until the harness
kills them externally, a plain script has no runtime to keep alive
and exits; the post-marker kill is a no-op either way and
time-to-marker is unaffected. Run standalone: `node startup-minimal.mjs`.

---

## `node-fastify/startup-minimal.mjs`

T1 startup-minimal fixture for the `node-fastify` contender: the same
protocol-B marker contract as `node-native/` (see its README) with
the **Fastify 5.12.1** application (pinned exactly in `package.json`;
`package-lock.json` is committed — run `npm ci --omit=dev` before the
first run, that is the harness build step) booted in front. The
module import, `fastify()` construction, route registration, and
`await app.ready()` (the full avvio boot — plugin loading, route
compilation, handler finalization, the same framework tax every
co-contender pays before its marker) run WITHOUT binding any socket —
this scenario has no wire protocol (no-bind rule for protocol B).
After the boot, the fixture performs the same route
semantics as every contender — the one-shot timer route reduced to a
single `BENCH_ROUTE_READY` line, printed exactly once — and exits 0.
No BENCH_* env variable is read: timing/RSS are captured externally
and the marker timing IS the output. Run standalone: `npm ci` then
`node startup-minimal.mjs`.

---

## `node-native/t2-json.mjs`

T2 t2-json fixture for the `node-native` contender: a zero-dependency
Node script (ESM, no `package.json` — the committed script IS the
artifact) implementing the scenario's protocol-B contract. It reads
`BENCH_PAYLOAD_BYTES` (default 32768, validated against the payload
axis 1024|32768|262144|1048576; invalid values abort before any
marker), builds the canonical JSON document of exactly SIZE bytes
(`{"id":"bench","seq":0,"fill":"<K×'b'>"}` — the JS port of
bench-loadgen's `payload::canonical_json_body`, tick = 0), logs
`BENCH_INPUT_SHA256=<digest>` (must equal the scenario's committed
golden — INPUT parity across every contender), then performs the same
route as every fixture: unmarshal (`JSON.parse`) → filter
(`id == "bench"`) → transform (insert `"bench": true` into the parsed
map) → marshal (the single serialization, `JSON.stringify`) → output
assert (exact SIZE+13 length AND parsed semantic equality) → marker
`BENCH_ROUTE_READY bytes=<len>` exactly once. The +13 delta is exactly
the inserted `,"bench":true` member; an assert failure exits non-zero
BEFORE the marker, so a missing or wrong marker means the cell failed.
INPUT parity is what the digest proves — cross-runtime OUTPUT
byte-parity is NOT claimed (serializers differ in member order; see
the scenario README caveat). The route is the warm-tick timer form
`timer:bench?period=10&repeatCount=10000&delay=0` — per tick: the
same pipeline (set_body → unmarshal → filter → transform → marshal →
output assert) fires IMMEDIATELY at t0, then every 10 ms (10000
exchanges total), then the script idles like the rust fixture until
the smoke/harness kills it. Every tick brackets the WHOLE per-tick
body (t0 before set_body, record after the assert) and appends
`BENCH_LATENCY <tick> <duration_ns>` to `BENCH_LATENCY_FILE` (default
`/tmp/v3-protocol-b-t2-json_node-native.log`, truncated at startup) —
one record per tick = one full pipeline. The marker
`BENCH_ROUTE_READY bytes=<len>` is latched to the FIRST completed
exchange, at its original code-path position (after the output
assert) and BEFORE that tick's latency record. The canonical body is
built ONCE at startup with `seq` frozen at `CANONICAL_SELFTEST_TICK`
and reused verbatim every tick, so the measured window contains only
exchange processing. Run standalone:
`BENCH_PAYLOAD_BYTES=32768 node t2-json.mjs`.

---

## `node-fastify/t2-json.mjs`

T2 t2-json fixture for the `node-fastify` contender: the same
protocol-B contract as `node-native/` (see its README for the full
extraction — env contract, canonical body builder, transform chain,
SIZE+13 output assert, golden `BENCH_INPUT_SHA256` INPUT parity) with
the **Fastify 5.12.1** application (pinned exactly in `package.json`;
`package-lock.json` is committed — run `npm ci --omit=dev` before the
first run, that is the harness build step) booted in front. The module
import, `fastify()` construction, route registration, and
`await app.ready()` (the full avvio boot — plugin loading, route
compilation, handler finalization, the same framework tax every
co-contender pays before its marker) run WITHOUT binding any socket —
this scenario has no wire protocol (no-bind rule for protocol B), and
the registered route is never served. After the boot, the fixture
arms the same warm-tick timer route
`timer:bench?period=10&repeatCount=10000&delay=0` — per tick: SHA log
→ unmarshal → filter → transform → marshal → output assert, firing
IMMEDIATELY at t0 then every 10 ms (10000 exchanges total), then
idles until the smoke/harness kills it. Every tick brackets the WHOLE
per-tick body and appends `BENCH_LATENCY <tick> <duration_ns>` to
`BENCH_LATENCY_FILE` (default
`/tmp/v3-protocol-b-t2-json_node-fastify.log`, truncated at startup);
the marker `BENCH_ROUTE_READY bytes=<len>` is latched to the FIRST
completed exchange, before that tick's latency record. The canonical
body is built ONCE at startup with `seq` frozen at
`CANONICAL_SELFTEST_TICK` and reused verbatim every tick. OUTPUT
byte-parity across runtimes is not claimed (serializer member-order
caveat in the scenario README). Run standalone: `npm ci` then
`BENCH_PAYLOAD_BYTES=32768 node t2-json.mjs`.

---

## `node-native/t2-realistic-eip.mjs`

T2 t2-realistic-eip fixture for the `node-native` contender: a
zero-dependency Node script (ESM, no `package.json` — the committed
script IS the artifact) implementing the scenario's protocol-B
contract. The scenario has no payload axis and no env contract: the
route sets its OWN body, so — unlike t2-json / split-aggregate —
there is no `BENCH_INPUT_SHA256` line, and the harness-exported
`BENCH_PAYLOAD_BYTES` is ignored exactly as the rust fixture ignores
it. The chain (extracted identically from all five existing
contenders — rust-camel-lib `src/main.rs`, rust-camel-cli
`routes/t2-realistic-eip.yaml`, camel-standalone `App.java` +
`routes.yaml`, camel-quarkus `BenchRoute.java`) is:
`timer:bench?period=10&repeatCount=10000&delay=0` fires IMMEDIATELY
at t0 with an empty exchange body, then every 10 ms (10000 exchanges
total); per exchange: `set_body("ping")` → `set_header(source =
"bench")` → `filter("${body} == 'ping'")` (always true under this
route — the body was set to the literal one step earlier — but
evaluated, not assumed; a false filter skips the choice block, and
the log after it stays unconditional in every fixture) →
`choice { when("${header.source} == 'bench'") → set_body
("pong-bench"); otherwise → set_body("pong-other") }` → `log
("BENCH_ROUTE_READY body=${body}")`. The when branch is the taken
one, so the final body is `pong-bench` and the marker reads
`BENCH_ROUTE_READY body=pong-bench` — latched to the FIRST completed
exchange (the harness greps -F the full string and validates the
count); a wrong-branch run would read `body=pong-other` and fail the
grep, making the branch decision observable, not silent. Every tick
brackets the WHOLE per-tick body and appends `BENCH_LATENCY <tick>
<duration_ns>` to `BENCH_LATENCY_FILE` (default
`/tmp/v3-protocol-b-t2-realistic-eip_node-native.log`, truncated at
startup) — one record per tick = one full EIP pipeline. After
repeatCount the script idles like the rust fixture (`ctrl_c().await`)
until the smoke/harness kills it. Run standalone:
`node t2-realistic-eip.mjs`.

Semantic parity evidence (task 2.4, no committed golden — parity is
asserted against the LIVE rust reference, not a digest):
`cargo build -p t2-realistic-eip-rust-camel-lib` then
`timeout 8 ./target/debug/t2-realistic-eip` emits
`INFO BENCH_ROUTE_READY exchange_id=<uuid>` (static line) and
`INFO BENCH_ROUTE_READY body=pong-bench` (the marker, exactly once),
no `pong-other`, then idles — filter passed, when branch taken, final
body `pong-bench`. `node t2-realistic-eip.mjs` emits the identical marker line
`BENCH_ROUTE_READY body=pong-bench` exactly once, no `pong-other`,
then idles: same choice/when outcomes, same final body. Two
documented divergences from the rust reference: (1) it emits an extra
STATIC `BENCH_ROUTE_READY exchange_id=…` line before the marker — a
rust-camel workaround (its `log` step is static-only; the fixture
pairs a v1-baseline static line with a dynamic `process` step), not
chain semantics; the four Simple/YAML/Java fixtures (every one
except rust-camel-lib) emit only the dynamic log line, which is the shape mirrored here. (2) rust-camel's
builder closes an empty `.end_filter()` before `.choice()` (a
type-system artifact — `choice` lives on RouteBuilder), while this
port nests the choice inside the filter scope like the YAML and Java
fixtures; under an always-true filter the two shapes are
observationally identical.

---

## `node-fastify/t2-realistic-eip.mjs`

T2 t2-realistic-eip fixture for the `node-fastify` contender: the
same protocol-B contract as `node-native/` (see its README for the
full chain extraction and parity evidence — no payload axis, no env
contract, no `BENCH_INPUT_SHA256`; `set_body("ping")` → `set_header
(source="bench")` → `filter("${body} == 'ping'")` → `choice { when
("${header.source} == 'bench'") → "pong-bench"; otherwise →
"pong-other" }` → `log("BENCH_ROUTE_READY body=${body}")`, fired per
tick by `timer:bench?period=10&repeatCount=10000&delay=0`) with the
**Fastify 5.12.1**
application (pinned exactly in `package.json`; `package-lock.json`
is committed — run `npm ci --omit=dev` before the first run, that is
the harness build step) booted in front. The module import,
`fastify()` construction, route registration, and `await
app.ready()` (the full avvio boot — plugin loading, route
compilation, handler finalization, the same framework tax every
co-contender pays before its marker: rust `ctx.start().await`,
Camel `Main.run`) run WITHOUT binding any socket — this scenario has
no wire protocol (no-bind rule for protocol B), and the registered
route is never served. After the boot, the fixture arms the same
warm-tick timer route `timer:bench?period=10&repeatCount=10000&delay=0`
— per tick: chain → marker `BENCH_ROUTE_READY body=pong-bench`
latched to the FIRST completed exchange (when branch taken, final
body `pong-bench`; a wrong branch would read `body=pong-other` and
miss the harness grep), then idles until the smoke/harness kills it.
Every tick brackets the WHOLE per-tick body and appends
`BENCH_LATENCY <tick> <duration_ns>` to `BENCH_LATENCY_FILE` (default
`/tmp/v3-protocol-b-t2-realistic-eip_node-fastify.log`, truncated at
startup). Run standalone: `npm ci --omit=dev` then
`node t2-realistic-eip.mjs`.

Observed behavior (parity with the LIVE rust reference —
`./target/debug/t2-realistic-eip` after `cargo build -p
t2-realistic-eip-rust-camel-lib`): rust emits the marker
`BENCH_ROUTE_READY body=pong-bench` exactly once (preceded by its
static `BENCH_ROUTE_READY exchange_id=…` line — a rust-camel
static-log workaround documented in the node-native entry header
(`<scn>.mjs`), not mirrored here) and no `pong-other`, then idles; this fixture emits
the identical single marker line `BENCH_ROUTE_READY body=pong-bench`
exactly once, no `pong-other`, then idles — same choice/when
outcomes, same final body. `ready()` lands at `<scn>.mjs`, the
marker emission in the same file — boot strictly before the marker.

---

## `node-native/split-aggregate.mjs`

T2 split-aggregate fixture for the `node-native` contender: a
zero-dependency Node script (ESM, no `package.json` — the committed
script IS the artifact) implementing the scenario's protocol-B
contract. The input is FIXED: the canonical array body of exactly 591
bytes — a compact JSON array whose 100 items are the strings `b0`
through `b99` (item `i` is `"b" + i`), the JS port of bench-loadgen's
`payload::canonical_split_aggregate_array`. Unlike t2-json there is no
env contract: `BENCH_PAYLOAD_BYTES` is ignored here exactly as the rust
fixture ignores it (the array is size- and tick-independent). The
fixture logs `BENCH_INPUT_SHA256=<digest>` (must equal the scenario's
committed golden `123444b4…` — INPUT parity across every contender),
unmarshals (`JSON.parse`), then splits SEQUENTIALLY: one fragment after
another, each dispatched to `direct:agg-in` and its response fully
processed before the next dispatch (`parallel(false)` semantics — the
`await` in the split loop IS the split scope). The aggregation route is
hand-rolled in ~30 lines of plain JS — correlation buckets keyed by the
constant `bench.correlation` header, `CollectAll` list-append,
completion at exactly 100 fragments, and the pending sentinel
(`CamelAggregatorPending=true`, empty body) for fragments 1..99; this
hand-rolled coordination is the honest point of the contender, so no
orchestration library and zero dependencies. The completion assert
verifies the collection length (100) AND consistency with
`CamelAggregatedSize`, stamps `bench.aggregated.size = 100`, and the
marker `BENCH_ROUTE_READY items=100` fires ONLY from that path, exactly
once; sentinels flow through the same guard steps and never emit. An
assert failure exits non-zero BEFORE the marker, so a missing or wrong
marker means the cell failed. The route is the warm-tick timer form
`timer:bench?period=10&repeatCount=10000&delay=0`: the same
split+aggregate pipeline fires IMMEDIATELY at t0, then every 10 ms
(10000 exchanges total), then the script idles like the rust fixture
until the smoke/harness kills it. Every tick brackets the WHOLE
per-tick body (t0 before set_body, record after the completion
assert) and appends `BENCH_LATENCY <tick> <duration_ns>` to
`BENCH_LATENCY_FILE` (default
`/tmp/v3-protocol-b-split-aggregate_node-native.log`, truncated at
startup) — one record per tick = one full split+aggregate pipeline.
The marker `BENCH_ROUTE_READY items=100` is latched to the FIRST
completed exchange (fragment 100's completion inside tick 1), before
that tick's latency record. Run standalone: `node split-aggregate.mjs`.

---

## `node-fastify/split-aggregate.mjs`

T2 split-aggregate fixture for the `node-fastify` contender: the same
protocol-B contract as `node-native/` (see its README for the full
extraction — fixed 591-byte canonical array, sequential split,
hand-rolled correlation buckets, pending sentinel, completion assert,
golden `BENCH_INPUT_SHA256` INPUT parity) with the **Fastify 5.12.1**
application (pinned exactly in `package.json`; `package-lock.json` is
committed — run `npm ci --omit=dev` before the first run, that is the
harness build step) booted in front. The module import, `fastify()`
construction, route registration, and `await app.ready()` (the full
avvio boot — plugin loading, route compilation, handler finalization,
the same framework tax every co-contender pays before its marker) run
WITHOUT binding any socket — this scenario has no wire protocol
(no-bind rule for protocol B), and the registered route is never
served. After the boot, the fixture arms the same warm-tick timer
route `timer:bench?period=10&repeatCount=10000&delay=0` — per tick:
SHA log → unmarshal → sequential split → aggregate → completion
assert → marker `BENCH_ROUTE_READY items=100` latched to the FIRST
completed exchange, then idles until the smoke/harness kills it.
Every tick brackets the WHOLE per-tick body and appends
`BENCH_LATENCY <tick> <duration_ns>` to `BENCH_LATENCY_FILE` (default
`/tmp/v3-protocol-b-split-aggregate_node-fastify.log`, truncated at
startup). Run standalone: `npm ci` then `node split-aggregate.mjs`.

---

## `node-native/http-server.mjs`

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
tick scenarios (t2-json, split-aggregate, t2-realistic-eip, xsd, xslt)
write real records and the startup cell ignores the env, matching
their JVM/rust peers.
Run standalone: `BENCH_HTTP_URL=http://127.0.0.1:8080/bench node
<scn>.mjs`, then `POST /bench` with any body.

---

## `node-fastify/http-server.mjs`

T3 http-server fixture for the `node-fastify` contender: the same
protocol-A contract as `node-native/` served through
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
t2-json, split-aggregate, t2-realistic-eip, xsd, xslt write real
records; the startup cell ignores the env, matching their JVM/rust
peers). Run standalone: `npm ci` then
`BENCH_HTTP_URL=http://127.0.0.1:8080/bench node http-server.mjs`.

---

## `node-native/xsd-validation-bridge.mjs`

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
`import.meta.url` (`benchmarks/scenarios/xsd-validation-bridge/shared/bench-payload.xml`,
`benchmarks/scenarios/xsd-validation-bridge/shared/schema.xsd`) because the harness launches node cells with
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
`BENCH_PAYLOAD=benchmarks/scenarios/xsd-validation-bridge/shared/bench-payload.xml BENCH_SCHEMA=benchmarks/scenarios/xsd-validation-bridge/shared/schema.xsd
BENCH_LATENCY_FILE=/tmp/t4b-node-native.log node xsd-validation-bridge.mjs`.

---

## `node-fastify/xsd-validation-bridge.mjs`

T4b xsd-validation-bridge fixture for the `node-fastify` contender
(bench-node task 3.1): the same protocol-B contract as
`node-native/` (see its README for the full extraction — env
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
per-validation cost (see the node-native `xsd-validation-bridge.mjs`
header for the measured numbers and the caveat).

The module import, `fastify()` construction, route registration, and
`await app.ready()` (`<scn>.mjs` — the full avvio boot, the same
framework tax every co-contender pays before its marker: rust
`ctx.start().await`, Camel `Main.run`) run WITHOUT binding any
socket — this scenario has no wire protocol (no-bind rule for
protocol B), and the registered route is never served. After the
boot and self-test, the fixture arms the timer route: per tick
validate + `BENCH_LATENCY` record + `BENCH_XSD_TICK` log, marker
`BENCH_ROUTE_READY <unix_ms>` exactly once (`<scn>.mjs` — boot
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
`BENCH_PAYLOAD=benchmarks/scenarios/xsd-validation-bridge/shared/bench-payload.xml BENCH_SCHEMA=benchmarks/scenarios/xsd-validation-bridge/shared/schema.xsd
BENCH_LATENCY_FILE=/tmp/t4b-node-fastify.log node xsd-validation-bridge.mjs`.

---

## `node-native/xslt-bridge.mjs`

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
byte-pinned shared assets (`benchmarks/scenarios/xslt-bridge/shared/bench-payload.xml`,
`benchmarks/scenarios/xslt-bridge/shared/identity-transform.xsl`) the JVM/rust contenders read.

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
(`benchmarks/scenarios/xslt-bridge/smoke/node-native.log`) must carry the same digest. The output
itself is the payload doc copied verbatim minus the XML declaration
(`omit-xml-declaration="yes"` in the stylesheet; the leading comment
and all whitespace text nodes are copied by the identity template).

Observed behavior (task 3.2 evidence, dev host, node v22.23.2):
startup ~2s (node boot + xslt3 SEF compile ~0.7s + first transform),
then one `BENCH_XSLT_SELFTEST_SHA256=17713b3d54921b7d3c1420252685e94eca4689781258268e6c948ae5ae6742d9`,
single marker, and `BENCH_XSLT_TICK`/`BENCH_LATENCY` pairs at ~1-2ms
transform time per 10ms tick. Committed smoke evidence:
`benchmarks/scenarios/xslt-bridge/smoke/node-native.log`.

Run standalone: `npm ci --omit=dev` then
`BENCH_PAYLOAD=benchmarks/scenarios/xslt-bridge/shared/bench-payload.xml BENCH_STYLESHEET=benchmarks/scenarios/xslt-bridge/shared/identity-transform.xsl BENCH_LATENCY_FILE=/tmp/t4a-node-native.log node xslt-bridge.mjs`.

---

## `node-fastify/xslt-bridge.mjs`

T4a xslt-bridge fixture for the `node-fastify` contender (bench-node
task 3.2): the same protocol-B contract as `node-native/` (see its
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
documented in the node-native entry headers: cross-runtime byte-parity
is not asserted; the self-test digest
`BENCH_XSLT_SELFTEST_SHA256=<hex>` pins THIS fixture's output against
its committed smoke evidence (`benchmarks/scenarios/xslt-bridge/smoke/node-fastify.log`).

The module import, `fastify()` construction, route registration, and
`await app.ready()` (`<scn>.mjs` — the full avvio boot, the same
framework tax every co-contender pays before its marker: rust
`ctx.start().await`, Camel `Main.run`) run WITHOUT binding any
socket — this scenario has no wire protocol (no-bind rule for
protocol B), and the registered route is never served.
After the boot and self-test, the fixture arms the timer route: per
tick transform + `BENCH_LATENCY` record + `BENCH_XSLT_TICK` log,
marker `BENCH_ROUTE_READY <unix_ms>` exactly once (`<scn>.mjs` —
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
Committed smoke evidence: `benchmarks/scenarios/xslt-bridge/smoke/node-fastify.log`.

Run standalone: `npm ci --omit=dev` then
`BENCH_PAYLOAD=benchmarks/scenarios/xslt-bridge/shared/bench-payload.xml BENCH_STYLESHEET=benchmarks/scenarios/xslt-bridge/shared/identity-transform.xsl BENCH_LATENCY_FILE=/tmp/t4a-node-fastify.log node xslt-bridge.mjs`.
