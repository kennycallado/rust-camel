# Proposal: expects-matcher-growth

## Why

ADR-0072 step 2: one assertion algebra, two tiers. Recon of the landed state
corrects the motivating story — and sharpens the work:

1. **The unit tier is NOT count-only.** `camel-cli`'s test command already
   parses `expects.bodies` (strict grammar) and `expects.headers` (dual
   grammar) with the matcher keys `equals, regex, contains, startsWith,
   endsWith, exists, jsonSubset` — the IDENTICAL key set the scenario tier
   uses, because wave D mirrored the mock-testkit rules when building its
   grammar (now carved into `camel-matchers`). ADR-0072's Context overstates
   the gap ("endpoint-to-count only") and must be amended to the honest
   baseline.
2. **The real defect is duplication + incompleteness.** Two parallel
   implementations of the same seven-key vocabulary: `camel-component-mock`'s
   `BodyMatcher`/`HeaderMatcher` (with their own `matches()` evaluation — a
   dual-use RUNTIME component whose assertions production routes also consume)
   and the carved `camel-matchers` core. This invites semantic drift — the
   exact bug class this program exists to kill. And the unit tier's bounds are
   `count`|`minCount` (mutually exclusive, no upper bounds, no absence
   claims), while the scenario tier has the full `CountBound`
   (Exact/AtLeast/AtMost/Range with settle/absence semantics).
3. `expectReply` assertions ride yet another private matcher set in the same
   module.

## What Changes

- **camel-component-mock consumes `camel-matchers` where the algebra
  covers it**: string verbs (`regex`/`contains`/`startsWith`/`endsWith`) and
  `jsonSubset` delegate to `camel_matchers::expectation_matches` through
  per-tier projections of the received `Body` (`text_only` for string verbs,
  non-text fails closed; `json_value` for `jsonSubset` with the
  object-pattern guard kept mock-side); variant-tagged `equals` (`body_eq`)
  and `exists` remain observation-typed evaluations in the mock; the local
  `json_subset` duplicate is deleted. The public assertion API
  (`BodyMatcher`/`HeaderMatcher` types, `expect_*` setters,
  `try_assert_satisfied`) and all diagnostics stay byte-identical.
- **camel-cli test command**: grammar unchanged in surface — it already
  deserializes into the mock's public types (correct layering); `ExpectReply`
  stays as is (it already evaluates through the mock's public matcher API).
- **Bounds completion**: `ExpectSet` gains the missing upper-bound forms via
  `camel_matchers::CountBound` (grammar addition, backward compatible:
  `count`/`minCount` keep working; new `maxCount`, `minCount > maxCount`
  rejected at parse, and `minCount`+`maxCount` = inclusive range; unit-tier
  absence = `maxCount: 0` decided on the post-settle snapshot — no polling).
- **ADR-0072 amendment**: Context corrected to the true baseline (rich ad-hoc
  grammar duplicated across mock/scenario implementations; bounds
  incomplete); direction unchanged.
- **camel-test kit**: minimal — re-export alignment so programmatic users
  see one matcher type; no grammar, no observation types (purity rule holds).

**Excluded**: observational probes/step identity (ADR-0072 step 3), any
scenario-tier/itest change (wave E, other agent), grammar unification beyond
the shared deserialization target, virtual time (ADR-0069 §6).

## Acceptance criteria

- Zero duplicate matcher algebra where sharing is sound: the local
  `json_subset` is deleted from camel-mock; string-verb and `jsonSubset`
  verdicts route through `camel_matchers::expectation_matches`; the mock's
  public types and diagnostics are byte-identical (observation-typed
  `equals`/`exists` evaluation stays).
- Existing unit-tier test corpus parses and passes byte-identically (no
  `.test.yaml` edits).
- New bounds forms (`maxCount`, range, absence) work with post-settle
  semantics; own tests; `minCount > maxCount` is a parse error (exit 2).
- `camel-matchers` untouched (no new deps, no API change).
- ADR-0072 amended with a dated amendment note; all gates green.

## Risk budget

Acceptable: churn inside camel-mock matcher internals and camel-cli
test-command internals; the new mock → camel-matchers dependency edge.
Out of bounds: breaking `.test.yaml` compatibility, changing any public
matcher type, signature, or diagnostic byte, touching itest (wave E owns
it), changing `camel-matchers`, virtual time, tier selection.
