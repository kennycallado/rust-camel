# CLI

The command-line interface crate. Provides `camel run`, `camel new`, `camel plugin`, and other
subcommands for building and running Camel routes from the terminal.

## ADR-0012 log-policy sites

This crate keeps eleven ADR-0012 `error!` sites. All are class **system-broken**:
CLI/bootstrap/shutdown lifecycle failures where no `ErrorHandler` exists to own
the ERROR. Per the ADR-0012 taxonomy (§ Taxonomy, l.19) these fall under
letter-code **(d) CLI, bootstrap, application startup/shutdown**, distinct from
the route-lifecycle code **(c)** (`consumer_management.rs` / `route_controller.rs`).
Both codes share the `system-broken` class. `error!` is preserved and each call
site carries a `// log-policy: system-broken` annotation. Sites are cited by
enclosing symbol. Line numbers are current-HEAD positions and are secondary to
the symbol.

Five former `camel run` sites moved to the boot cascade crate (ADR-0069
section 10). The table lists them under `camel-bundles` with their new anchors.
See `crates/camel-bundles/CONTEXT.md`.

| Symbol | File:Line | Code / class | Description |
|--------|-----------|--------------|-------------|
| `fn maybe_instrument_routes` | `commands/bench_instrument.rs:99` | (d) system-broken | cannot open `BENCH_LATENCY_FILE` |
| `async fn run` | `commands/run.rs:367` | (d) system-broken | failed to add route definition |
| `async fn run` | `commands/run.rs:402` | (d) system-broken | route discovery failed (detail sites share the failure arm) |
| `async fn run` | `commands/run.rs:415` | (d) system-broken | `CamelContext` start failed |
| `async fn run` | `commands/run.rs:469` | (d) system-broken | file watcher failed |
| `async fn run` | `commands/run.rs:513` | (d) system-broken | `BootHandle` teardown failed (log-and-continue, exit code unchanged) |
| `async fn run_job` | `commands/job/mod.rs:233` | (d) system-broken | `camel job`: component cascade boot failed |
| `async fn run_job` | `commands/job/mod.rs:245` | (d) system-broken | `camel job`: route loading failed |
| `async fn run_job` | `commands/job/mod.rs:317` | (d) system-broken | `camel job`: failed to add route definition |
| `async fn run_job` | `commands/job/mod.rs:325` | (d) system-broken | `camel job`: `CamelContext` start failed |
| `async fn run_job` | `commands/job/mod.rs:349` | (d) system-broken | `camel job`: send apparatus failure (producer/endpoint, not a pipeline verdict) |
| `fn boot` | `crates/camel-bundles/src/lib.rs:306` | (d) system-broken | moved to camel-bundles (boot cascade): failed to initialize SQL bundle. See `crates/camel-bundles/CONTEXT.md` |
| `fn boot` | `crates/camel-bundles/src/lib.rs:329` | (d) system-broken | moved to camel-bundles (boot cascade): failed to initialize SurrealDB bundle. See `crates/camel-bundles/CONTEXT.md` |
| `BootHandle::shutdown_with_deadline` | `crates/camel-bundles/src/lib.rs:103` | (d) system-broken | moved to camel-bundles (boot cascade): shutdown error from `ctx.stop`. See `crates/camel-bundles/CONTEXT.md` |
| `BootHandle::shutdown_with_deadline` | `crates/camel-bundles/src/lib.rs:111` | (d) system-broken | moved to camel-bundles (boot cascade): JMS pool shutdown failed. See `crates/camel-bundles/CONTEXT.md` |
| `BootHandle::shutdown_with_deadline` | `crates/camel-bundles/src/lib.rs:123` | (d) system-broken | moved to camel-bundles (boot cascade): CXF pool shutdown failed. See `crates/camel-bundles/CONTEXT.md` |

## Signal handling contract

`camel run` treats SIGINT and SIGTERM with the same rules.

