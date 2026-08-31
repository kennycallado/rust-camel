T2 t2-realistic-eip fixture for the `node-fastify` contender: the
same protocol-B contract as `../node-native/` (see its README for the
full chain extraction and parity evidence — no payload axis, no env
contract, no `BENCH_INPUT_SHA256`; `set_body("ping")` → `set_header
(source="bench")` → `filter("${body} == 'ping'")` → `choice { when
("${header.source} == 'bench'") → "pong-bench"; otherwise →
"pong-other" }` → `log("BENCH_ROUTE_READY body=${body}")`, fired once
by `timer:bench?repeatCount=1&delay=0`) with the **Fastify 5.12.1**
application (pinned exactly in `package.json`; `package-lock.json`
is committed — run `npm ci --omit=dev` before the first run, that is
the harness build step) booted in front. The module import,
`fastify()` construction, route registration, and `await
app.ready()` (the full avvio boot — plugin loading, route
compilation, handler finalization, the same framework tax every
co-contender pays before its marker: rust `ctx.start().await`,
Camel `Main.run`) run WITHOUT binding any socket — this scenario has
no wire protocol (no-bind rule for protocol B), and the registered
route is never served. After the boot, the fixture performs the same
single route execution (`repeatCount=1` semantics): chain → marker
`BENCH_ROUTE_READY body=pong-bench` exactly once (when branch taken,
final body `pong-bench`; a wrong branch would read `body=pong-other`
and miss the harness grep), then idles until the smoke/harness kills
it. Run standalone: `npm ci --omit=dev` then `node route.mjs`.

Observed behavior (parity with the LIVE rust reference —
`./target/debug/t2-realistic-eip` after `cargo build -p
t2-realistic-eip-rust-camel-lib`): rust emits the marker
`BENCH_ROUTE_READY body=pong-bench` exactly once (preceded by its
static `BENCH_ROUTE_READY exchange_id=…` line — a rust-camel
static-log workaround documented in ../node-native/README.md, not
mirrored here) and no `pong-other`, then idles; this fixture emits
the identical single marker line `BENCH_ROUTE_READY body=pong-bench`
exactly once, no `pong-other`, then idles — same choice/when
outcomes, same final body. `ready()` lands at route.mjs:34, the
marker emission at :58 — boot strictly before the marker.
