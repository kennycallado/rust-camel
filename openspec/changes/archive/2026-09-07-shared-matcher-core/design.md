# Design: shared-matcher-core

## Approach

A **pure-crate carve**: the matcher algebra wave D landed inside
`camel-integration-test` moves to a new zero-camel-dependency crate
`crates/camel-matchers`; itest rewires to consume it. No behavior change — the
carve must be readable from the diff as deletion-from-itest +
addition-to-crate + import rewires. This is epic rc-8zau7 step 1 (e_opus
consultation ses_f880eca32ffeCbIky4WBYVB71w: "extract while D is hot"; pure-crate
carve, no features bundled).

**What moves** (all plain Rust, already serde-free — document.rs keeps a raw
serde stage that constructs these):
- `CountBound` + judgment fns `bound_holds` / `settles_early` / `above_ceiling`
  (document.rs:321, partner_validate.rs:98-134)
- `PathFilter` (document.rs:336)
- `PartnerExpectation` (document.rs:349) — renamed **`RequestExpectation`** in
  core (neutral subject for the unit tier; itest re-exports
  `pub use camel_matchers::RequestExpectation as PartnerExpectation` — `pub`,
  not `pub(crate)`, alongside `pub use` re-exports of the other moved types,
  because itest's lib.rs publicly re-exports `CountBound`, `Expectation`,
  `PartnerExpectation`, `PathFilter` today and the staying `ValidateExpectation`
  carries them in pub variants)
- `Expectation` (value matcher, document.rs:284) + the pure per-form evaluator
  `expectation_matches(&Expectation, &Value) -> bool` and its helper definitions
  `stringify` (runner.rs:780) and `json_subset` (runner.rs:790-801, including the
  :796 recursion inside its body). The harness match arms at runner.rs:138 and
  :624-679 — which construct `ScenarioFailure` details with subject, redaction,
  and humantime rendering — STAY in itest and delegate the boolean decision to
  the core evaluator, keeping detail rendering and the invalid-regex
  `ValidationMismatch` arm (runner.rs:635-640) itest-side so verdict bytes do
  not change. Moved types swap `camel_api::Value` → `serde_json::Value`
  (camel-api/src/value.rs:2 is a plain alias; zero semantic change).
- `query_pairs` (percent-decoding query parser, partner_validate.rs:85)
- `matching_requests` → core **`matching_count`**, parameterized the algebra,
  not the observation: input is an iterator of `(method: &str,
  path_and_query: &str)` tuples — no `HttpWireRequest`, no trait. itest maps its
  wire records to tuples at the call site.
- `render_bound` (pure bound rendering, partner_validate.rs:303)

**What stays in itest**: the raw serde stage + grammar parsing
(`expectation_from_value`, `partner_expectation_from_value` construct core
types), async polling (`partner_validate_action`), `PartnerRouter` /
`HttpWireRequest` observation, redaction-coupled diagnostics
(`partner_mismatch_detail`, `render_filters` — ADR-0051 law), `deadline` /
`elapsed_at_least` semantics (window ownership), tier selection.

**Crate dependencies**: `regex` (PathFilter::Matches, Expectation::Regex),
`serde_json` (Expectation over Value), `form_urlencoded` (query decoding).
Zero `camel-*` deps — the purity rule is "no camel/harness/redaction deps",
foundational third-party allowed. No async runtime, no tokio. Because the
moved enums are `#[non_exhaustive]`, the stay-side exhaustive matches
(runner.rs:624-679, partner_validate.rs:327-337) gain wildcard arms with a
defined defensive error — no verdict-byte change. (Acknowledged:
after the carve, non-http itest builds gain `serde_json`/`form_urlencoded`
transitively where today they are http-gated optionals — not an ADR-0069 §8
concern, that gate covers the hyper stack.)

**Placement decision (open question rc-8zau7, settled here)**: new crate over
camel-config. The B2 precedent (rc-6bsf: "camel-config is the natural home")
applied where a natural home already existed for BOOT sharing; here the natural
home IS the vocabulary itself, and stuffing matchers into a config crate is the
accretion that makes a 60-crate workspace feel disorganized. ADR-0055 topology:
zero camel deps ⇒ the crate publishes first, no cycle risk,
`lint-publish-cycles` passes trivially. Rejected: camel-core/camel-api (runtime
pollution with test vocabulary), camel-test (itest would depend on the unit kit
— inverted direction). Cost accepted: one more crate, justified by being one
named concept.

## Affected crates

- **camel-matchers (NEW)**: the algebra — bounds, filters, value matchers,
  generic request-matching over projected tuples, rendering. Own unit tests for
  every judgment fn (currently tested only transitively via itest scenarios).
- **camel-integration-test**: delete the moved items; import from
  `camel_matchers`; add the `as PartnerExpectation` alias; adapt
  `matching_requests` to project `(method, path)` tuples into
  `matching_count`; grammar raw-stage now constructs core types. All existing
  tests stay green without expectation-text edits.
- **workspace root**: member registration + publish order position.

## Architecture boundaries

Runtime (camel-core/processor) untouched. The crate sits in the foundational
band BELOW camel-api alongside pure libs — consumable by camel-test (unit tier,
step 2), camel-integration-test (scenario tier), and camel-cli without dragging
harness or wire concerns. Grammar and observation remain per-tier (ADR-0069 §2
mixing ban untouched; ADR-0072 pins this as law). Redaction never crosses into
core. Wire fidelity untouched — `matching_count` receives already-recorded raw
strings.

## ADR-0072 "Test Pyramid v2" (supersedes-in-part ADR-0069)

Pins: placement + purity rule; one algebra / per-tier grammar / per-tier
observation ("same verbs, different subjects"); staged direction — step 2
unit-tier `expects` growth and step 3 observational probes are future changes;
mutating weaving stays gated by ADR-0064 §5; wire timeouts never virtualized
(ADR-0069 §6 ceiling); grammar never unified; `recipient_list` force-FULL stays
static. Includes the **testing-surface map** (today spread across
ADR-0064/0069/0055/0070 + camel-cli + dual-use lean components) — closing the
documentation gap raised in the placement discussion.

Single phase — one coherent carve; no milestone grouping needed.
