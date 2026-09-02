//! Consolidated `rust-camel-lib` fixture (OpenSpec change
//! `bench-consol-tick`, task 1.1).
//!
//! One binary serving all seven scenarios that previously each had their
//! own `benchmarks/scenarios/<scn>/rust-camel-lib` crate. `argv[1]` selects
//! the scenario; dispatch goes straight to that scenario's `run()` — no
//! other scenario's route builder ever executes (dispatch-then-build
//! guard, design §Measurement integrity: "no scenario-dispatch work before
//! the marker; the marker fires on the same code-path position as today").
//!
//! Each `scenarios::<mod>::run()` is the verbatim `main()` of the old
//! per-scenario fixture (route construction, marker emission contract,
//! `BENCH_INPUT_SHA256` logging, `BENCH_PAYLOAD_BYTES` body building,
//! `BENCH_LATENCY_FILE` reading), wrapped as `pub fn run() -> i32`
//! (process exit code: 0 success, 1 route/context error).

mod scenarios;

/// The valid argv[1] values — printed on unknown/missing scenario.
const SCENARIOS: [&str; 7] = [
    "startup-minimal",
    "http-server",
    "t2-json",
    "split-aggregate",
    "t2-realistic-eip",
    "xsd-validation-bridge",
    "xslt-bridge",
];

fn main() {
    // Dispatch-then-build guard: NOTHING runs before the selected
    // scenario's `run()` — no tracing init, no env reads, no context
    // construction. Non-selected route builders never execute.
    let code = match std::env::args().nth(1).as_deref() {
        Some("startup-minimal") => scenarios::startup_minimal::run(),
        Some("http-server") => scenarios::http_server::run(),
        Some("t2-json") => scenarios::t2_json::run(),
        Some("split-aggregate") => scenarios::split_aggregate::run(),
        Some("t2-realistic-eip") => scenarios::t2_realistic_eip::run(),
        Some("xsd-validation-bridge") => scenarios::xsd_validation_bridge::run(),
        Some("xslt-bridge") => scenarios::xslt_bridge::run(),
        Some(other) => {
            eprintln!("error: unknown scenario '{other}'");
            usage();
            2
        }
        None => {
            eprintln!("error: missing scenario argument (argv[1])");
            usage();
            2
        }
    };
    std::process::exit(code);
}

fn usage() {
    eprintln!("valid scenarios: {}", SCENARIOS.join(", "));
}
