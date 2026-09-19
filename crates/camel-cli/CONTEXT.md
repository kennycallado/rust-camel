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
| `async fn run_job` | `commands/job/mod.rs:450` | (d) system-broken | `camel job`: component cascade boot failed |
| `async fn run_job` | `commands/job/mod.rs:462` | (d) system-broken | `camel job`: route loading failed |
| `async fn run_job` | `commands/job/mod.rs:558` | (d) system-broken | `camel job`: failed to add route definition |
| `async fn run_job` | `commands/job/mod.rs:578` | (d) system-broken | `camel job`: `CamelContext` start failed |
| `async fn run_job` | `commands/job/mod.rs:639` | (d) system-broken | `camel job`: send apparatus failure (producer/endpoint, not a pipeline verdict) |
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

## camel job signal handling

`camel job` arms its signal streams at the first lines of `run_job`,
before config loading, so a signal arriving during boot (config load,
bundle cascade, route discovery, context start) is buffered by the runtime
and consumed by the send/drain race instead of hitting the default
disposition and killing the process (spec: signal during boot is buffered;
mirrors `camel run` entry registration). Arming is document-runs only: the
no-argument listing path never installs the streams, because handlers
whose streams are never consumed would swallow SIGINT/SIGTERM during
listing instead of letting the default disposition terminate the process.
The first SIGINT or SIGTERM cancels the
in-flight send or batch drain, runs bounded teardown, and reports outcome
`Interrupted` with exit code 2. The teardown budget is mode-dependent:
one-shot floors the wall-clock remaining to the overall deadline at
`MIN_SHUTDOWN_BUDGET` (5 s), while batch keeps the no-floor rule —
teardown cannot run past the overall deadline, so a spent deadline
computes a zero budget there. A `shutdown_error` is recorded only when
teardown had a non-zero budget; a zero-budget failure — whether on the
interrupted-batch path or the timeout path — is a foregone artifact
(stderr-only, the verdict keeps the report).

After the first signal is consumed, a force-exit guard owns the streams: a
second SIGINT or SIGTERM during teardown exits 1 immediately without
waiting for the bounded shutdown (rc-kz85m — orchestrators resend the stop
signal after their grace period). Identical signal bursts may coalesce
before delivery (tokio semantics), so strict signal counting is not
guaranteed. The wait race is signal-first: a `biased` select polls the
signal arm before the operation, so a signal ready at the same poll point
as send completion or deadline expiry wins deterministically (spec:
signal-first tie).

On Unix the SIGINT and SIGTERM streams are registered at entry, so the
buffered-during-boot guarantee holds for the whole boot stretch. On
non-Unix platforms there is no SIGTERM stream; the portable Ctrl+C
listener is awaited inside the wait race, which first runs at the
send/drain race — tokio installs the console handler only when the first
`ctrl_c()` future is polled, so a Ctrl+C during boot hits the default
disposition and terminates the process outright (no `Interrupted` report,
no exit 2). The covered stretch on non-Unix starts at the send/drain race.

## camel job failure modes

`camel job <doc>` runs one `*.job.yaml` document declaring a top-level `execute:` section (mode `one-shot`/`batch`, one `direct:`/`seda:` send, mandatory `timeout`, one family route source, optional `description:` for listing). A bare name resolves `<name>.job.yaml` across ordered `[jobs].dirs` roots (`[jobs].dir` remains a one-root compatibility alias, default `jobs`, anchored at the Camel.toml root); an explicit path always wins. `dirs` takes precedence when both keys exist. `camel job` with no argument lists the discovery set — name plus relative path when needed and description via a cheap probe parse, `(unparseable)` siblings tolerated, empty/absent roots exit 0. The metadata walk uses lexical order, root depth 0, depth limit 8, 512 files per root, no directory symlinks, and one warning per truncated root. Bare-name lookup remains root-level only. It boots the REAL composition root (the `camel run` seams: config, security context, bind acks, the `camel_bundles` cascade, ambient `${env:}` discovery). After the boot config (ambient or embedded) parses and validates, a job boot projection removes the durable runtime journal and replaces ambient observability with defaults before context configuration, for every job form including compiled artifacts; malformed ambient config still fails loud; ambient repositories remain config-driven. It starts every document route and relies on the load-time consumer allowlist for side-effect safety, with the send target as the sole entry point, sends one exchange, tears down through `BootHandle::shutdown_with_deadline`, and emits a JSON report to stdout (or `--report`) for outcomes that reach the send (verdict, interruption, or timeout) plus shutdown failures after a verdict; early exit-2 classes are stderr-only. Seda targets are rewritten to `waitForTaskToComplete=Always` so the send is synchronous (verdict fidelity). Exit precedence mirrors `camel test`: `2 > 1 > 0`. Route side-effect safety is fail-closed: documents whose routes consume (`from:`) from any scheme outside `{direct, seda, log, mock}` are rejected at load; `to:` URIs are unrestricted. `mode` accepts `one-shot` and `batch`; other values are rejected at load. The general tracing layer writes to stdout, so a machine-parseable stdout report needs `log_level = "off"` or `--report`.

