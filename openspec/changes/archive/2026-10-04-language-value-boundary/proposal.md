# Proposal: language-value-boundary

## Why

GH #62 (filed by the owner from the camel-cache demo, engine 0.56.0, reproduced
on 0.54.0) exposed two silent-loss defects at the language-to-engine boundary:

1. **Error swallow.** Runtime errors from language expressions are discarded by
   the camel-core glue (`await_eval`/`await_matches` in
   `crates/camel-core/src/lifecycle/adapters/step_resolution.rs`), which
   substitutes `null`/`false`. `set_property: {rhai: '"abc".parse_float()'}`
   stores `null` and the route continues; a failing `filter` predicate drops the
   Exchange. Sixteen step verbs are affected, for every Language.
2. **Structure loss.** The rhai outbound converters fall back to
   `Dynamic::to_string()`. A rhai `Map` stored by `set_property` reads back as
   the debug string `#{"a": 1}` in a later script.

The e_opus papal ruling (`.opencode/fleet/e-opus-rhai-papal-ruling-20261002.md`,
2026-10-02) defines the full contract: information crosses the boundary intact,
or the step fails with a loud, typed, redacted error. No third outcome. Owner
decisions are sealed in ruling section 8: no deprecation shims (pre-1.0 clean
break), no `ProcessorError` alias, strict boolean predicates, refuse
u64 > i64::MAX, all Languages at once, `Body::Stream` fails loudly. Vehicle:
minor release 0.57.0 with `BREAKING:` commit trailers.

## What Changes

This change adds the `language-value-boundary` capability (normative contract
E1-E7, V1-V5, B1-B5, P1-P2, O1-O3, language-agnostic; rhai is the first
implementation) and implements the P1 roadmap:

- **Phase 1 (bd rc-16aft, BREAKING):** fallible async expression glue in
  camel-core for all sixteen verbs; `CamelError::ExpressionFailed`
  (language, route, step, verb, `line:col`, error class, optional cause) with
  no compat alias; redaction of every value-bearing error class to
  class+position across all Languages, rhai and JavaScript audited first
  across all Languages per the mission-340 audit matrix — js is a LIVE leak
  today via `script:` (repro R5), simple/jsonpath/minijinja are latent
  (ADR-0051); strict boolean predicate results (non-bool is a type-mismatch
  error); compile-time rejection of `set_header()`/`set_property()` in
  read-only expressions, pointing authors to `script:` (sealed Q1: immediate,
  no deprecation release); language crates stop choosing failure log levels
  (ADR-0012).
- **Phase 2 (bd rc-33q5v + rc-7qv3r):** one recursive, fallible Dynamic-to-JSON
  converter (Map/Array native; NaN/Inf, FnPtr, timestamps, custom types, and
  JSON u64 > i64::MAX refused); `script:` integrity (body written back only
  when assigned, only changed headers/properties written back, native body
  exposure, `Body::Stream` refused).
- **Phase 3 (bd rc-qzd64):** GH #62 regression suite in camel-test with both
  YAML repros verbatim plus the in-script `try` statement form.

**Excluded** (contract present in the spec, implemented by follow-up changes per
ruling section 6): `has_header`/`has_property` and read-surface parity (V4, B5),
metrics, span attributes, and `camel test` error rendering (O1-O3). The P2
round-trip property tests and multi-language conformance kit also follow
separately.

Affected crates: `camel-api`, `camel-language-api`, `camel-core`,
`camel-processor`, `camel-language-rhai` (plus predicate-strictness audit in
`camel-language-{simple,js,jsonpath,xpath,minijinja}`), `camel-test`.

## Acceptance criteria

- A failing expression in any verb returns `Err(CamelError)` of
  variant `ExpressionFailed`, visible to `do_try`/`catch` and `on_exception`,
  with no partial Exchange mutation applied.
- `"SECRET".parse_float()` error output (CamelError, logs, DLC payload) contains
  no `SECRET`.
- GH #62 repro 1 (swallow) fails loudly; repro 2 (map) prints `type=map`.
- A mutating script that only sets one header leaves a JSON body bit-identical.
- Route addition fails when a read-only expression contains `set_property()`
  or `set_header()`, with an error pointing to `script:`.
- Non-boolean predicate results are type-mismatch errors, not coerced truth.

## Risk budget

BREAKING in 0.57.0 is the sanctioned risk: routes that relied on silent
`null`/`false`/truthiness now fail. Accepted. Out of bounds: any silent-loss
path remaining after this change, any new unredacted exchange data in errors
(ADR-0051 regression), and deprecation shims (sealed decision: none).
