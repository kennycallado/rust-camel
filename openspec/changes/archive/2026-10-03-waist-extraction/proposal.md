# Proposal: waist-extraction

## Why

Three assertion families now live in `camel-integration-test`: the partner
(traffic) family and two state families, SQL and Surreal. The rule of three
is satisfied, and with it the duplication tax:

- Datasource resolution (name -> config `db_url` -> pool handle, with
  ADR-0051 redaction) is copied verbatim across four sites:
  `sql_action.rs`, `surreal_action.rs`, `runner/sql_validate.rs`,
  `runner/surreal_validate.rs`. The surreal sites reach through
  `crate::sql_action::sanitize_db_error` — a cross-family smell.
- The deadline poll lattice is copied across three sites
  (`runner/partner_validate.rs`, `runner/sql_validate.rs`,
  `runner/surreal_validate.rs`). The SQL and Surreal loops are identical;
  the partner loop is the monotone early-settle variant of the same shape.
- The matcher algebra already has one home (`camel-matchers`, ADR-0072),
  but ADR-0069 does not record the adapter taxonomy that now exists:
  traffic adapters vs state adapters, and the activation-per-need rule
  the second state family just exercised.

bd: rc-25lup.6 (epic child 6/6, the last). Epic rc-25lup.

## What Changes

- Extract a datasource steering seam: one resolver, generic over the pool
  handle type, carrying the identifier law (errors name the datasource,
  never the URL) and the redaction law (every driver error sanitized
  against `db_url`). All four sites call it.
- Extract a deadline poll driver: one async `poll_until` with a
  fallible snapshot and two judgments (an early judgment that may stop
  the poll, and a final judgment that decides at expiry). Partner passes
  the monotone variant; SQL and Surreal pass the ceiling-breach variant.
- Amend ADR-0069 with a section 14: the traffic/state adapter taxonomy,
  the waist map (steering axis, poll driver, matcher algebra home), and
  the activation-per-need rule. The amendment cites ADR-0072 as the
  matcher algebra home without deciding its status.
- Record the new terms in `crates/camel-integration-test/CONTEXT.md`.

Explicitly excluded: any generic `state:` verb, any shared state-family
trait, any change to per-family query vocabulary, projection, action
names, poll intervals, or error text. This is a behavior-preserving
refactor; every error string stays byte-identical.

## Acceptance criteria

- All four datasource resolution sites call one resolver (structural);
  the surreal family no longer imports from `sql_action`, and the
  `sql_action::sanitize_db_error` public path survives as a re-export
  of the steering seam.
- All three validate sites use one poll driver; no family vocabulary
  leaks into it.
- Error-string regression: exact-string tests cover every label
  (`sql action`, `sql validation`, `surreal action`, `surreal
  validation`) against both resolution failures (unknown name, pool
  error) — equality checks, not substring checks.
- Feature-off and single-feature builds stay warning-clean under
  `-D warnings`: featureless, `http`-only, `sql`-only, `surreal`-only,
  and the combined build.
- ADR-0069 gains section 14 (qualifying section 5 as traffic-family
  law); CONTEXT.md gains the taxonomy terms.
- Batteries green at baseline: redis lib 584 / battery 608,
  feature_profiles 18, ws-lib, sql and surreal itest batteries, and the
  full `camel-integration-test` suite.

## Risk budget

Zero behavior change is the hard bound. Error strings byte-identical,
poll timing order unchanged (snapshot -> early judgment -> deadline
check -> sleep). Out of bounds: new grammar, new public API beyond the
crate root re-export, any touch on `camel-matchers` purity (ADR-0072),
any move of the poll driver into the pure matcher crate.