Documents MAY declare job arguments in a top-level `args:` block beside
`execute:`. Names match `[A-Za-z_][A-Za-z0-9_]*`; each declaration admits
`required` (boolean), `default` (string), and `description` (string), and MAY
carry `type` (`string` default, `int`, `bool`, or `enum[...]` as one string
scalar with trimmed, non-empty, unique members); unknown fields fail. Typed
values coerce at resolution, coercion and typed-default failures exit 2, and
`${arg:NAME}` substitutes the canonical string form. Every
`--arg NAME=VALUE` pair on a declared document must name a declaration;
unknown and missing-required names exit 2, defaults fill omissions, and
explicit pairs win. Resolved values interpolate through `${arg:NAME}` in
`to`, `body`, `headers`, and `timeout` before field validation, at the same
scanner stage as `${env:NAME}`. The `arg:` namespace never falls through to
the environment, `:-fallback` is rejected, and unresolved names exit 2.
Declared documents inject no implicit headers; legacy documents keep raw
send fields and header injection with one deprecation note on stderr.

| Failure mode | Trigger | Exit code |
|--------------|---------|-----------|
| Doc load error | unreadable file, non-`*.job.yaml` suffix (a `*.test.yaml` declaring `execute:` gets rename guidance), missing `execute:`, mixed `scenario:`/unit-tier sections, serde/grammar errors, unsupported `mode` (accepted: `one-shot`, `batch`), missing/invalid `timeout`, non-`direct:`/`seda:` send target, route-source conflict, no `Camel.toml` ancestor for `routeFilesFromRoot`, bare-name miss across configured `[jobs].dirs` roots | 2 |
| Argument validation | invalid `args:` declaration or `type` grammar, typed-default or resolved-value coercion failure, undeclared or missing-required `--arg`, unresolved `${arg:}`/`${env:}` in a declared document | 2 |
| Job-safety rejection | a discovered route consumes from a scheme outside the `{direct, seda, log, mock}` allowlist; or the send target has no matching consumer route, or its base is ambiguous across several; or the route source resolves zero routes | 2 |
| Boot failure | config load, context configure, security compile context, `camel_bundles::boot`, route discovery/parse, route registration, `ctx.start()` | 2 |
| Pipeline failure | the send's route pipeline failed (`PipelineOutcome::Failed` through the producer reply seam) | 1 |
| Overall timeout | the mandatory `timeout` expired before send+drain+teardown completed (report outcome `Timeout`) | 2 |
| Signal interruption | first SIGINT/SIGTERM cancelled the in-flight send or batch drain (report outcome `Interrupted`; teardown runs under the one-shot `MIN_SHUTDOWN_BUDGET` floor or the batch no-floor deadline) | 2 |
| Send apparatus failure | producer/endpoint creation failed past the 3 s startup-race window (not a pipeline verdict) | 2 |
| Shutdown failure | teardown failed or exceeded its budget after a recorded verdict (report `shutdown_error` carries the detail; `error` keeps the verdict) | 2 |
| Report write failure | `--report` path unwritable, or report serialization failed | 2 |

## Compiled artifacts (`camel compile`)

`camel compile <document> -o <artifact>` produces a self-contained native
Linux preview artifact: a copy of the current executable with a trailer
appended (ADR-0075). The payload is captured before `${env:}` interpolation,
so `${env:NAME}` expressions resolve from the deployment environment at
artifact runtime, never from the compile environment. This crate owns both
sides of the format: `compile::sources` (`SourceSelection`, `resolve`)
resolves compile inputs, `compile::trailer` owns the `CAMELTR1` codec
(v1 68-byte footer, v2 76-byte footer, `decode_artifact` dispatch), and
`compile::manifest` owns the operational manifest with its independent
`manifest_schema: 2` and `embedded_files` list. The canonical store model
lives in camel-dsl and is re-exported verbatim; this crate never defines a
second one.