The signal streams are armed at the start of `run`, before boot. A signal
that arrives during boot (config load, component boot, route discovery,
context start) is buffered, not default-killed. The run finishes boot and
then shuts down gracefully (rc-z5zch TERM, rc-ukwlt INT).

The first SIGINT or SIGTERM, at any time, starts a graceful shutdown: the
file watcher is cancelled, the context stops, and the component pools tear
down. The exit code is 0.

A subsequent SIGINT or SIGTERM, after the first was consumed and graceful
shutdown has begun, force-exits the process with code 1 (rc-kz85m). This is
the escape hatch for a hung teardown. Systemd and `docker stop` resend the
stop signal after their grace period, so the escape hatch must accept both
signals. Identical signal bursts may coalesce before delivery (tokio
semantics), so strict signal counting is not guaranteed.

On non-unix platforms there is no SIGTERM stream; the portable Ctrl+C
listener is the first-signal handler, and a second Ctrl+C force-exits.

## camel job failure modes

`camel job <doc>` runs one `*.test.yaml` document declaring a top-level `execute:` section (mode `one-shot`, one `direct:`/`seda:` send, mandatory `timeout`, one family route source). It boots the REAL composition root (the `camel run` seams: config, security context, bind acks, the `camel_bundles` cascade, ambient `${env:}` discovery), forces `auto_startup = false` on every route except the send target's consumer route, sends one exchange, tears down through `BootHandle::shutdown_with_deadline`, and emits a JSON report to stdout (or `--report`) for outcomes that reach the send plus shutdown failures after a verdict; early exit-2 classes are stderr-only. Seda targets are rewritten to `waitForTaskToComplete=Always` so the send is synchronous (verdict fidelity). Exit precedence mirrors `camel test`: `2 > 1 > 0`. Route side-effect safety is fail-closed: documents whose routes consume (`from:`) from any scheme outside `{direct, seda, log, mock}` are rejected at load; `to:` URIs are unrestricted. `mode: batch` parses but is rejected with a reserved-mode error. The general tracing layer writes to stdout, so a machine-parseable stdout report needs `log_level = "off"` or `--report`.

| Failure mode | Trigger | Exit code |
|--------------|---------|-----------|
| Doc load error | unreadable file, non-`*.test.yaml` suffix, missing `execute:`, mixed `scenario:`/unit-tier sections, serde/grammar errors, `mode: batch` (reserved), missing/invalid `timeout`, non-`direct:`/`seda:` send target, route-source conflict, no `Camel.toml` ancestor for `routeFilesFromRoot` | 2 |
| Job-safety rejection | a discovered route consumes from a scheme outside the `{direct, seda, log, mock}` allowlist; or the send target has no matching consumer route, or its base is ambiguous across several; or the route source resolves zero routes | 2 |
| Boot failure | config load, context configure, security compile context, `camel_bundles::boot`, route discovery/parse, route registration, `ctx.start()` | 2 |
| Pipeline failure | the send's route pipeline failed (`PipelineOutcome::Failed` through the producer reply seam) | 1 |
| Overall timeout | the mandatory `timeout` expired before send+drain+teardown completed (report outcome `Timeout`) | 2 |
| Send apparatus failure | producer/endpoint creation failed past the 3 s startup-race window (not a pipeline verdict) | 2 |
| Shutdown failure | teardown failed or exceeded its budget after a recorded verdict (report error carries the detail) | 2 |
| Report write failure | `--report` path unwritable, or report serialization failed | 2 |

## Metrics

Metrics instrumentation for CLI commands is limited to the jemalloc memory
sampler (`allocator_metrics.rs`): with the `jemalloc` feature, `camel run`
samples allocated/resident/active/mapped every 5 s and emits
`camel_allocator_memory_bytes{stat}` through the context's late-bound handle
(ADR-0066); read failures warn and retry, init failure disables the sampler.
`tikv-jemalloc-ctl` must stay in lockstep with `tikv-jemallocator`. Processor-crate
instrumentation is tracked separately.

