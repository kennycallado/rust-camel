# Design: cli-jobs

## Approach

The job is a scenario-action head pointed at the real boot path. The
document stays in the `*.test.yaml` family (a new mutually-exclusive
`execute:` section — the ADR-0069 Decision 6 tier-rule precedent: the
declared section classifies the document), but the execution reuses the
`camel run` composition seams verbatim: `load_config_or_default`,
`configure_context_with_beans`, the security compile context (or the
`ensure_security_supported` fail-closed fallback without the feature),
`install_bind_exposure_acks`, `canonical_project_root`, and
`camel_bundles::boot`. Route loading goes through
`camel_dsl::discover_routes_with_threshold_and_security` (ambient
`${env:}`, threshold, security context — the real-boot loader) for the
file forms; inline `routes:` go through `parse_routes_with_env` with the
ambient environment as lookup.

## Coupling guard outcome: job-local document types

Reusing `camel-integration-test`'s `ScenarioAction::Send` would pull the
hermetic interpreter (LayeredEnv, partner adapters, sealed config) into
a path that must see the real environment. The sanctioned fallback
applies: a minimal job-local send-action struct (`to` / `body` /
`headers`) parsed directly in `commands/job/document.rs`, grammar-mirroring
the scenario send. The unit-tier `TestDocError::RouteSourceConflict` and
`NoProjectRoot` variants are reused verbatim so route-source errors read
identically across the family.

## Consumer gate: why not `route_definitions_reference_scheme`

The reusable predicate scans `from_uri` AND every step URI, so using it
per deny-scheme would reject producers/sinks (`to: http:...` inside the
target route) — violating the locked "producers are fine" rule. The gate
is therefore from-uri-scoped: each route's `from:` scheme must be in the
fail-closed allowlist `{direct, seda, log, mock}` (scheme extraction is
a `split_once(':')`, mirroring the predicate's own URI handling).
`route_definitions_reference_scheme` is still reused for the conditional
`exec:` bundle registration, unchanged from `camel run`.

## Route-target safety

`camel job` forces `auto_startup = false` on every discovered route and
`true` on the route whose `from:` base (URI before query options)
matches the send target — the job's own trigger must start so the send
lands; other consumer routes must not. A send target with no matching
consumer route is a load-time exit-2 error. There is no public
single-route start API on `CamelContext` (the reload seam is
crate-private), so the auto_startup flip plus one `ctx.start()` is the
idiomatic composition.

## Send, timeout, and teardown

The send mirrors the unit-tier `deliver_input` / scenario
`DirectStimulus` discipline: producer created under the registry lock,
`oneshot` awaited outside it, with a bounded startup-race retry window
(3 s — the `deliver_input` 1 s deadline plus the SEDA readiness bound).
A `direct:` no-consumer error is a permanent route defect (same error
class as the race), which is exactly why the window is short: the defect
surfaces as a pipeline failure after ~3 s instead of spinning the whole
budget. The mandatory overall `timeout` is anchored at process start and
covers the whole run — boot, send, drain, teardown — via `timeout_at`;
expiry reports `Timeout` and exits 2. Teardown runs
`boot_handle.shutdown_with_deadline` under the remaining budget with a
5 s floor; a shutdown failure or timeout after a recorded verdict
appends to the report error and forces exit 2 (apparatus outranks
verdict, mirroring the test driver).

Seda verdict fidelity: the seda producer defaults to
`waitForTaskToComplete=IfReplyExpected`, which for an InOnly job send is
fire-and-forget — a failing route would report `Completed` and
`capture-reply` would echo the input. The runner therefore rewrites
`seda:` targets to carry `waitForTaskToComplete=Always` (replacing any
author-set value): the component honors `Always` unconditionally (the
producer attaches a reply channel and awaits the pipeline's
`send_and_wait` result), so the verdict and captured reply reflect the
route's outcome. Verified against the component source
(`WaitForTaskToComplete::Always => true` in `SedaProducer::call`); the
reject fallback was not needed.

`PipelineOutcome` mapping: the producer reply seam is the ADR-0024 §3.5
translation site — `Completed` and `Stopped` both arrive as
`Ok(exchange)`, `Failed` as `Err`. The job maps `Ok` to `Completed`
(exit 0) and `Err` to `Failed` (exit 1); `terminated_early` is reported
`false` because the seam deliberately erases `Stopped`. Surfacing it
needs a camel-core observation point (deferred with the EnvSource
unification).

## Report

`{document, mode, outcome, terminated_early, duration_ms, reply?,
error?}` serialized with `serde_json` to stdout (default) or
`--report`. The general tracing layer writes to stdout
(camel-config `init_tracing_subscriber`), so machine-parseable stdout
requires `log_level = "off"` in the config or `--report`; the
integration-test fixture sets `log_level = "off"`.

## Deferred (v1.1+)

- `Stopped` observation (`terminated_early` always `false`).
- WASM bean loading from `[beans]` (the run.rs inline block; the bundle
  cascade's wasm component support still boots).
- Signal streams (a one-shot killed by SIGTERM dies with the default
  disposition; the doc timeout bounds the run).
- Batch mode, EnvSource trait unification.

## Alternatives considered

**Lifecycle flags on `camel run` (`--once`).** Rejected: resident-mode
assumptions (watcher, signal loop, health) leak into every flag
interaction.

**A new `.command.yaml` file type.** Rejected: a second document family
duplicates parse/dispatch/validation; the reserved-suffix contract
(ADR-0062) already places family documents next to routes.

**Reusing `ScenarioAction` types.** Rejected by the coupling guard
(hermetic env machinery); the job-local struct is the sanctioned
fallback.