Compile-time source selection is explicit. Sources come only from
`--config <Camel.toml>` and repeated `--profile <name>`; `--profile`
without `--config` is rejected. Without `--config` the compiler embeds no
configuration and selects no profile, and it never discovers ambient
configuration. Resolution stays confined to the selected root: names
normalize to UTF-8 relative `/` paths, and absolute paths, `.`/`..`
components, non-UTF-8 names, symlink escapes, duplicate canonical targets,
duplicate logical paths, and out-of-root references fail with exit 2
before any output is created. Declared pattern order is preserved and each
pattern's matches sort by normalized logical path, so identical inputs
produce identical artifact bytes. The aggregate normalized embedded bytes
stay capped at 16 MiB. The store packs typed entries (`route`, `job`,
`config`, `include`, `profile`) plus a canonical index (`store_schema: 1`,
one logical entry point, ordered source plan). Compilation also fails
closed (exit 2, no output) for non-native targets, for unsupported
asset-bearing endpoint fields (certificates, private keys, CA files,
WASM/plugin files, XSLT/XSD, SQL files, static directories, literal secret
files, dynamic placeholders in those fields), and for compile-time
`CAMEL_*` overrides. Explicitly declared route files, includes, profile
sections, and job route sources are the supported virtual documents, not
assets. Asset-bearing endpoint URI schemes (`wasm:`, `xslt:`,
`validator:`) stay rejected in every URI-bearing field. Runtime endpoint
URI paths (e.g. `file:`, `kafka:`, `log:`), runtime `${env:}` expressions
outside forbidden fields, and deploy-side network/file I/O remain
permitted.

The binary self-detects its trailer before Clap parsing (`fn
self_detect_artifact`): a trailer-free image keeps the normal CLI; marked
corruption, an unsupported trailer version, or an unsupported
store/manifest schema exits 2 with an integrity diagnostic; a valid
artifact accepts only `--report <path>`, `--help`, `--version`, and
`--manifest` (anything else exits 2 naming the argument). `--manifest`
prints the manifest — schema, runtime version, kind, `embedded_files` metadata,
components, required environment names, and listeners — and exits 0 without boot.
Decoding yields an `EmbeddedRequest`: v1 trailers adapt to
`EmbeddedRequest::SingleDocument` with unchanged behavior; v2 trailers
build `EmbeddedRequest::VirtualStore` with the decoded store, manifest,
and one entry point. Route artifacts call
`camel_dsl::discover_virtual_store`, register every referenced route, and
reuse the existing `camel run` boot, context start, signal shutdown,
report, and exit handling with watch disabled; job artifacts consume the
embedded job/config/route entries through the existing job outcome
lifecycle. Virtual source diagnostics use the `compiled://<logical-path>`
identity. The runtime performs no source or config reads, no globbing, no
canonicalization, no extraction, and no watch; only deployment-time
endpoint I/O and `${env:}` resolution run. Stores are validated before
boot, and unknown schemas, invalid references, malformed ranges, kind
mismatches, checksum failures, or missing configuration entries exit 2
with zero routes booted, so artifacts run from a read-only root and a file
placed beside the artifact after compilation is never loaded.

Route `--report <path>` writes the exact JSON object
`{"kind":"route","status":"completed"|"failed","error":string|null}` after
boot/runtime completion or failure, with exit 0 for graceful completion and
2 for boot/discovery/report-write failure (1 stays reserved for job pipeline
failures). Job artifacts keep the existing job outcome report and exit
precedence (`2 > 1 > 0`). Boot still performs parse, interpolation, lowering,
component boot, and context start, so no boot-speed claim is made before P0
measurement.

Extension boundaries sit on the store, not on new formats. R2 (deploy-time
asset embedding) may add embedded files to the store; R3 (multi-entry
artifacts) may extend entry-point cardinality using the same store without
changing R1 runtime semantics. Compression, signing, and cross-target
compilation stay deferred.

## Build profiles

The `camel` binary composes its dependency closure from two orthogonal
axes: the allocator override and the feature profile.

**Allocator policy.** Default builds use the platform system allocator
(glibc malloc on gnu images; the Dockerfile musl production image opts
into jemalloc). `jemalloc` is the sole opt-in override: it backs musl
production builds and powers the allocator gauges (`allocator_metrics.rs`,
ADR-0066) and heap profiling (see below). The alternative backend feature
(`mimalloc`) was removed as dead surface — it was structurally
unreachable whenever jemalloc was enabled and no build path enabled it
(bd rc-rrz6a).

