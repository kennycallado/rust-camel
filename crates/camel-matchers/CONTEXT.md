# Matcher Algebra

The shared assertion algebra of the rust-camel test tiers. This crate owns
the pure matcher types and the pure predicates over them: value
expectations (`Expectation`), recorded-request count bounds (`CountBound`),
path filters (`PathFilter`), the request expectation (`RequestExpectation`),
query-subset matching, and the judgment functions (`bound_holds`,
`settles_early`, `above_ceiling`, `expectation_matches`, `matching_count`,
`query_pairs`, `render_bound`, `stringify`). Both tiers call into these
types; grammar and observation stay per-tier (ADR-0072).

> **Scope boundary.** This file defines only the pure algebra. Document
> grammar (per-tier formats) stays with the tier kits: the scenario
> vocabulary with
> [`crates/camel-integration-test/CONTEXT.md`](../camel-integration-test/CONTEXT.md)
> (ADR-0069), the unit vocabulary with
> [`crates/camel-cli/CONTEXT.md`](../camel-cli/CONTEXT.md) (ADR-0064).
> Boot and bundle terms live in
> [`crates/camel-bundles/CONTEXT.md`](../camel-bundles/CONTEXT.md).

## Purity rule

The crate is types plus pure functions. It carries zero `camel-*`
dependencies. Allowed foundational third-party dependencies: `regex`,
`serde_json`, `form_urlencoded`. Nothing else. The crate has no harness
types, no wire types, no redaction, no async runtime, and no Cargo
features. ADR-0072 section 2 pins this as law.

## What this crate is NOT

- **No grammar.** No `.test.yaml` document model, no serde stage. The tier
  kits keep their raw serde stages and construct these types from their own
  document formats (ADR-0069 section 2 mixing ban stands).
- **No observation types.** No `HttpWireRequest`, no Exchange projections,
  no observation trait. `matching_count` receives already-recorded raw
  strings as projected `(method, path_and_query)` tuples; each tier projects
  its own observation at the call site (ADR-0072 section 3).
- **No redaction.** The ADR-0051 redaction law stays in the tier kits, in
  the diagnostics that render these types.
- **No async.** No tokio, no polling, no deadlines. Poll semantics and
  window ownership stay with the harnesses.

## Consumers

- **`camel-integration-test` (today).** The scenario tier consumes the
  crate: its grammar stage constructs the core types, and its validation
  path calls the judgment functions. The carve was a pure move; verdict
  output is unchanged.
- **`camel-test` (step 2, future).** The unit tier grows body and header
  matchers from this crate. That is a future change (ADR-0072 section 4).

## Language

**count bound**:
`CountBound` — the recorded-request count bound of a
`RequestExpectation`: `Exact`, `AtLeast`, `AtMost`, `Range`. Poll
semantics per bound: `AtLeast` succeeds early, `AtMost` is an absence
claim over the window.
_Avoid_: limit, threshold

**path filter**:
`PathFilter` — the path-and-query filter of a `RequestExpectation`:
`Exact` (strict bytes), `Contains` (substring), `Matches` (regex,
compile-verified at load time).
_Avoid_: path matcher (the filter is one of several matcher kinds)

**query subset**:
The query filter of a `RequestExpectation`: every declared pair must be
present in the recorded request's percent-decoded query, order- and
encoding-independent. `query_pairs` decodes `%XX` and `+` via
`form_urlencoded`.
_Avoid_: query equality (the subset relation is one-directional)

**value expectation**:
`Expectation` — the message-value matcher: `Equals`, `Regex`,
`Contains`, `StartsWith`, `EndsWith`, `Exists`, `JsonSubset`. The
grammar keys mirror the mock-testkit matcher rules.
_Avoid_: assertion (assertions are the tier kits' verdict machinery)

**grammar/observation split**:
The algebra is shared; the document formats (grammar) and the data that
gets matched (observation) stay per-tier. Never unify either; parameterize
the algebra instead (ADR-0072 section 3).
_Avoid_: shared grammar, shared observation trait