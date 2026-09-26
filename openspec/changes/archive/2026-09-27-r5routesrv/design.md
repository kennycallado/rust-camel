# Design: r5routesrv

## Approach

**Verification-first convergence.** An empirical probe (2026-09-26,
debug binary 0.54.0-regular, worktree at f94dda95 — probe transcripts in
the parked JSON) established that the R5 run path ALREADY behaves as the
epic's deployment-equivalence demands:

- a compiled route artifact declaring a REST listener advertises it in
  `--manifest`, binds it, and answers HTTP 200 after boot;
- SIGTERM mid-request lets the in-flight request finish (3 s delay
  route, TERM at 1.5 s → response delivered, then exit 0);
- a plain timer artifact serves until TERM → exit 0 (it does NOT exit
  after boot — the bd's original premise predates R1);
- `camel run --routes <doc> --no-watch` behaves identically (200, TERM,
  exit 0) — deployment-equivalence holds;
- a compiled job artifact completes bounded (outcome `Completed`,
  exit 0) and never serves.

This is structural, not accidental: both route runtimes
(`run_embedded_route` v1, `run_embedded_store_route` v2) drive
`camel run`'s `drive_lifecycle`, which arms SIGINT/SIGTERM before boot
(buffered-during-boot), waits on the first stop signal, tears down
gracefully through `BootHandle::shutdown`, and force-exits 1 on a
second signal (rc-kz85m) — the same contract jobsignals landed for
`camel job`. R4's `verify_for_boot` runs before dispatch, hence before
any listener binds.

What is MISSING is everything that makes it a spec'd, regression-safe
surface: no artifact-level test pins listener serving, in-flight drain,
second-signal force-exit, boot-buffering, or deployment-equivalence; the
cli-compile spec (12 requirements / 82 scenarios) has no long-running
route-server requirement; the compile docs page never states the serve
semantics. R5 closes exactly that.

**Battery** additions to `compiled_artifact_test.rs`, reusing its
`compile`/`deploy_artifact`/`spawn_child`/`KillOnDrop`/`send_signal`/
`wait_for_marker`/`wait_exit_code` harness (REST fixture: `rest:` block
with base `path:` — an empty base path is rejected by the DSL — and a
`direct:` back-route with `set_body`):

1. serve-until-signal + exit 0 + completed `--report` JSON;
2. in-flight drain (delay route, TERM mid-request, response completes);
3. second-signal force-exit: INT+TERM pair at the mid-boot marker
   (`Starting CamelContext`) — both buffered during boot; the shutdown
   select consumes one, the force-exit guard polls the already-queued
   other → exit 1 + `forcing exit` WARN (deterministic, no teardown-
   duration race; same technique as `run_signal_test::
   second_sigterm_during_teardown_force_exits`);
4. signal during boot buffered (TERM at mid-boot marker → exit 0);
5. deployment-equivalence (`camel run --routes <doc> --no-watch`:
   serves, TERM → 0);
6. job artifact boundedness pinned explicitly (exits without any
   signal);
7. envelope-before-bind: required-signature artifact whose ENVELOPE
   bytes are corrupted (the artifact trailer stays valid, so decoding
   succeeds and boot verification is the failing step) → exit 2 naming
   the verification step; the test HOLDS the declared port across the
   child's execution (held-listener witness), so the child provably
   never binds it.

**Scope bound (bless finding #4):** the requirement text is
transport-agnostic ("listeners the embedded documents declare"), but the
R5 battery proves the REST/HTTP transport — the canonical listener the
manifest scanner itself documents. gRPC/WS sealed-artifact batteries are
recorded as a follow-up bd at park; they exercise the same
drive_lifecycle drain and signal seams, so no contract fork is possible.

**Contingency:** if any battery test runs red, the fix lands in the
existing seams (`drive_lifecycle` / `compile::runtime`), matching
jobsignals semantics exactly; e_opus is consulted only if drain
semantics fork against jobsignals (mission budget: 1).

## Affected crates

- camel-cli (tests only expected): `tests/compiled_artifact_test.rs`
  battery + `tests/common/mod.rs` helpers if a port-probe helper is
  needed; production code unchanged unless a probe-red forces a fix.
- docs: `docs/src/cli/compile.md` (Running an artifact → serve/signal/
  drain paragraph, docwave 277 style), `crates/camel-cli/CONTEXT.md`
  (compiled-artifacts paragraph gains the listener-serving sentence).
- openspec: `openspec/changes/r5routesrv/specs/cli-compile/spec.md` —
  pure ADDED requirement, no MODIFIED (no carry churn against the
  12-requirement canon; the guard stays strict anyway).

## Architecture boundaries

No DSL, component, or core changes: the listener is camel-http's
existing REST consumer; drain/teardown is the existing
`BootHandle`/ctx.stop path; signals are drive_lifecycle's existing
entry-registered streams. The artifact argument surface stays narrow
(`--report/--help/--version/--manifest/--verify`); no ambient config,
no watch, no new manifest fields (ADR-0075 boundary respected; ADR-0083
envelope check position unchanged — only pinned).

## Phases

Single-phase: one coherent slice (battery + spec + docs); no `## Phase`
headings in tasks.md.

## Alternatives considered

- MODIFY "Run embedded documents without extraction" to carry the
  serve semantics — rejected: the new requirement is orthogonal
  (long-running posture vs extraction contract), and pure ADD avoids
  carrying all 7 existing scenario names of a 545-line canon.
- Implement a dedicated artifact drain loop — rejected: drive_lifecycle
  already provides the exact jobsignals-matching semantics; duplicating
  it would fork the signal contract.
- gRPC/WS listener battery — bounded out of R5 (bless finding #4): the
  requirement is transport-agnostic and the drain/signal seams are
  shared, but the evidence is REST-only; gRPC/WS batteries file as a
  follow-up bd instead of overstating the claim.