**Feature profiles.** `default = ["flavor-regular"]`. The flavor markers
`flavor-slim`, `flavor-regular`, and `flavor-full` are the single
selection surface for profiles; CI legs pass only `flavor-*` markers.
Bodies are CHAINED: each flavor includes the marker of the flavor below
it, so slim ⊆ regular ⊆ full is structural (asserted by
`flavor_chain_is_structural` in `tests/feature_profiles.rs`).

- `flavor-slim = ["mqtt", "mqtt-tls", "http-static", "sql",
  "lang-jsonpath", "lang-rhai"]` — the edge pack on top of the base
  components (core, direct, seda, log, file, timer, http, template,
  master, cron, controlbus, mock, validator, camel test harness).
- `flavor-regular = ["flavor-slim", "otel", "grpc", "wasm", "llm",
  "mcp", "security", "redis", "redis-tls", "jms", "cxf", "xj", "xslt",
  "opensearch", "ws", "lang-xpath", "lang-js", "lang-minijinja", "lsp",
  "kubernetes", "integration-http", "integration-sql"]`.
- `flavor-full = ["flavor-regular", "exec", "kafka", "surrealdb",
  "containers"]`.

Regular excludes ONLY the four principled items: kafka (C dependency,
unportable to musl), surrealdb (BUSL-1.1, the only non-OSI license in
the graph), exec (arbitrary host-binary execution, ADR-0037), and
containers (infrastructure-daemon client: camel-function +
camel-component-container as ONE feature). The contract sets in
`tests/feature_profiles.rs` (`SLIM_FORBIDDEN_PREFIXES`,
`REGULAR_FORBIDDEN_PREFIXES`, `REGULAR_REQUIRED_PREFIXES`,
`FULL_REQUIRED_PREFIXES`) gate the closures, and `full_covers_universe`
fails CI when a feature is placed in no flavor. The legacy `full`
feature (historical closure list) remains for compose-users; flavors no
longer reference it. The `slim-http` and `slim-benchmarks` aliases were
dropped at 0.50 (bd rc-n6iop). Overlapping markers report by priority
full > regular > slim; a raw composition with no marker reports
`custom`. `camel --version` prints the flavor as a suffix
(`camel 0.49.0 (regular)`) while the compiled-artifact manifest stays
semver-only. The kafka surface is exactly two features:
`kafka` (capability — component activation plus registration in the lint
registry and the boot cascade; librdkafka source build via cmake — the
feature forwards `camel-component-kafka/cmake-build`, because the
rdkafka-sys default build (mklove/sh) cannot build on Windows or cross
targets, bd rc-2ii8l) and
`dynamic-linking` (capability plus system-librdkafka linking; it implies
`kafka`). The historical `cmake-build` and `kafka-static` names were
removed (bd rc-5t5fo.1): they activated the dependency without enabling
registration. `lsp` and each `lang-*` feature are individually
selectable and compose additively with any flavor marker.
`containers` gates the Docker-daemon client stack as ONE feature
(function⇒container per ADR-0005): the camel-function runtime service
and the camel-component-container bundle registration. Without it, no
function runtime is constructed and the `function:`/`container:` paths
error explicitly. `kubernetes` forwards camel-config's platform wiring
(pod identity, readiness-gate patching); it is demand-gated, no longer
hardcoded on the dependency.
`camel-language-minijinja` stays linked in every profile: the workspace
consumes camel-template with default features (the engine is optional at
the source since mission 115; the in-workspace flip needs the
named-feature shape that forces golden regeneration, tracked as bd
rc-gcs5d, discovered from rc-9720m). The eight bridges (jms, sql, redis,
opensearch, ws, cxf, xslt, xj) are optional via same-named camel-cli
features, each activating the own dependency and forwarding the
camel-bundles gate. `full` enables all eight. Slim drops seven of them;
redis remains via the unconditional camel-config → camel-redis-repo path
(out of zone). `redis-tls` implies `redis`. camel-cli's `http-static`
feature forwards camel-bundles' HttpStaticBundle gate only.

The integration-sql CI profile
(`cargo test -p camel-cli --no-default-features --features
integration-sql,itest-e2e` in `.github/workflows/integration-sql.yml`)
is lang-free and lsp-free BY DESIGN since this change: the SQL
independence proof must not pull the language runtimes or the LSP stack.
If a future itest fixture needs a language expression, add the `lang-*`
feature to that job's feature list then.

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
