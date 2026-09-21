# Design: errorkinds

## Approach

Mirror the 24ba1ee9 (rc-2vm2y) registration pattern in
`crates/camel-dsl/src/compile.rs`, three times:

1. Append `"UnsupportedMediaType"`, `"NotAcceptable"`,
   `"ProcessorErrorWithSource"` to `supported_exception_kinds()`.
2. Add structural match arms in `exception_kind_matches()`:
   - `"UnsupportedMediaType" => matches!(err, CamelError::UnsupportedMediaType { .. })`
   - `"NotAcceptable" => matches!(err, CamelError::NotAcceptable { .. })`
   - `"ProcessorErrorWithSource" => matches!(err, CamelError::ProcessorErrorWithSource(..))`
   Each arm carries a rationale comment citing rc-5u8co and the raise site
   (media negotiation gate / source-carrying producers).
3. Keep `ProcessorError` matching ONLY `CamelError::ProcessorError(_)`.
   `variant_name()` aliases `ProcessorErrorWithSource` (and
   `AuthProviderUnavailable`) to `"ProcessorError"` for doTry
   catch-by-variant compat; `on_exceptions` matching is structural and
   stays distinct — pinned by an alias test mirroring 24ba1ee9.

Guard-test hardening (rc-2vm2y suggestion): replace sole reliance on the
pinned literal-list assertion with a tri-state classification walk —
- camel-dsl test table: one entry per `CamelError` variant (25), each
  classified `Some(matchable kind)`, `Startup` (intentionally unmatchable —
  raised only at startup fail-fast, never reaches the configured route
  error handler), or `Deferred` (`TemplateReload`, pending its bd).
  Assert: every matchable classification is in
  `supported_exception_kinds()`, every vocabulary entry is classified
  matchable, `Startup`/`Deferred` entries are absent from the vocabulary.
- Anchor against silent drift: the exhaustive `variant_name` test in
  `crates/camel-api/src/error.rs` already constructs every variant; extend
  it with a count pin + cross-reference comment pointing at the camel-dsl
  classification table. No new public API in camel-api.
- Known limitation (explicit): two manually maintained tables + comments
  are review/process hardening, not mechanical prevention — a contributor
  who updates `variant_name()` and overlooks camel-dsl still needs review
  to catch the gap. A shared generated inventory would be mechanical but
  exceeds this change's no-public-API scope.

Document the intentionally-unmatchable set (`ConfigValidation`,
`EndpointUri` — startup fail-fast, never reach the route error handler) in
a doc comment on `supported_exception_kinds()` so the next audit reads the
decision in place. `TemplateReload` stays unregistered pending its bd.

bd filings (from repo root, `--deps discovered-from:rc-5u8co`):
`TemplateReload` matchability; `EndpointUri` vs `InvalidUri` vocabulary
consistency question.

## Affected crates

- camel-dsl: `src/compile.rs` — vocabulary, matcher arms, doc comment,
  tests (three match tests, one alias pin, pinned-vocabulary update,
  classification guard test).
- camel-api: `src/error.rs` tests only — count pin + cross-reference
  comment on the exhaustive `variant_name` test.

## Architecture boundaries

DSL change only (compile-time vocabulary + structural matcher); no
runtime, component, or transport code touched. camel-api gets test-only
edits, zero public-surface change. Rendering tables are NOT touched:
`pipeline_error_to_reply` already maps 415/406; PEWS already falls into
the catch-all 500 arm — matching (policy routing) is orthogonal to
rendering. Data/control plane boundary unaffected.

Single-phase change (one coherent slice, ~3 tasks).
