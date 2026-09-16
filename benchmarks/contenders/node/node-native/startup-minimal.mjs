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
// - Parks after the marker (wrapper-asym ruling R2, e_opus
//   2026-09-16): the process stays alive until the harness's
//   post-marker KILL, so `time -v` sees the same peak-RSS
//   termination cause (external SIGKILL) as every idle fixture in
//   the family. Self-exiting handed time -v a different physical
//   instant (V8/libuv teardown) — a directional RSS bias and a
//   stdout-flush/exit race. Marker position is unchanged; the M1
//   wall clock (marker observation) is untouched.

console.log("BENCH_ROUTE_READY");

process.stdin.resume();
await new Promise(() => {});
