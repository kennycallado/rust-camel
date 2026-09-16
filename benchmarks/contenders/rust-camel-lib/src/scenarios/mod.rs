//! Scenario modules — one per benchmark scenario, each ported verbatim
//! from `benchmarks/scenarios/<scn>/rust-camel-lib/src/main.rs` (deleted
//! in task 1.2). File names keep the scenario names (hyphens); module
//! names are the underscore forms used by the dispatcher in `main.rs.

// Measured http-server fixture: minimal-bare (e_opus ruling D1,
// 2026-09-16; bd rc-h42s6). Gated symmetrically with the trace variant
// below so a `bench-trace` build contains ONLY the trace route and a
// default build contains ONLY the minimal route.
#[cfg(not(feature = "bench-trace"))]
#[path = "http-server.rs"]
pub mod http_server;
// Smoke-trace variant (e_opus ruling D1, 2026-09-16; bd rc-h42s6):
// compiled ONLY with the `bench-trace` feature — the harness M1–M4
// builds NEVER enable it, so the default binary contains no trace
// path (prove with benchmarks/harness/checks/trace-absent.sh).
#[cfg(feature = "bench-trace")]
#[path = "http-server-trace.rs"]
pub mod http_server_trace;
#[path = "split-aggregate.rs"]
pub mod split_aggregate;
#[path = "startup-minimal.rs"]
pub mod startup_minimal;
#[path = "t2-json.rs"]
pub mod t2_json;
#[path = "t2-realistic-eip.rs"]
pub mod t2_realistic_eip;
#[path = "xsd-validation-bridge.rs"]
pub mod xsd_validation_bridge;
#[path = "xslt-bridge.rs"]
pub mod xslt_bridge;
