# Architectural Ruling — rc-90ez: Splitter fail-loud semantics

Status: BINDING. Gates the `splitter-fail-loud` OpenSpec change. Author: senior architect escalation.
Scope: research/consultation only — no code was modified producing this ruling.

## Summary verdict

Bless **Option A (typed error), no lenient opt-in mode, no fallback.** Change the
`SplitExpression` signature in place to a fallible return (pre-1.0 break). Extend the same
treatment to the streaming path and to the `DeclarativeSplit` language-expression path, which
today already silently applies an inconsistent Option-B behaviour. Split the aggregate DSL
split-brain into its own bd issue.

---

## Q1 — Semantics: Option A, Option B, or hybrid?

**Verdict: Option A (typed error). No lenient mode.**

Rationale: the repo's own error philosophy already resolves this. `UnmarshalService` on an
unparseable body returns `CamelError::TypeConversionFailed` (verified `marshal.rs:150`), and the
RecipientList EIP was deliberately hardened to a zero-success `Err(last_error)` contract "never
`Ok(original)`" (camel-processor CONTEXT.md, ADR-0058). A silent success no-op on a
type-misconfiguration is the exact anti-pattern ADR-0058 removed for RecipientList; Splitter must
match. Apache Camel parity also favours A — `split(jsonpath(...))` on a non-JSON body fails at
expression evaluation, it does not silently emit zero fragments. Option B is rejected because for
`split_body_json_array` on `Body::Text("[1,2,3]")` (the exact forgot-unmarshal case) it would fire
the aggregator once with the raw string — trading a silent timeout for silent mis-aggregation,
which directly contradicts the issue's fail-loud intent. A hybrid lenient flag is rejected on
YAGNI + surface-cost grounds: it adds a `SplitterConfig` knob and a second semantic contract to
test/document for a use case nobody has asked for; the genuinely-empty case (Q2) already covers the
only legitimate "don't fail" scenario. If a real lenient demand appears later it is purely
additive (a config flag or a distinct `split_body_lines_lenient()` constructor) and can be added
without re-breaking.

## Q2 — Empty-content case: does pass-through stay?

**Verdict: Pass-through stays for all three genuinely-empty-but-correct-type cases. Unchanged.**

