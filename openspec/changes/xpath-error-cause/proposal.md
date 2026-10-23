# Proposal: xpath-error-cause

## Why

camel-112 cannot diagnose xpath failures on bodies around 100 KB: every failure
path (size limit, body coercion, malformed XML, evaluation error) collapses to
`LanguageError::EvalError`, which maps to class `Runtime`, and
`to_expression_failed` drops the message entirely (`cause: None`,
`camel-language-api/src/eval.rs:93`). The operator-visible surface is
"expression failed: runtime in xpath" — a size/limit problem is
indistinguishable from malformed XML without `RUST_LOG` (bd rc-e22v9).

## What Changes

- camel-language-xpath reclassifies its four failure paths onto the existing
  typed `EvalFailure` variant: input size → `Limit`, body coercion →
  `Conversion` (dropping the interpolated provider error text), malformed XML
  body → `Parse`, genuine evaluation failure → `Runtime` (unchanged class).
  No new `ExpressionErrorClass` variants; the `Parse` class doc in camel-api
  is extended to cover document sources.
- camel-language-api adds `LanguageError::redacted_detail()` and
  `to_expression_failed` populates `cause` with a fixed, redacted detail
  wrapped in `CamelError::ProcessorError` instead of `None`. Free-form
  payloads (`EvalError`, `ParseError` reason) keep `cause: None` — they are
  not guaranteed free of provider or exchange text.
- camel-processor `chain_predicate_error` replaces a synthetic detail cause
  with the original route error so doTry/onWhen chaining keeps working
  (non-breaking requirement).
- Existing xpath tests that assert `EvalError` on the size path are rewritten
  for the new classification; a ~100 KB regression test distinguishes a size
  failure from malformed XML by class and by cause content.

Out of scope: engine-specific sibling work (jsonpath `EvalError` size path,
minijinja timeout class — tracked in bd rc-fukc6), reproducing the partner's
exact 92–100 KB threshold, XPath evaluation limits, version bumps. Note: the
shared `to_expression_failed` fix mechanically propagates typed cause
payloads across ALL languages, not just xpath: every producer of
`EvalFailure.detail`, `TypeMismatch`, or `ConversionError` (jsonpath's
`body is not valid JSON`, JS read-only refusals, minijinja and rhai typed
failures) now reaches `cause`. All current producers emit static strings or
type names (verified by the stage-4 expert producer grep); the
engine-specific classification gaps remain the rc-fukc6 deferral. Sibling
classes are unchanged.

## Acceptance criteria

- Size-limit failure carries class `Limit`; coercion carries `Conversion`;
  malformed XML carries `Parse`; genuine evaluation failure keeps `Runtime`.
- `to_expression_failed` carries a fixed, redacted detail in `cause` for every
  reclassified xpath path; the raw body never appears in any error surface.
- Non-breaking: callers matching `Runtime` keep working for genuinely-runtime
  paths; doTry catch-predicate chaining still surfaces the original error.
- A ~100 KB regression test tells a size/limit failure apart from malformed
  XML by class and cause, without `RUST_LOG`.

## Risk budget

- Class drift for non-xpath languages: bounded — only xpath paths change
  class; other languages keep their current classes.
- doTry chain semantics: the only production writer of the `cause` slot is
  `chain_predicate_error`; the replace rule is tested to preserve the original
  error.
- Redacted-cause leakage: cause content is limited to static strings already
  present in today's messages, plus type-name-only strings.

Affected crates: camel-language-xpath, camel-language-api, camel-api,
camel-processor, camel-core (tests only). Bd: rc-e22v9.
