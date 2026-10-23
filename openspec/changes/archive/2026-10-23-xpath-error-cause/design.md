# Design: xpath-error-cause

## Approach

Three layers, mirroring the jsonpath precedent
(`EvalFailure { class: Conversion, detail: Some("body is not valid JSON") }`):

1. **Classify (camel-language-xpath).** `run_query` and `extract_xml` replace
   all four `EvalError` emissions with `EvalFailure`:

   | Path | Today | After |
   |---|---|---|
   | size over `max_input_bytes` | `EvalError("input exceeds maximum allowed size")` | `EvalFailure { class: Limit, detail: Some("input exceeds maximum allowed size") }` |
   | body not XML, coercion fails | `EvalError("body is not XML and cannot be coerced: {e}")` | `EvalFailure { class: Conversion, detail: Some("body is not XML and cannot be coerced") }` — the `{e}` interpolation is dropped (provider/body text risk) |
   | coercion produced non-XML | `EvalError("body coercion did not produce XML")` | `EvalFailure { class: Conversion, detail: Some("body coercion did not produce XML") }` |
   | sxd parse failure | `EvalError("xml parse error: body is not valid XML")` | `EvalFailure { class: Parse, detail: Some("body is not valid XML") }` |
   | sxd eval failure | `EvalError("xpath query failed: ...")` | `EvalFailure { class: Runtime, detail: Some("xpath query failed: expression could not be evaluated") }` |

   Taxonomy decision: no new `ExpressionErrorClass` variants. `Parse` is used
   for malformed XML bodies; the class doc in camel-api is extended from
   "expression source" to "expression or document source" (doc-only change).
   Coercion (body type conversion) is `Conversion`, matching jsonpath's
   classification of non-JSON bodies.

2. **Preserve cause (camel-language-api).** New
   `LanguageError::redacted_detail() -> Option<String>` returns `Some` only for
   payloads that are contractually exchange-data-free: `EvalFailure.detail`
   (redaction contract on the variant), `TypeMismatch` and `ConversionError`
   (type/destination names only per their docs). Free-form payloads
   (`EvalError`, `ParseError.reason`, registry strings) return `None`.
   `to_expression_failed` sets `cause: err.redacted_detail().map(|d|
   Box::new(CamelError::ProcessorErrorWithSource(d,
   Arc::new(ExpressionDetailCause))))`, where `ExpressionDetailCause` is a
   PRIVATE zero-size marker error type in camel-language-api. The carrier
   renders identically to `ProcessorError` (`Processor error: <detail>`), so
   no rendered message changes. Provenance is EXPLICIT, not shape-matched:
   new `pub fn is_expression_detail_cause(&CamelError) -> bool` (camel-
   language-api) downcasts the carrier's `#[source]` to the private marker;
   a caller-built `ProcessorError`/`ProcessorErrorWithSource` can never carry
   the marker. The `cause` field is `Option<Box<CamelError>>`, so a
   `CamelError` carrier is forced. Display of `ExpressionFailed` does not
   render `cause`. Additive public API: one predicate fn; no version bump,
   no new `CamelError` variant.

3. **Non-breaking (camel-processor).** `chain_predicate_error` currently
   chains the original route error only when `cause.is_none()`. Once
   `to_expression_failed` emits diagnostic causes, an onWhen predicate
   failure would keep the detail cause and drop the original route error from
   the chain. Fix: `chain_predicate_error` replaces the cause with the
   original route error when `cause.is_none()` OR
   `camel_language_api::is_expression_detail_cause(&predicate_err)`; any
   caller-supplied cause chain arriving through the public
   `PredicateSource::Async` closure API — including a hand-built
   `ProcessorError` or `ProcessorErrorWithSource` — passes through untouched
   (no marker). The previous behavior for genuine chains is preserved
   exactly.

   Diagnostic preservation decision: in the catch flow the diagnostic cause
   is replaced by the INTACT original `Box<CamelError>` — variant-matchable,
   Display-preserving, and required by existing tests
   (`do_try.rs:1069`, `expression_failure_propagation_test.rs:874` assert
   the cause renders the original text). Wrapping the original inside a
   source-bearing carrier (`ProcessorErrorWithSource(detail, original)`)
   was rejected: the top-of-chain render would become the detail string,
   breaking those exact-text contracts and demoting the original to an
   untyped `Arc<dyn Error>`. In the catch flow the class
   (`Limit`/`Parse`/`Conversion`/`Runtime`) still discriminates; the fixed
   cause content is carried by `to_expression_failed` in every non-catch
   flow (the camel-112 surface).

