# CLI

The command-line interface crate. Provides `camel run`, `camel new`, `camel plugin`, and other
subcommands for building and running Camel routes from the terminal.

## ADR-0012 log-policy sites

All sites in this crate are category **(c) system-broken** — bootstrap/shutdown lifecycle failures
where no ErrorHandler exists to own the ERROR. The `error!` level is preserved; each call site
carries a `// log-policy: system-broken` annotation.

| File | Line | Category | Annotation |
|------|------|----------|------------|
| `commands/run.rs` | 389 | (c) system-broken | `// log-policy: system-broken` — failed to add route definition |
| `commands/run.rs` | 395 | (c) system-broken | `// log-policy: system-broken` — route discovery failed |
| `commands/run.rs` | 403 | (c) system-broken | `// log-policy: system-broken` — CamelContext start failed |
| `commands/run.rs` | 442 | (c) system-broken | `// log-policy: system-broken` — file watcher failed |
| `commands/run.rs` | 487 | (c) system-broken | `// log-policy: system-broken` — shutdown error |
| `commands/run.rs` | 497 | (c) system-broken | `// log-policy: system-broken` — JMS pool shutdown failed |
| `commands/run.rs` | 506 | (c) system-broken | `// log-policy: system-broken` — CXF pool shutdown failed |

## Metrics

Metrics instrumentation is not yet wired for CLI commands. See TODO(PROC-004) in individual
processor crates for the broader instrumentation gap.
