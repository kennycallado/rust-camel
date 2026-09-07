# Proposal: shared-matcher-core

## Why

The two test tiers speak different assertion vocabularies with the same intent. The
scenario tier (ADR-0069, wave D `812555cf`) landed a full matcher algebra —
`CountBound`, `PathFilter`, query-subset matching, `bound_holds`/`settles_early`/
`above_ceiling` — while the unit tier's `expects` vocabulary is endpoint→count
only (ADR-0064). Capability gravity inverted the test pyramid: pilot teams fled to
the scenario tier not because they wanted real wire, but because the unit tier was
mute. The e_opus consultation (ses_f880eca32ffeCbIky4WBYVB71w, bd rc-8zau7)
validated the direction: one SEMANTIC core shared by both tiers, with grammar and
observation staying per-tier — "same verbs, different subjects".

Extracting the core now, while wave D's code is fresh, prevents the algebra from
welding itself permanently into `camel-integration-test` harness concerns
(`HttpWireRequest`, `PartnerRouter`, redaction).

## What Changes

- **New zero-camel-dependency crate `crates/camel-matchers`** (placement decision
  weighed in design.md against the camel-config alternative; e_opus B2 precedent
  and ADR-0055 publish topology on record): the pure matcher algebra — bound types
  (`CountBound`), path filters (`PathFilter`), query-subset matching, pure judgment
  functions (`bound_holds`, `settles_early`, `above_ceiling`), and bound rendering.
- **`camel-integration-test` rewires to consume the crate** — a carve, not a copy:
  the pure types/functions move out; the welded parts (async polling,
  `PartnerRouter` observation, redaction diagnostics, scenario grammar
  deserialization) stay and adapt. No behavior change: all scenario-assertion tests
  stay green byte-for-byte in verdict output.
- **ADR-0072 "Test Pyramid v2"** (supersedes-in-part ADR-0069's vocabulary
  ownership): pins the placement decision, the purity rule (no harness/redaction/
  camel deps in the core; serde derives allowed), per-tier grammar and observation,
  and the staged direction e_opus approved (unit-tier `expects` growth and
  observational probes come as follow-up changes; mutating weaving stays gated by
  ADR-0064 §5; wire timeouts never virtualized per ADR-0069 §6).

**Excluded** (deliberately, to keep this a pure carve): growing unit-tier
`expects` (epic step 2), step-identity probes (step 3), any observation trait,
log-content assertions (rc-tdgh5), itest test-file extraction debt.

## Acceptance criteria

- `camel-matchers` exists with zero `camel-*` dependencies (regex, serde_json,
  form_urlencoded allowed; serde derives permitted); `lint-publish-cycles`
  passes; no crate that depends on it publishes before it.
- `camel-integration-test` contains no duplicate of the extracted algebra;
  `matching_requests`/validation paths consume core types; scenario test suite
  verdicts unchanged (existing tests green without textual expectation edits).
- ADR-0072 merged; CONTEXT-MAP.md + crate CONTEXT.md updated.
- All quality gates green.

## Risk budget

Acceptable: mechanical churn inside `camel-integration-test` internals. Out of
bounds: any change to scenario grammar (`.test.yaml`), verdict output, redaction
law (ADR-0051), or tier selection. The carve must be verifiable as a pure move:
`git diff` shows deletions from itest, additions to the crate, and import rewires.
