# Design: expects-matcher-growth

## Approach

ADR-0072 step 2, corrected by recon to its honest shape. The unit tier is NOT
count-only — `camel-mock` already implements the full seven-key vocabulary
over `Body` (`matcher.rs`), and camel-cli's grammar deserializes into the
mock's public types. The work is therefore NOT "add matchers" but:

1. **One algebra, two projections** — mock's evaluation consumes
   `camel-matchers` where the algebra genuinely covers it, keeping its
   `Body`-typed public API as the observation layer (e_opus's law:
   parameterize the algebra, not the observation):
   - `Regex`/`Contains`/`StartsWith`/`EndsWith`: delegate to
     `expectation_matches` through a `text_only(Body) -> Option<Value>`
     projection (`Text(s) -> String(s)`, everything else `None -> false`) —
     preserves today's fail-closed-for-non-text semantics exactly.
   - `JsonSubset`: delegate through the existing `json_value(Body)` projection
     with the mock-side `pattern.is_object()` guard kept BEFORE delegation
     (core treats scalar patterns as equality; mock semantics say non-object
     patterns always fail — the guard preserves bytes, no core API change).
   - `Equals` (variant-tagged `body_eq` over Text/Json/Binary/Empty) and
     `Exists` (non-`Empty`) STAY mock-side: they are observation-typed
     equality; a Value projection would collapse the Text/Json variant
     distinction `body_eq` preserves. The local `json_subset` duplicate is
     DELETED (core provides it via `expectation_matches`).
   - `mismatch_note`/`Display` diagnostics stay mock-side (harness detail).
2. **Bounds completion** — `MockEndpointInner` gains
   `expect_bound(&CountBound)` (with `expect_maximum_count(n)` alongside the
   existing `expect_count`/`expect_minimum_count` sugar). Unit-tier absence
   semantics are simpler than the scenario tier's: the runner settles traffic
   BEFORE asserting, so every bound decides on the single post-settle
   snapshot — `AtMost(n)` is the absence claim, no polling. Error text
   follows the house style ("expected at most N exchanges, got M").
3. **Grammar addition** — `ExpectSet` gains `maxCount` (backward compatible):
   `count` stays exclusive with `minCount` AND `maxCount`;
   `minCount`+`maxCount` together = `Range`; `minCount > maxCount` is a
   document error (exit 2) in the same exclusivity family — a silently
   unsatisfiable range would fail confusingly at runtime. The existing
   "mutually exclusive" doc-error family extends.
4. **camel-test kit** — re-exports `camel_matchers::{Expectation, CountBound}`
   so programmatic users see one matcher type across tiers. No grammar, no
   observation types (purity rule).
5. **ADR-0072 amendment** — Context corrected to the true baseline (rich
   ad-hoc grammar in mock, already mirrored by the scenario tier; real gaps
   were duplication and missing upper bounds) via a DATED amendment note
   (repo precedent: ADR-0050 carries one — never silently
   rewrite a landed ADR). Decision sections unchanged — the direction was
   right, the motivation overstated. The projection pair (`text_only`,
   `json_value`) becomes the ADR's worked example of per-tier observation.
6. **expectReply** — unchanged: it already evaluates through the mock's
   public matcher API (runner.rs: "never a CLI-private comparison"). Correct
   layering predates us.

## Affected crates

- **camel-component-mock** (modified): matcher.rs delegates string/json verbs
  to the core (projections above), deletes `json_subset`; gains
  `expect_bound`/`expect_maximum_count`; gains dependency `camel-matchers`.
- **camel-cli** (modified): test-command grammar `maxCount` +
  exclusivity rules; maps fields to the mock API (same pattern the spec
  pins); no matcher logic of its own (already true).
- **camel-test** (modified): re-exports.
- **camel-matchers**: UNTOUCHED (purity; no API change — delegation uses
  existing `expectation_matches` + types).

## Architecture boundaries

Runtime (camel-core/processor) untouched. camel-mock stays a dual-use runtime
component (ADR-0064 lean set; production routes may consume its assertions)
— it gains a dependency on the pure leaf crate that publishes first
(ADR-0055). Per-tier grammar holds: bodies strict grammar / headers dual
grammar deserialize into mock-typed assertions exactly as today; the scenario
tier's grammar targets the core directly. `lint-component-deps` must accept
the new edge (verify; components depending on a pure top-level lib is the
ADR-0072-sanctioned shape).

## Phases

Single phase — one coherent slice: mock delegation + bounds + grammar +
kit re-exports + ADR amendment.
