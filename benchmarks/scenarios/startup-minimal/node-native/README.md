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
time-to-marker is unaffected. Run standalone: `node route.mjs`.
