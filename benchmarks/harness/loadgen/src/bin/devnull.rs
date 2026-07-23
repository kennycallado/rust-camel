//! Standalone bench-devnull binary.
//!
//! Same HTTP devnull as `bench-loadgen devnull`, but packaged as a
//! separate bin target so the harness can launch it without CLI dispatch
//! overhead (and so unit tests can spawn it with `Command::new`).

use std::process::ExitCode;

fn main() -> ExitCode {
    let port: u16 = std::env::var("BENCH_DEVNULL_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(18080);
    match bench_loadgen::cli_runtime::run_devnull(port) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("bench-devnull: error: {e}");
            ExitCode::FAILURE
        }
    }
}