## Heap profiling

The `jemalloc` feature also compiles in heap profiling (jemallocator
`profiling` feature). Profiling stays off by default. Enable it at process
start with `prof:true` through the allocator envvar; the tikv symbol prefix
makes that envvar `_RJEM_MALLOC_CONF` (the musl-jemalloc-verify workflow
sets both spellings). This is a startup opt-in: a pod restart enables or
disables it, and runtime switching through `prof.active` would need extra
machinery. While profiling runs, the jemalloc ctl option `prof.dump` writes
a heap dump. musl/aarch64 profiling and the k8s dump path are not verified
yet; the human-dispatched musl-jemalloc-verify workflow owns that check
(bd rc-i9f9).

## camel test failure modes

`camel test` runs each `*.test.yaml` document in-process and reports one `PASS`/`FAIL` line per endpoint or asserted reply (unit tier) or per scenario action (full tier), preceded by one `[lean]`/`[full]` tier annotation line per executed document, then a final `N passed, M failed` summary. The tier is content-derived (`camel-integration-test::derive_tier`; a `scenario:` section forces full). Exit-code precedence is `2 > 1 > 0`: any parse-error, misuse, or apparatus class forces 2, else any verdict failure forces 1, else 0. A document-level error is reported to stderr and execution continues with the next document. Directory arguments expand recursively to `*.test.yaml`/`*.test.yml` documents (sorted, with `target`/`.git`/`node_modules` skipped). Documents declaring `scenario:` dispatch to the scenario parser (`parse_scenario_document`); when the CLI is built with `integration-http` and every wired endpoint scheme is `direct`, `http`, or `fake`, the document runs through the embedded FULL-tier boot (one harness `HttpPartner` per `http` endpoint bound on `127.0.0.1:0`, each `bindVar` folded into the layered environment, the real composition root, whole-document run, then teardown — ADR-0069 sections 4-5, 10). A `fake:`-only document keeps the no-boot smoke path in any build; any other scheme (or `http`/`direct` without the feature) reports `infra-unavailable` naming the adapter.

| Failure mode | Trigger | Exit code |
|--------------|---------|-----------|
| Doc parse error | unreadable file, invalid YAML, `TestDocError` from `parse_test_document`, or `DocError` from `parse_scenario_document` (doc-validation class) | 2 |
| Boot failure | unit-tier `CamelContext` boot, route load, route start, or input delivery fails; full-boot scenario partner bind (`partner-bind-failure`), sealed config load, or composition-root boot (`full-boot-failure`) | 2 |
| Expansion error | zero-document directory argument or unreadable directory during walk | 2 |
| Settle timeout | traffic does not quiesce within the quiet window plus the 5s instability budget | 1 |
| Assertion failure | expectation mismatch reported by `MockEndpointInner::try_assert_satisfied` | 1 |
| Reply assertion failure | `expectReply` mismatch on a captured reply (FAIL reply line) | 1 |
| Scenario verdict failure | `receive-timeout`, `validation-mismatch` (FAIL action line) | 1 |
| Scenario apparatus failure | runtime `scenario-var-unresolved` (authoring bug, rc-whof), `action-transport-failure`, `partner-startup-failure`, `shutdown-failure` (FAIL action line, ADR-0069 §7); `partner-startup-failure` is reserved in v1 — no adapter separates bind from handler start, bind failures report `partner-bind-failure` on stderr | 2 |
| Infra unavailable | scenario endpoint scheme has no partner adapter in this build; stderr names the adapter | 2 |
| Harness wiring error | a `send`/`receive` endpoint escapes the harness-built adapter map (doc-validation class, never a silent `ReceiveTimeout`) | 2 |
| Tier filter collision | an explicitly named document derives the tier the `--unit`/`--integration` filter excludes | 2 |
| Tier flags misuse | `--unit --integration` together; rejected before any document is read | 2 |
| Zero-survivor misuse | a filter set admits no document (at least one filter given); stderr names the filters | 2 |
| JUnit write failure | `--junit` report cannot be written; stderr message names the path | 2 |

