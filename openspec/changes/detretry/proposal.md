# Proposal: detretry

## Why

The job send loop's OUTER transport arm (`crates/camel-cli/src/commands/job/mod.rs`,
`send_with_startup_retry`'s `Err(detail)` branch) retries every transport
failure unconditionally for the full 3 s `SEND_RETRY_WINDOW` — no classifier
is consulted. Deterministic (terminal-class) failures burn the whole window
on errors no retry can ever clear. This is the sedaretry holistic-review
finding (bd rc-zovuy, discovered-from rc-rif19) and the same defect family
as the gate (rc-ucemm) and terminal-config (rc-rif19) inner-arm fixes.

Observable case: a job document sending to `seda:foo?size=5` against a route
consuming `from: seda:foo?size=10` — `is_compatible_with` rejects the config
conflict deterministically, yet the send loop sleeps and retries for 3 s
before failing. Bad endpoint parameters (`CamelError::InvalidUri` from
direct/seda URI parsing) behave the same.

## What Changes

- `attempt_send`'s outer error changes from `String` to a typed
  `TransportFailure` enum (component-not-registered / endpoint-creation /
  producer-creation), each carrying the cause `CamelError` instead of
  erasing it. Display output stays byte-identical to today's three
  `format!` strings (report fidelity).
- New classifier `is_deterministic_transport_failure`: registry miss is
  deterministic by construction; `CamelError::InvalidUri` is deterministic
  structurally (rc-utx98 variant-decides doctrine — bad URI/param parse
  never becomes valid); seda terminal-config markers classify through the
  existing bounded source-chain walk. Everything else keeps the retryable
  default (component-owned marker doctrine — foreign components opt in by
  adding markers).
- `camel-component-seda`: the `is_compatible_with` config conflict gains a
  typed marker — `TerminalConfigError` enum grows an `EndpointConfigConflict`
  variant; the rejection detail stays byte-identical, carried by
  `EndpointCreationFailedWithSource`. `is_seda_terminal_config_error`
  matches both variants; `is_direct_startup_race` narrows accordingly
  (deterministic conflict fails fast on the inner arm too — consistent).
- Deterministic transport failures return `SendError::Transport` on the
  FIRST attempt without sleeping; transient failures keep the existing
  window semantics.
- Keycloak boundary: job send targets are restricted to `direct:`/`seda:`
  at document load (cli-jobs spec), so keycloak endpoint/role config errors
  cannot reach the job transport arm — no keycloak marker is added
  (documented in design.md; the bd's reachability claim is refuted by the
  allowlist).
- Folded comment drift: the three `is_direct_startup_race` call sites
  (`commands/test/runner.rs`, `commands/test_support.rs`,
  `camel-integration-test/src/adapters.rs`) still say only "the seda gate
  fails fast" — updated to name the terminal-config exclusion too.

Excluded: keycloak marker work (unreachable seam), foreign-component
markers (each component opts in when a real seam needs one), inner-arm
semantics beyond what the narrowed seda predicate implies.

## Acceptance criteria

- Deterministic transport failure (config conflict, `InvalidUri`, registry
  miss) exits the send loop after the FIRST attempt — elapsed far below
  the 3 s window, pinned by behavioral and subprocess e2e tests.
- Transient-class failures (queue-full plain `EndpointCreationFailed`,
  foreign plain failures) keep the existing retry semantics — pinned by
  classifier characterization tests.
- All rejection and transport Display strings stay byte-identical to
  today (pinned).
- The three sibling call-site comments name both fail-fast exclusions.

## Risk budget

- Regression risk concentrated in the classifier boundary: a
  misclassification that fails fast on a genuinely transient failure would
  break the startup-race retry the window exists for. Mitigated by
  pinning both sides with tests (rc-rif19 pattern).
- Byte-fidelity of report/error strings is a hard requirement (pinned
  tests); any wording change is a defect.
- No behavior change for foreign components (retryable default).

## Affected crates

- `camel-component-seda` (marker variant, rejection conversion, predicate
  doc, unit tests)
- `camel-cli` (typed transport error, classifier, loop, doc comments,
  classification tests, subprocess e2e)
- `camel-integration-test` (call-site comment only)

Bd: rc-zovuy
