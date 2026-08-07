# Design: audit-fix-docdrift-t1-baseline

## Approach

Mechanical documentation-hygiene pass over the five T1 crates. No
architectural change; the changes are comments, rustdoc, README text, and
corrected error-message strings (with their coupled tests). Each finding is
resolved in isolation by crate, so tasks are independently dispatchable.

The one item that touches runtime text is builder M4: the error strings emitted
by `build_canonical()` say `"canonical v1"` but ADR-0016 established v2 as the
current canonical spec version. The fix changes the literals to `v2` and
updates the assertions in `canonical_spec_test.rs` and the inline tests in
`lib.rs` that pin the exact substring. This is an intentional diagnostic-text
compatibility change: rejection and compilation semantics are preserved (the
same steps are rejected the same way; only the version label in the message
moves). Exact-string consumers must update from `canonical v1` to
`canonical v2` in lockstep.

## Affected crates

- **camel-api** — `claim_check.rs`: replace 5 phantom
  `CamelError::NotFound(...)` rustdoc references with the actual variant the
  functions return (verified against `error.rs` at task time). `lib.rs`:
  remove the `TODO(API-006)` comment and document why no trait re-export is
  made in `camel-api` (the re-export is a separate API-surface decision outside
  D1; implementing it here is prohibited to avoid scope creep).
- **camel-config** — `context_ext.rs:143`, `config.rs:19`, `config.rs:45`:
  remove or rewrite the three `TODO(CONFIG-004)` comments; hot-reload is wired
  and consumed (CLI `run.rs` reads `camel_config.watch`).
- **camel-dsl** — `README.md`: rebuild the step-coverage tables from the
  shipped step set.
- **camel-cli** — `README.md`: add `plugin` and `openapi` to the top-level
  command list (sections already exist lower down), remove the stray
  `## Overview` block, add `--otel`/`--otel-endpoint`/`--service-name` to the
  `camel run` flags. `CONTEXT.md:31`: remove the dead `TODO(PROC-004)`
  citation (verify PROC-004 status at task time; if open, the comment must be
  accurate, not a dangling pointer).
- **camel-builder** — `lib.rs` (~11 literal sites) and `tests/canonical_spec_test.rs`
  (2 assertion sites): `"canonical v1"` → `"canonical v2"` consistently (M4).
  The builder M2 (BUILDER-003/006 numbering collision) is out of scope here,
  split to bd rc-z6zw.

## Architecture boundaries

This change does not cross the data/control-plane boundary. It touches only
comments, documentation, and one error-message literal. No `Service<Exchange>`
implementation, no `Component`/`Endpoint`/`Consumer` trait, no canonical
compilation logic changes. The M4 string edit is inside `build_canonical()`
but alters only the formatted error text, not the rejection decision.

## Relevant ADRs

- **ADR-0011 / ADR-0016** — canonical route spec versioning. v2 is current;
  this is why the M4 `"canonical v1"` strings are stale.
- **ADR-0012** — log-level convention. Not directly in scope (no log-line
  edits), but the `lint-log-levels` gate must stay green.

## Alternatives considered

- **Leave findings advisory (defer post-freeze).** Rejected: clean docs is an
  owner-confirmed freeze requirement, and the `lint-context-citations` gate
  (D2) needs a closed baseline to calibrate against.
- **Implement api M5 (API-006 re-export) as a real feature.** Rejected and
  prohibited: the re-export is a separate API-surface decision. D1 removes the
  stale `TODO(API-006)` comment and records why no re-export is made; it does
  not add the re-export.

## Phases

Single-phase. The findings are small, independent, and share no ordering
constraint beyond landing together as one baseline before D2.
