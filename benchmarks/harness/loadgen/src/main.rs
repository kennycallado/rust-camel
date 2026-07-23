//! bench-loadgen CLI dispatcher.
//!
//! Subcommands:
//!
//! - `devnull` — start HTTP devnull stub (loopback baseline target).
//! - `calibrate` — Protocol A calibration phase (spec §4.4).
//! - `measure-a` — Protocol A measurement (warmup + n rounds × n samples).
//! - `measure-throughput` — M3 sustained-throughput measurement.
//! - `parse-protocol-b` — parse `/tmp/v3-protocol-b-<cell>.log`.
//! - `rss-sample` — sample process-tree RSS at fixed cadence.
//! - `bridge-check` — compare bridge PIDs at round boundaries.
//! - `aggregate-bridge-tax` — pair rust vs java per scenario from a tree
//!   of `m2-summary.json` files (spec F1 bridge tax reporting).
//!
//! See `bench_loadgen` library docs for the spec citations per command.

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        usage();
        return ExitCode::from(2);
    }
    let cmd = args[1].as_str();
    let rest = &args[2..];
    match cmd {
        "help" | "--help" | "-h" => {
            usage();
            ExitCode::SUCCESS
        }
        "devnull" => bench_loadgen::cli::devnull_main(rest),
        "calibrate" => bench_loadgen::cli::calibrate_main(rest),
        "measure-a" => bench_loadgen::cli::measure_a_main(rest),
        "measure-throughput" => bench_loadgen::cli::measure_throughput_main(rest),
        "parse-protocol-b" => bench_loadgen::cli::parse_protocol_b_main(rest),
        "rss-sample" => bench_loadgen::cli::rss_sample_main(rest),
        "bridge-check" => bench_loadgen::cli::bridge_check_main(rest),
        "aggregate-bridge-tax" => bench_loadgen::cli::aggregate_bridge_tax_main(rest),
        other => {
            eprintln!("error: unknown subcommand '{other}'");
            usage();
            ExitCode::from(2)
        }
    }
}

fn usage() {
    eprintln!(
        "usage: bench-loadgen <subcommand> [args]\n\
         \n\
         subcommands:\n\
         \n  devnull [--port=PORT]              Start HTTP devnull baseline stub.\
         \n  calibrate --url=URL                 Protocol A calibration (spec §4.4).\
         \n  measure-a --url=URL --rate=N --samples=N [--rounds=N]\
         \n                                       Protocol A measurement.\
         \n  measure-throughput --url=URL [--duration-secs=N] [--warmup-secs=N] [--workers=N]\
         \n                                       M3 sustained-throughput measurement.\
         \n  parse-protocol-b --log=PATH         Parse /tmp/v3-protocol-b log + print stats.\
         \n  rss-sample --pid=PID --interval-ms=N --duration-ms=N\
         \n                                       Sample process-tree RSS.\
         \n  bridge-check --start=PATH --end=PATH  Compare bridge PID files.\
         \n  aggregate-bridge-tax --input-dir=PATH [--output=PATH]\
         \n                                       Pair rust vs java per scenario.\
         \n\
         \nThis is the v3 benchmark harness loadgen (spec §4.5). See the library\
         \ndocs (cargo doc -p bench-loadgen --open) for spec citations per command."
    );
}
