# Proposal: r5routesrv

## Why

R5 of the compile roadmap (epic rc-rye74, bd rc-zs7au) converges compiled
route artifacts on **deployment-equivalence**: a route document that runs
under `camel run` in a deployment posture must, when compiled, serve
identically — bind its listeners (REST/HTTP; gRPC/WS where the document
declares them), serve until the first SIGINT/SIGTERM, drain gracefully,
and exit 0.

Code inspection shows the run path largely exists: both route-artifact
runtimes (`run_embedded_route` v1, `run_embedded_store_route` v2) drive
`camel run`'s `drive_lifecycle`, which arms signal streams before boot,
waits on the first stop signal, tears down gracefully, and force-exits 1
on a second signal (rc-kz85m). But **none of this is pinned for
listener-bearing artifacts**: every compiled-artifact battery test uses a
timer→log route, the second-signal escape hatch has no artifact-level
test, no test proves a listener-bearing document actually binds and
serves from a sealed artifact, and the cli-compile spec has no
requirement block for long-running route-server semantics. The original
bd text ("route docs currently exit after boot") predates R1; the honest
R5 delta is verification-first convergence plus whatever real gaps the
battery exposes.

## What Changes

- **Battery** (camel-cli `compiled_artifact_test.rs` + harness): route
  artifact with a REST listener binds and serves HTTP after `--manifest`
  advertises the listener; in-flight request completes during graceful
  drain; first signal → graceful exit 0 with `{"kind":"route","status":"completed"}`;
  second signal during teardown → force exit 1; job artifacts keep
  exit-after-completion; deployment-equivalence (same doc under
  `camel run --no-watch` and compiled, same serve/stop/exit behavior).
- **Run path**: close only the gaps the probe/battery finds (expected
  small; e.g. marker/log wording, report-on-interrupt wording). No new
  signal machinery — drive_lifecycle + the jobsignals-landed semantics
  are the contract.
- **R4 interaction**: pin by test that envelope verification happens
  BEFORE any listener binds (signed artifact with tampered envelope
  binds nothing).
- **Spec**: cli-compile delta — one ADDED requirement block
  ("Long-running route-server artifacts") with scenarios; no MODIFIED
  blocks (avoid carry churn against the 12-requirement/82-scenario
  canon).
- **Docs**: camel-cli CONTEXT.md compiled-artifacts section + compile
  docs page gain the route-server semantics paragraph (docwave 277
  style).

Excluded: deploy-side TLS material delivery (owner decision, bd
rc-p823t), R6 compression, R7 cross-target, any watch/hot-reload.

## Acceptance criteria

- A compiled route document that declares listeners serves until SIGTERM,
  drains gracefully, exits 0, and writes the completed route report.
- First signal graceful / second signal force-exit 1 — pinned by
  artifact-level tests, semantics matching jobsignals exactly.
- Envelope verification precedes listener binding — pinned by test.
- Job artifacts unchanged (exit-after-boot-completion preserved).
- `openspec validate r5routesrv --type change` clean; canonical spec
  gains the requirement block at archive.

## Risk budget

Risk: the listener probe reveals a real serving gap in the sealed path —
then implementation grows (w_heavy drain-loop work as ordered). Out of
bounds: touching job outcome semantics, widening the artifact argument
surface, new manifest fields, ambient config reads.
