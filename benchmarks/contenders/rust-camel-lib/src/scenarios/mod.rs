//! Scenario modules — one per benchmark scenario, each ported verbatim
//! from `benchmarks/scenarios/<scn>/rust-camel-lib/src/main.rs` (deleted
//! in task 1.2). File names keep the scenario names (hyphens); module
//! names are the underscore forms used by the dispatcher in `main.rs.

#[path = "http-server.rs"]
pub mod http_server;
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