Test plan (evidence bar + bless fixes):

- camel-language-api unit tests: `to_expression_failed` populates cause for
  `EvalFailure{detail}`, `TypeMismatch`, `ConversionError` (exact payload,
  carrier render `Processor error: <detail>`); keeps `cause: None` for
  `EvalError` and `ParseError`; `Error::source()` traversal reaches the
  detail payload.
- camel-language-xpath unit tests: mapped assertions for all four classes
  (`Limit`/`Conversion`/`Parse`/`Runtime` via `to_expression_failed`), exact
  fixed cause payloads per path, secret-sentinel check (body text absent
  from cause and from `source()` traversal), and the ~100 KB discrimination:
  limit `L = 96 KiB`, well-formed body just above `L` → `limit`, malformed
  body just below `L` → `parse`.
- camel-processor unit tests: synthetic `ProcessorError` cause replaced by
  original; caller-supplied non-synthetic cause passes through untouched.
- camel-core integration test: doTry + xpath onWhen predicate failing on a
  malformed body — surfaced error carries the predicate's class and the
  original route error stays reachable in the `cause` chain; a
  Runtime-matching caller assertion proves the genuinely-runtime path is
  unchanged.

Non-breaking proof: no production code branches on
`ExpressionErrorClass::Runtime` (all `Runtime` matches outside camel-api are
test fixtures in choice/do_try/do_try_segment/loop_eip/filter/script_mutator/
compile.rs); the only cause-slot reader is `chain_predicate_error`. A
camel-core integration test proves a doTry catch with a failing xpath onWhen
predicate still surfaces the original error and the new `Parse` class.

## Affected crates

- camel-language-xpath: reclassify the four failure paths; rewrite
  `expression_rejects_oversized_input`; strengthen
  `predicate_rejects_oversized_input`; add class/cause unit tests and the
  ~100 KB size-vs-malformed discrimination test.
- camel-language-api: `redacted_detail()`, private `ExpressionDetailCause`
  marker, public `is_expression_detail_cause()`, cause population in
  `to_expression_failed`, unit tests (cause populated for typed payloads
  with exact carrier render; `cause: None` for `EvalError`/`ParseError`;
  provenance predicate true only for mapper-generated causes; `Error::
  source()` traversal reaches the detail payload).
- camel-api: doc-line extensions of `ExpressionErrorClass::Parse` (covers
  document sources) and `ExpressionErrorClass::Conversion` (covers
  input-body coercion, matching the jsonpath precedent).
- camel-processor: `chain_predicate_error` marker-based replace rule + unit
  tests (mapper-generated cause replaced by original; caller-supplied
  non-synthetic cause — including a hand-built `ProcessorError` — passes
  through untouched).
- camel-core: integration test (doTry + xpath predicate, original-error
  chain), stale comment update in `expression_failure_propagation_test.rs`.

## Architecture boundaries

Languages boundary: classification and redaction live in the language crates;
`camel-language-api` owns the shared error envelope; `camel-api` owns only the
class doc. `camel-processor` owns doTry chaining semantics. No Runtime/DSL/
component behavior changes. Sibling languages (jsonpath/minijinja cause drops)
stay untouched — tracked in bd rc-fukc6 (deferral: covered-by-open-bd).

## Deferrals

- jsonpath/minijinja/xpath limits-and-classes inconsistency beyond this
  mission's xpath scope: covered-by-open-bd rc-fukc6.
- Partner's 92–100 KB threshold root cause: non-goal per mission order
  (recorded-decision, bd rc-e22v9 description asks partner for route YAML +
  RUST_LOG).
