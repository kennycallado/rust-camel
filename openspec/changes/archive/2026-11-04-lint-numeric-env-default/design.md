# Design: lint-numeric-env-default

## Approach

Verification-first: the reported false positive was probed against v0.65.0
code before any design was committed. The probe matrix (unit-level
`RSchemaRule::analyze`, the same code path the CLI's R-SCHEMA rule runs)
covered: bare single-route / `{routes: [...]}` envelope / legacy-array
forms; quoted and unquoted tokens; single-token and dual-token delay bodies
(string and clean-integer `dynamic_header` defaults); choice / do-try
nesting; flow style; the shorthand `delay: ${env:X:-20000}` whole-body form;
and the `error_handler.retry` policy positions. Result: every
boot-accepts case is already lint-clean; float and empty defaults Error
exactly where boot rejects.

The change is therefore documentation truth + regression armor + one
bounded diagnostic-output fix, in three slices:

1. **Docs truth** (`env_interpolation.rs`): update the
   `WholeScalarEnvToken::WithDefault` variant doc and the module SYNC header
   so they state the two-stage behavior — the mirror forces STRING typing
   and collects candidates; `rschema.rs`'s `typed_carve_out_instance` then
   re-validates clean-integer/clean-boolean defaults as NUMBER/BOOL copies
   (Condition A: STRING-copy error chain covers the leaf via `span_covers`,
   leaf-anchored or ancestor-anchored; Condition B: the typed probe clears
   that chain). The stale text is what produced bd rc-jq775's root-cause
   claim; the module header's "this file has no clean-integer logic and
   needs no behavioral counterpart" line stays (it is still true) but gains
   a pointer to the rschema carve-out so the two files cannot be read
   apart.
2. **Regression pins** (`rules/rschema/tests.rs`): executable tests for the
   reported shape family — `delay_ms` shorthand-body and full-config forms,
   quoted and unquoted, bare and envelope documents; `retry.multiplier`
   with a clean-integer default at the NUMBER-typed position (the carve-out
   is not integer-position-specific on the probe side: the typed copy wins
   whenever the STRING copy's chain clears); `on_exceptions[0].retry`.
   Each pin asserts the exact absence of R-SCHEMA diagnostics for the
   leaf.
3. **Info-note suppression under Error** (`rules/rschema.rs`): in
   `analyze`'s Info-note loop, skip a substituted-default note when any
   Error-severity diagnostic already emitted for the validation has a span
   covering the note's leaf span (reuse `span_covers`). The note's text
   ("validated against substituted default") is only truthful when the
   leaf's chain is error-free. Float/empty defaults at numeric positions
   then report exactly one Error and no Info. Two bounded consequences,
   declared in the proposal and pinned in the delta: (a) one existing
   guard-arm pin (`${env:AS:-1}` at `auto_startup`, expecting Info beside
   the Error) migrates to zero Info; (b) collateral suppression — an
   ancestor-anchored Error span also covers individually valid sibling
   leaves under the same anyOf node (e.g. a valid `dynamic_header` beside
   an invalid float `delay_ms`), so their notes disappear with the
   offending leaf's. Suppression touches Info output only: Errors are
   never removed, downgraded, or relocated, and notes outside any Error
   span are unaffected.

Scenario fixtures follow the probe-verified shapes: unquoted env tokens
appear only in block mappings (an unquoted token inside a flow mapping is
an R-SYN parse error — the token's `}` terminates the flow mapping); flow
forms, where referenced, quote the token; every new pin asserts the
document parses before asserting the R-SCHEMA outcome, so silence can
never be vacuous. The retry-policy pins use valid routes
(`max_attempts` present — `RouteDslRedeliveryPolicy` has no serde default
for it).

The Error set itself does not change anywhere: boot-accepts cases stay
silent, boot-rejects cases stay Error.

## Affected crates

- `camel-lint`: `src/env_interpolation.rs` (doc comments only),
  `src/rules/rschema.rs` (Info-note suppression in `analyze`),
  `src/rules/rschema/tests.rs` (new pins).

## Architecture boundaries

No new dependencies; `camel-lint` stays runtime-free (no `camel-core` /
`camel-dsl` / `camel-cli` edges — the hexagonal test keeps guarding this).
The doc fix touches a SYNC-annotated file: the SYNC obligations are
behavioral (`interpolate_string` / `interpolate_env_with` /
`interpolated_validation_copy` mirroring `camel-dsl`), and comments
describing the lint-side typing mirror do not alter any SYNC'd behavior.
The lexical gates `clean_integer` / `clean_boolean` are untouched, so the
three-crate SYNC pin (camel-dsl `env_int_probe`, camel-config
`clean_i64`/`clean_bool`, camel-lint mirrors) is preserved byte-for-byte.

## Phases (optional)

Omitted — single-phase change (one coherent slice: docs + pins + one
suppression rule, all reviewed together against the same spec delta).

## Alternatives considered

- **Relax the typed carve-out condition (the rejected first attempt).**
  Ignoring ancestor-anchored errors in the carve decision over-carves when
  two whole-scalar candidates share an anyOf node (`delay_ms` plus
  `dynamic_header` under `DelayBody`): the probe's own new type error
  collapses to the shared ancestor, gets ignored, and the leaf is carved
  while a real string error hides. Step schemas use `anyOf` here, not
  `oneOf` — branch failures surface at the nearest non-matching node, so
  ancestor coverage via `span_covers` is load-bearing in BOTH conditions.
  Rejected: the bug it tried to fix does not exist on current main; the
  relaxation would only introduce false negatives.
- **Re-infer numbers in the typing mirror whenever the schema expects a
  number** (the bd's first suggested remedy). Rejected: it duplicates the
  carve-out pass with a schema-type lookup the mirror deliberately avoids
  (crate purity — no schema-aware walk in `env_interpolation.rs`), and the
  existing Condition A/B probe already implements exactly this with
  boot-parity semantics.
- **Downgrade the float/empty-default case to Advice** (the bd's second
  remedy). Rejected: boot rejects those inputs (the typed probe coerces
  only clean integers and exact-lowercase booleans; floats are not clean),
  so an Advice would under-report real load failures.
- **Extend boot (`camel-dsl` `env_int_probe`) to coerce clean floats at
  f64 positions**, which would make `${env:M:-2.0}` boot-accepted and
  change the parity target. Rejected for this change: cross-crate boot
  behavior change, unrelated to the reported false positive, and it would
  move the canon under the lint instead of fixing the report. Filed as a
  bd follow-up for the human to approve.
- **Suppress the Info note only for float defaults** (narrower than
  span-covering suppression). Rejected: the contradiction is not
  float-specific (empty defaults, `2x`, any Error-covered leaf); a
  shape-specific rule would need its own enumeration and drift.
