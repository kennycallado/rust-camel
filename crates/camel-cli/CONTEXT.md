# CLI

The command-line interface crate. Provides `camel run`, `camel new`, `camel plugin`, and other
subcommands for building and running Camel routes from the terminal.

## ADR-0012 log-policy sites

All 10 `error!` sites in this crate are class **system-broken** — CLI/bootstrap/shutdown lifecycle
failures where no `ErrorHandler` exists to own the ERROR. Per ADR-0012 taxonomy (§ Taxonomy, l.19)
these fall under letter-code **(d) CLI, bootstrap, application startup/shutdown**, distinct from the
route-lifecycle code **(c)** (`consumer_management.rs` / `route_controller.rs`). Both codes share the
`system-broken` class: `error!` is preserved and each call site carries a
`// log-policy: system-broken` annotation. Sites are cited by enclosing symbol; line numbers are the
current-HEAD positions and are secondary to the symbol.

| Symbol | File:Line | Code / class | Description |
|--------|-----------|--------------|-------------|
| `fn maybe_instrument_routes` | `commands/bench_instrument.rs:43` | (d) system-broken | cannot open `BENCH_LATENCY_FILE` |
| `async fn run` | `commands/run.rs:388` | (d) system-broken | failed to initialize SQL bundle |
| `async fn run` | `commands/run.rs:411` | (d) system-broken | failed to initialize SurrealDB bundle |
| `async fn run` | `commands/run.rs:476` | (d) system-broken | failed to add route definition |
| `async fn run` | `commands/run.rs:482` | (d) system-broken | route discovery failed |
| `async fn run` | `commands/run.rs:490` | (d) system-broken | `CamelContext` start failed |
| `async fn run` | `commands/run.rs:529` | (d) system-broken | file watcher failed |
| `async fn run` | `commands/run.rs:574` | (d) system-broken | shutdown error (`ctx.stop`) |
| `async fn run` | `commands/run.rs:584` | (d) system-broken | JMS pool shutdown failed |
| `async fn run` | `commands/run.rs:593` | (d) system-broken | CXF pool shutdown failed |

## Metrics

Metrics instrumentation for CLI commands is not yet wired; processor-crate instrumentation is tracked separately.
