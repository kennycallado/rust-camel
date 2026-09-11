# Proposal: cli-jobs

## Why

`camel run` is a resident process (consumers, watchers, signal loop) and
`camel test` is hermetic (mocks, layered env). Neither runs a route ONCE
against the real composition root: operators scripting pipelines (CI
steps, cron one-shots, batch triggers) must boot a resident runtime and
kill it. A one-shot verb with a JSON report and a strict exit taxonomy
fills that gap without lifecycle flags on `camel run` (resident-mode
assumptions) or a new file type.

## What Changes

- New top-level `execute:` section in the existing `*.test.yaml`
  document family, mutually exclusive with `scenario:` and the unit-tier
  vocabulary (`expects`, `inputs`, ...). `camel test` classifies it as a
  third section and refuses it with a pointer to `camel job`.
- New `camel job <doc>` subcommand: real boot composition (the same
  seams `camel run` uses — config load, security compile context, bind
  acks, the `camel_bundles` cascade, ambient `${env:}` discovery), the
  document-family route-source keys (`routeFiles` / `routeFilesFromRoot`
  / `routes`, same exactly-one conflict rule), then exactly one send to
  a `direct:` or `seda:` endpoint under a mandatory overall `timeout`.
- Side-effect safety: every discovered route is forced
  `auto_startup = false` except the send target's consumer route, which
  is forced on. A fail-closed load-time gate rejects documents whose
  routes consume from any scheme outside `{direct, seda, log, mock}`;
  producers/sinks as `to:` URIs are unrestricted.
- JSON report (stdout default, `--report <path>` optional): document,
  mode, outcome (`Completed` / `Failed` / `Timeout`), `terminated_early`,
  `duration_ms`, `reply` when `capture-reply` is set, `error` when
  applicable.
- Exit codes mirror the test-driver taxonomy: 0 completed; 1 pipeline
  failed; 2 load, validation, boot, drain-timeout, shutdown, and
  report-write errors.

## Explicitly excluded

- `mode: batch` — the key parses but is rejected at load with a clear
  "not available yet" error (reserved for a follow-up change).
- No unification of the scenario interpreter's env machinery into a
  shared `EnvSource` trait (deferred, v1.1+).
- No signal streams, no watcher, no Prometheus, no health port.
- No surfacing of `Stopped` vs `Completed` (the producer reply seam
  erases the distinction by design, ADR-0024 §3.5; `terminated_early`
  reports `false` until a camel-core observation seam exists).

## Acceptance criteria

- `camel job doc.test.yaml` with a `direct:` route exits 0 and reports
  `Completed`; `capture-reply` carries the reply body/headers.
- A route whose pipeline fails exits 1 with a `Failed` report.
- `mode: batch`, missing/invalid `timeout`, non-`direct:`/`seda:` send
  targets, mixed `execute:`+`scenario:`, and non-job-safe consumer
  schemes all exit 2 at load with named errors.
- `openspec validate cli-jobs --type change --json` passes.
- `cargo test -p camel-cli --lib` and the new integration tests green;
  clippy and doc gates green, including the sql-only feature set.