Precedence when classes mix: any parse-error, misuse, or apparatus class ⇒ 2, else any failed endpoint, reply, or scenario verdict ⇒ 1, else 0. The settle timeout, assertion, reply, and scenario verdict failures all surface as a `FAIL` line and count toward `failed`; parse-error, boot, and apparatus failures surface on stderr and do not count toward `passed`/`failed`. A `shutdown-failure` after a recorded verdict reports both and keeps exit 2.

The `intercepts` block in `*.test.yaml` maps source URIs to `skipTo` or `divertCopyTo` `mock:` targets before route load; see [Declarative camel test — Intercepts](../../docs/src/testing/index.md#intercepts) and the [route-interception spec](../../openspec/specs/route-interception/spec.md). The `beans:` block declares stub beans (echo, setBody, fail) for `bean:` steps; see [Declarative camel test — Bean stubs](../../docs/src/testing/index.md#bean-stubs). An input may declare `expectReply` to assert against the reply message the `direct:` producer returns; see [Declarative camel test — Reply assertions](../../docs/src/testing/index.md#reply-assertions). The `repositories:` block registers named `cache`, `idempotent`, and `claimCheck` repositories as in-memory stubs for the run; see [Declarative camel test — Repository stubs](../../docs/src/testing/index.md#repository-stubs). The `env:` block declares string fixture values consulted before inline defaults; see [Declarative camel test — Env fixtures](../../docs/src/testing/index.md#env-fixtures). The `--junit <FILE>`, `--filter-file <GLOB>`, `--filter-endpoint <NAME>`, `--unit`, and `--integration` flags shape a run for CI: the report path, the file glob, the endpoint name filter, and the symmetric tier filters (excluding the opposite tier by derived tier — silently for expanded documents, `tier-filter-collision` for explicitly named ones); the JUnit report carries a `<property name="tier">` row per suite; see [Declarative camel test — CI output and filters](../../docs/src/testing/index.md#ci-output-and-filters).

LEAN tier route loading resolves `${env:}` placeholders against the document `env:` map first, then each token's inline `:-default` (rc-l7m7t; ADR-0069 section 13.1). Route files load through `camel_dsl::load_from_file_with_env` with the document-env lookup, and inline `routes:` through the same `camel_dsl::parse_routes_with_env` seam (`fn load_routes`, `commands/test/runner.rs:166`) — the same parser `camel run` uses, including the 16 MiB cap and path-annotated errors. Typing has typed-probe boot parity (env-int-placeholder-typing): a whole-scalar `:-default` token at a string-valued position resolves to its default, a whole-scalar placeholder on an integer-typed field LOADS through the loader's typed probe (also when the document `env:` map supplies the value), and an unset variable without a default still fails the document naming the variable; boolean-typed positions and non-integer defaults still fail. The process environment is never consulted, so runs stay hermetic.

The unit-tier document identifier fields interpolate `${env:NAME:-default}` against the document `env:` map first, then inline defaults, through the camel-dsl scanner, for name-match parity with the interpolated route sources (rc-l7m7t; rc-4hexo, ADR-0069 section 4). These fields are the `repositories` and `beans` map keys, the `intercepts` source keys and action targets, the `mock:` references in `expects` keys and `sequence` entries, and the `inputs[].to` values. Interpolation runs right after deserialization and before the other document-validation guards, so the scheme, blank-name, and built-in-name checks see the resolved value. Assertion data never interpolates, and the path fields `routeFiles` and `routeFilesFromRoot` never interpolate: input `body` and `headers`, `expectReply` blocks, matcher contents, bean `methods` and `config` values, repository stub targets, and `settle` all keep the literal text. An identifier placeholder without a default fails document validation at exit 2 naming the variable and the field, mirroring the route-side wording; the ambient environment is never consulted, so the run stays hermetic.
