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

## camel test failure modes

`camel test` runs each `*.test.yaml` document in-process and reports one `PASS`/`FAIL` line per endpoint or asserted reply, then a final `N passed, M failed` summary. Exit-code precedence is `2 > 1 > 0`: any parse-error class forces 2, else any failed endpoint or reply assertion forces 1, else 0. A document-level error is reported to stderr and execution continues with the next document. Directory arguments expand recursively to `*.test.yaml`/`*.test.yml` documents (sorted, with `target`/`.git`/`node_modules` skipped).

| Failure mode | Trigger | Exit code |
|--------------|---------|-----------|
| Doc parse error | unreadable file, invalid YAML, or `TestDocError` from `parse_test_document` | 2 |
| Boot failure | `CamelContext` boot, route load, route start, or input delivery fails | 2 |
| Expansion error | zero-document directory argument or unreadable directory during walk | 2 |
| Settle timeout | traffic does not quiesce within the quiet window plus the 5s instability budget | 1 |
| Assertion failure | expectation mismatch reported by `MockEndpointInner::try_assert_satisfied` | 1 |
| Reply assertion failure | `expectReply` mismatch on a captured reply (FAIL reply line) | 1 |
| Zero-survivor misuse | a filter set admits no document (at least one filter given); stderr names the filters | 2 |
| JUnit write failure | `--junit` report cannot be written; stderr message names the path | 2 |

Precedence when classes mix: any parse-error class ⇒ 2, else any failed endpoint ⇒ 1, else 0. The settle timeout, assertion failure, and reply assertion failure all surface as a `FAIL` line and count toward `failed`; parse-error and boot failures surface on stderr and do not count toward `passed`/`failed`.

The `intercepts` block in `*.test.yaml` maps source URIs to `skipTo` or `divertCopyTo` `mock:` targets before route load; see [Declarative camel test — Intercepts](../../docs/src/testing/index.md#intercepts) and the [route-interception spec](../../openspec/specs/route-interception/spec.md). The `beans:` block declares stub beans (echo, setBody, fail) for `bean:` steps; see [Declarative camel test — Bean stubs](../../docs/src/testing/index.md#bean-stubs). An input may declare `expectReply` to assert against the reply message the `direct:` producer returns; see [Declarative camel test — Reply assertions](../../docs/src/testing/index.md#reply-assertions). The `repositories:` block registers named `cache`, `idempotent`, and `claimCheck` repositories as in-memory stubs for the run; see [Declarative camel test — Repository stubs](../../docs/src/testing/index.md#repository-stubs). The `--junit <FILE>`, `--filter-file <GLOB>`, and `--filter-endpoint <NAME>` flags shape a run for CI: the report path, the file glob, and the endpoint name filter; see [Declarative camel test — CI output and filters](../../docs/src/testing/index.md#ci-output-and-filters).
