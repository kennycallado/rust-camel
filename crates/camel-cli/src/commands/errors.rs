//! Centralized CLI error helpers.
//!
//! Provides a single point for emitting CLI-subcommand failure logs +
//! exiting with the appropriate status code. Used by the `run` subcommand
//! (and future subcommands) to keep error-handling consistent.
//!
//! Per ADR-0012, CLI bootstrap/subcommand failures are category (d) — keep
//! `error!` with `// log-policy: system-broken` annotation. The annotation
//! lives at each call site in the subcommand bodies.

use camel_api::CamelError;

/// Report a fatal CLI subcommand failure: print to stderr and exit(1).
///
/// Category (d) per ADR-0012 — the caller's `error!` invocation MUST be
/// preceded by `// log-policy: system-broken`. This helper exists to
/// deduplicate the `eprintln!` + `std::process::exit(1)` pattern, NOT to
/// replace the `error!` log (which is the ADR-mandated operator signal).
///
/// Use when the failure is a CLI-bootstrap problem (config read, route
/// discovery, context start) that should terminate the process with exit
/// code 1.
pub fn report_cli_failure_and_exit(ctx: &str, e: &CamelError) -> ! {
    eprintln!("camel-cli {ctx} failed: {e}");
    std::process::exit(1);
}

/// Report a fatal CLI subcommand failure with a non-CamelError error type.
///
/// Same semantics as [`report_cli_failure_and_exit`], for use when the
/// underlying error is not a `CamelError` (e.g., `std::io::Error` from
/// signal handling, `anyhow::Error` from a dependency).
// Reserved for future non-CamelError failure paths (e.g., signal-handler io::Error); unused in Phase C Task 2.
#[allow(dead_code)]
pub fn report_cli_failure_msg_and_exit(ctx: &str, msg: impl std::fmt::Display) -> ! {
    eprintln!("camel-cli {ctx} failed: {msg}");
    std::process::exit(1);
}
