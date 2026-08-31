// T1 startup-minimal fixture — node-native contender (bench-node task 2.1).
// Zero-dependency Node script, protocol B (process-spawn shape, like
// every contender in this scenario).
//
// Contract (extracted from the existing contenders — camel-standalone
// App.java/AppYaml.java, camel-quarkus BenchRoute.java + routes.yaml,
// rust-camel-lib main.rs, rust-camel-cli startup-minimal.yaml):
// - Route semantics: `timer:bench?repeatCount=1&delay=0` -> log
//   `BENCH_ROUTE_READY`. `delay=0` skips Camel's 1000ms default
//   initial delay; `repeatCount=1` fires exactly once. The script
//   equivalent of "timer fires once immediately on start" is a single
//   unconditional marker line at startup.
// - `BENCH_ROUTE_READY` on stdout exactly ONCE (the harness greps -F
//   the marker and validates the exact count; every existing fixture
//   also emits it exactly once).
// - No env contract: this scenario reads no BENCH_* variables. Timing
//   and RSS are captured by the harness from OUTSIDE the process
//   (single clock, GNU time -v); there is no latency file and no
//   canonical payload — the marker timing IS the scenario's output.
// - Exits 0 after the marker. The framework fixtures idle after the
//   marker until the harness kills them externally; a plain script
//   has no runtime to keep alive, so it exits — the harness's
//   post-marker kill is a no-op either way and time-to-marker is
//   unaffected.

console.log("BENCH_ROUTE_READY");