`Body::Empty`, `Body::Text("")` under BodyLines (`"".lines()` yields 0 lines), and
`Body::Json([])` under BodyJsonArray (empty array iterates to 0 fragments) are all **correct type,
genuinely empty content** — case (a), not the bug. These already flow through the correct-type arm
and produce `Vec::new()` legitimately; the processor-level `if fragments.is_empty()` pass-through
(splitter.rs:103, split_segment.rs:139) stays and remains Camel-compatible ("null/empty body →
original exchange continues"). The fix must fire **only** when the body TYPE does not match the
expression's expected type — i.e. only the `_ =>` arms at api/splitter.rs:319 and :334 change from
`return Vec::new()` to `return Err(...)`. The existing `test_split_empty_fragments`
(camel-processor/src/splitter.rs:491) and `test_split_body_lines_empty` (api:382) stay green and
must be preserved as regression pins.

## Q3 — Implementation shape: signature change vs new fallible type?

**Verdict: Change `SplitExpression` in place. Pre-1.0 break is correct; a parallel
`TrySplitExpression` doubles the surface permanently.**

Mandated target signature (camel-api/src/splitter.rs:12):

```rust
pub type SplitExpression =
    Arc<dyn Fn(&Exchange) -> Result<Vec<Exchange>, CamelError> + Send + Sync>;
```

Rationale: introducing `TrySplitExpression` with the old alias deprecated leaves two constructors,
two call sites in every consumer, and a permanent "which one do I use" question — the CONTEXT.md
already carries several `legacy-pending-removal` shells and the project is explicitly pre-1.0
(v1.0 freeze table is aspirational, not shipped). A one-time in-place break is cheaper than
carrying a deprecated twin forever. The blast radius is bounded and mechanical because nearly every
call site is a builder that wraps a **known-good** expression and can `Ok(...)`-wrap trivially:

- `split_body_lines`/`split_body_json_array`/`split_body` (api): the two matchers return
  `Err(...)` on the `_` arm, `Ok(vec)` otherwise.
- Every `SplitterConfig::new(split_body_lines())` call site (camel-builder ~20 sites, camel-core
  route_definition/commands/step_resolution/startup_validation, benches) is unaffected at the call
  site — they pass the constructor result through; only the constructor body changes.
- Two consumption points dereference the expression and must propagate the `Result`:
  `SplitterService::call` (splitter.rs:100) and `SplitSegment::run` (split_segment.rs:137). Both
  already return `Result`/`PipelineOutcome`, so propagation is a `?` / `Failed(err)` map.
- The `DeclarativeSplit` compiler closure (splitting.rs:126-149) and any custom user
  `split_body` closure become `Result`-returning.

Also add a fallible builder method mirroring the other config validators; the expression is
evaluated per-exchange, so there is no build-time validation to add — the error surfaces in `call`.

## Q4 — Streaming splitter: same treatment?

**Verdict: The streaming path is ALREADY fail-loud and needs NO signature change — but the
`StreamingSplitExpression` behaviour must be pinned by a regression test, and one gap closed.**

Verified: `StreamingSplitExpression = Arc<dyn Fn(Exchange) -> Stream<Item = Result<Exchange,
CamelError>>>` (api/splitter.rs:20) — it is **already fallible per item**. The production ndjson
expression returns `Err(CamelError::ProcessorError("streaming split requires Body::Stream"))` on a
non-`Body::Stream` body (streaming_splitter test-mirror of step_resolution logic; confirmed the
`_ =>` arm yields an error item). So a forgot-to-provide-a-stream misconfiguration on the streaming
path already fails loudly, not silently. Two required follow-ups inside this change:
1. Upgrade that message to the mandated diagnostic format (Q5) and, ideally, promote it from
   `ProcessorError` to `TypeConversionFailed` for classification parity with the eager path.
2. Empty-stream pass-through (`test_ndjson_body_stream_empty_stream` →
   `Body::Json([])` under CollectAll) is correct and stays — it is case (a) for streams.

The `StreamingSplitExpression` signature is therefore **not** changed; only the eager
`SplitExpression` is.

## Q5 — Diagnostic quality: error format + should there be a warn log?

**Verdict: Typed error is sufficient. NO warn log — a typed `Err` that fails the exchange is the
diagnostic; a duplicate warn would be double-reporting and would violate the ADR-0012 log-policy
(the route ErrorHandler owns ERROR/WARN responsibility, not the EIP).**

Mandated message format (models on the existing `TypeConversionFailed(String)` Display,
`error.rs:93`). Use `CamelError::TypeConversionFailed` so it classifies with the unmarshal family:

For `split_body_lines`:
```
split(body-lines) expects a text body but received Body::Json; \
add an unmarshal step before split to convert the body to text
```

For `split_body_json_array`:
```
split(body-json-array) expects a JSON array body but received Body::Text; \
add an unmarshal step before split to convert the body to a JSON array
```

Format contract the spec must enforce (test-asserted substrings): (1) names the split expression
kind, (2) names the **received** body variant, (3) names the **expected** type, (4) carries the
remediation hint `add an unmarshal step before split`. The received-variant name must come from a
small `body_type_name(&Body) -> &'static str` helper (or inline `match`) — do NOT `Debug`-print the
body (leaks payload; violates the redacting posture in camel-api CONTEXT.md). Name the variant only
(`Body::Json`, `Body::Text`, `Body::Bytes`, `Body::Xml`, `Body::Stream`, `Body::Empty`).

## Q6 — Aggregate DSL split-brain: child issue or fold in?

**Verdict: Split into a child bd issue. Do NOT fold into rc-90ez.**

Verified the split-brain is real and orthogonal: `compile_aggregate_step` (compile.rs:1806-1811)
intentionally does not wire `def.correlation_key` on the builder path ("the builder path lacks
`correlate_by_expr()`"), while the declarative validator (compile.rs:2071-2075) rejects a missing
`correlation_key` as required. This is an Aggregator-EIP compiler concern with zero code overlap
with the Splitter expression signature; folding it in would blur the `splitter-fail-loud` change's
blast radius and its test surface. File as `discovered-from:rc-90ez`, priority P2, type bug,
scoped to the builder-vs-canonical aggregate correlation-key lowering.

## Q7 — Scope guard: blast-radius risks the spec/design phase must cover

Flag the following explicitly in the spec so no consumer is missed at compile time:

1. **Pre-existing Option-B inconsistency (MUST FIX in this change).** The `DeclarativeSplit`
   language-expression compiler closure (splitting.rs:147) already does
   `_ => vec![exchange.clone()]` — a silent single-fragment fallback (Option B) for a
   non-string/non-array expression result. This directly contradicts the Option-A ruling and is a
   second silent-mis-route path. The spec MUST change this arm to return the same typed error so
   all eager split paths share one contract. This is the single most important finding — without
   it the fix is half-done and the two split paths disagree.
2. **Benchmark closures.** `benches/splitter.rs` and any bench-local `SplitExpression` closures must
   be `Ok(...)`-wrapped; the grep shows bench usage. A failing bench compile is the most likely
   "green in crate, red in workspace" surprise.
3. **`split_segment.rs` internal uses.** `SplitSegment::run` (:137) dereferences the expression;
   propagate the `Err` as `PipelineOutcome::Failed(err)` (NOT `Stopped` — this is a failure, not
   successful control flow, per ADR-0024/0025). The `if fragments.is_empty()` arm (:139) stays.
4. **`startup_validation.rs` (camel-core:530).** Constructs a `SplitterConfig` with
   `split_body_lines()`; confirm it still compiles and that no startup-time validation newly needs
   to run the expression (it does not — the error is per-exchange).
5. **`commands.rs` / `route_definition.rs` / `step_resolution.rs`.** All construct via the
   constructors; they are call-site-stable but must be listed as touched for the reviewer.
6. **Regression pins that must stay green:** `test_split_empty_fragments`,
   `test_split_body_lines_empty`, `test_split_body_json_array_not_array` (api:401 — NOTE: this test
   currently asserts `fragments.is_empty()` on a non-array JSON body; under the new contract the
   constructor returns `Err`, so **this test must be rewritten** to assert the typed error, not
   deleted). Flag it explicitly — it is the one existing test whose semantics invert.
7. **CONTEXT.md updates.** The "Aggregation contract (divergence from Apache Camel)" block and the
   `SplitExpression` glossary entry in camel-api CONTEXT.md must be updated to state the new
   fallible contract and the fail-loud-on-type-mismatch semantics.

## Additional findings the implementation must know

- **`test_split_body_json_array_not_array` inverts** (see Q7.6) — the only behavioural test that
  changes meaning rather than staying green. Call it out in tasks.md so it is not treated as a
  regression.
- **Two aggregation code paths, one contract.** `aggregate` (Tower, splitter.rs:252) and
  `aggregate_completed` (segment, split_segment.rs:26) are independent; the empty pass-through and
  the new error both sit *before* aggregation, so neither aggregator changes. Good — keeps blast
  radius off the aggregation code.
- **`SplitExpression` is re-exported** from `camel_api::splitter` and at crate root
  (`camel_api::SplitExpression`); both are used across the workspace. The type change is source-
  compatible for importers (same path), only the closure return type changes.
- **Classification choice matters.** Using `TypeConversionFailed` (not `ProcessorError`) means the
  error classifies with the unmarshal family in metrics/tracing (`error.rs:93`, `classify`), which
  is the semantically honest bucket for a "wrong body type, needs unmarshal" failure.
