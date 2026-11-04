# Proposal: lint-numeric-env-default

## Why

bd rc-jq775 reports that `camel lint` emits an R-SCHEMA Error for a numeric
`${env:VAR:-default}` at a numeric schema position (the reporter's example:
`steps -> delay -> delay_ms` fed by `${env:OPS_X:-20000}`, quoted and
unquoted) while the boot path accepts the document. The issue cites the
typing mirror's `WholeScalarEnvToken::WithDefault` documentation — "the leaf
is validated as the STRING default with numeric/boolean re-inference
suppressed" — as the deliberate cause.

Verification on v0.65.0 (the reporting release; `git diff v0.65.0..main` is
empty for both lint files) shows the reported false positive does NOT
reproduce. The typed carve-out shipped in rc-93wct rev 2 (and the boolean
follow-up bd8b906cf) already validates clean-integer and clean-boolean
defaults as NUMBER/BOOL copies whenever the STRING copy flags the leaf's
chain — including the ancestor-anchored anyOf shape the reported route uses
(`DelayBody` is `anyOf[integer, DelayFullConfig]`, nested inside the
43-branch `RouteDslStep` anyOf). A 20-shape probe matrix (bare/envelope/
legacy-array/rest forms, quoted/unquoted, single/dual tokens, choice and
do-try nesting, flow style, shorthand `delay:` body, retry policies) found
zero boot-accepts-lint-rejects cases. The one real defect in the neighborhood
is the documentation the bd quotes: `WholeScalarEnvToken::WithDefault`'s doc
comment still describes the pre-carve-out behavior, so the next reader (or
reporter) repeats the misdiagnosis.

Three residual defects survive verification:

1. **Stale typing-mirror documentation** — `env_interpolation.rs`'s
   `WithDefault` variant doc claims numeric/boolean re-inference is
   suppressed with no mention of the carve-out pass that follows in
   `rschema.rs`. This comment produced the bd's wrong root cause.
2. **No regression pins for the reported shape family** — existing carve-out
   tests cover plain-property positions (`throttle.max_requests`,
   `rest.port`, `mcp.max_tools`) and `auto_startup`, but nothing covers the
   anyOf-wrapped full-config positions the report names: `delay_ms` under
   `DelayBody`'s anyOf, or `retry.multiplier` (a NUMBER-typed position).
   A refactor could silently reintroduce the false positive.
3. **Misleading Info note under a boot-correct Error** — a substituted
   default that boot does NOT coerce (a float like `${env:M:-2.0}` at the
   NUMBER-typed `retry.multiplier` position, or an empty default) yields
   BOTH the boot-parity Error AND an Info note saying "validated against
   substituted default" — the Info contradicts the Error that accompanies
   it.

## What Changes

- Correct the stale `WholeScalarEnvToken::WithDefault` (and module-header)
  documentation in `crates/camel-lint/src/env_interpolation.rs` so it states
  the full behavior: STRING typing at collection time, then the typed
  carve-out pass in `rschema.rs` re-validating clean-integer / clean-boolean
  defaults as NUMBER/BOOL when the STRING copy flags the leaf's chain.
- Add regression pins in `crates/camel-lint/src/rules/rschema/tests.rs` for
  the reported shape family: `delay_ms` (shorthand integer body AND
  full-config form, quoted and unquoted, bare and envelope forms) and
  `retry.multiplier` (clean-integer default at a NUMBER-typed position) —
  all produce no diagnostic.
- Add boot-parity pins for the float exclusion: `retry.multiplier:
  ${env:M:-2.0}` and `retry.jitter_factor: ${env:J:-0.5}` keep their
  Error-severity diagnostic (boot's clean-integer probe rejects float
  defaults — the Error is boot-correct), and emit no Info note.
- Suppress the substituted-default Info note when an Error-severity
  R-SCHEMA diagnostic's span covers the same leaf, so the note never
  contradicts a concurrent Error. Two declared consequences: (a) ONE
  existing guard-arm pin migrates — the `${env:AS:-1}` integer-default-at-
  boolean-position case currently asserts one Info note alongside the
  Error (`rschema/tests.rs` guard-arm test); under the suppression rule its
  Info expectation becomes zero while its Error assertion stays. (b)
  Collateral Info suppression: an ancestor-anchored Error span can cover
  SIBLING leaves that are individually valid (e.g. a valid
  `dynamic_header` beside an invalid float `delay_ms` under the same
  `DelayBody` anyOf); their Info notes are suppressed too. Both consequences
  are bounded to Info output — no Error is removed, downgraded, or
  relocated.

Explicitly excluded:

- No change to carve-out Conditions A/B (the ancestor-anchored span
  coverage that `span_covers` provides stays untouched — relaxing it was
  the rejected first-attempt design and over-carves under shared anyOf
  nodes).
- No float coercion anywhere: boot's `env_int_probe` coerces only clean
  integers and exact-lowercase booleans, so lint must not carve float
  defaults at any position. (A potential boot-side float-coercion feature
  is filed as a bd follow-up, out of scope here.)
- No downgrade of the float/empty-default Error to Advice: boot rejects
  those inputs, so the Error is honest.

## Acceptance criteria

- `WholeScalarEnvToken::WithDefault`'s documentation and the
  `env_interpolation.rs` module header state the carve-out behavior and are
  consistent with `rschema.rs`'s implementation docs.
- New pins: `delay_ms` unquoted/quoted, shorthand-body and full-config
  forms, bare and envelope documents — zero R-SCHEMA diagnostics.
- New pins: `retry.multiplier` with clean-integer default at the
  NUMBER-typed position — zero R-SCHEMA diagnostics.
- New pins: `retry.multiplier` / `retry.jitter_factor` with float defaults —
  exactly one Error, zero Info notes.
- Existing guard-arm pins that assert Info survival when the leaf's chain
  is error-free keep passing unchanged; the ONE pin asserting Info
  alongside an Error (`${env:AS:-1}` at `auto_startup`) migrates to zero
  Info with its Error assertion retained, as declared above.
- New pin: collateral suppression — a valid sibling `dynamic_header` Info
  note disappears beside an invalid float `delay_ms` under the same anyOf,
  while a disjoint string-position Info note survives; no Error is removed.
- All quality gates green in the worktree.

## Risk budget

- Acceptable: doc-only churn in `env_interpolation.rs` (comments only; the
  SYNC header pins the behavioral counterparties and must stay accurate).
- Acceptable: Info-note suppression changes Info output only — it cannot
  hide or downgrade an Error, so no false-negative budget is consumed.
- Out of bounds: any relaxation of Condition A/B; any float/numeric-string
  coercion; any behavior change to boot (`camel-dsl`/`camel-config`);
  changes to `clean_integer`/`clean_boolean` lexical gates (SYNC-pinned
  across three crates).
