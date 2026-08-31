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
`timer:bench?repeatCount=1&delay=0` fires once, immediately, with an
empty exchange body; then `set_body("ping")` → `set_header(source =
"bench")` → `filter("${body} == 'ping'")` (always true under this
route — the body was set to the literal one step earlier — but
evaluated, not assumed; a false filter skips the choice block, and
the log after it stays unconditional in every fixture) →
`choice { when("${header.source} == 'bench'") → set_body
("pong-bench"); otherwise → set_body("pong-other") }` → `log
("BENCH_ROUTE_READY body=${body}")`. The when branch is the taken
one, so the final body is `pong-bench` and the marker reads
`BENCH_ROUTE_READY body=pong-bench` — printed exactly once (the
harness greps -F the full string and validates the count); a
wrong-branch run would read `body=pong-other` and fail the grep,
making the branch decision observable, not silent. After the marker
the script idles like the rust fixture (`ctrl_c().await`) until the
smoke/harness kills it. Run standalone: `node route.mjs`.

Semantic parity evidence (task 2.4, no committed golden — parity is
asserted against the LIVE rust reference, not a digest):
`cargo build -p t2-realistic-eip-rust-camel-lib` then
`timeout 8 ./target/debug/t2-realistic-eip` emits
`INFO BENCH_ROUTE_READY exchange_id=<uuid>` (static line) and
`INFO BENCH_ROUTE_READY body=pong-bench` (the marker, exactly once),
no `pong-other`, then idles — filter passed, when branch taken, final
body `pong-bench`. `node route.mjs` emits the identical marker line
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
