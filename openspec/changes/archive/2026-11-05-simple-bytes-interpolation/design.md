# Design: simple-bytes-interpolation

## Approach

The Simple template assembly (`Expr::Interpolated`) renders each slot value with
`Value::Null → ""`, `Value::String(s) → s`, else `other.to_string()`. A
Bytes-backed body arrives as `Value::Array` of integers, so it stringifies as a
number list. The fix is scoped to the interpolation assembly only:

- In BOTH interpolation sites (`eval_sync_expr` and `eval_async_expr` in
  `crates/languages/camel-language-simple/src/evaluator.rs`), special-case the
  sub-expression shape `Expr::Body` (a bare `${body}` slot inside a larger
  template). Instead of the generic render, call a new helper
  `interpolate_body_text(exchange) -> Result<String, LanguageError>`:
  - `Body::Bytes(b)`: `std::str::from_utf8(b)` → OK: the text;
    Err: `LanguageError::ConversionError { source_type: "Body::Bytes", target: "text" }`
    (class `conversion`). Never lossy, no U+FFFD.
  - every other body variant: delegate to the existing `body_value` + render
    chain so Text/Xml/Json/Empty and the `Body::Stream` typed refusal behave
    exactly as today.
- All other slot sub-expression shapes keep the generic render. Comparisons,
  arithmetic, `default`/`min`/`max` arguments, and `Expr::BodyField` (`${body.field}`)
  continue to see the native `body_value` projection — the array. The bare
  whole-slot `${body}` never reaches the interpolation loop (whole-slot sources
  return native values), so the canonical byte-array scenario is untouched.
- Rhai, JavaScript, MiniJinja, and `convert_body_to` are untouched.

This mirrors the rule the canonical requirement already gives MiniJinja: UTF-8
text on valid bytes, typed `conversion` error otherwise, never lossy decoding.

TDD: the six acceptance tests in the proposal are written first (red), then the
helper is implemented until green.

## Affected crates

- `camel-language-simple`: interpolation sites + new helper + tests.
- `docs`: one-line rule in `docs/src/languages/simple.md` next to the existing
  Bytes note (line ~63).
- `openspec/specs/language-value-boundary`: MODIFIED delta (via the change's
  `specs/` directory), interpolated-context rule added.

## Architecture boundaries

Languages boundary only. The Simple language crate owns its template rendering;
no runtime, DSL, component, or service surface changes. Data/control plane
split is untouched — this is data-projection ergonomics at the language
evaluation seam. Respects the canonical value-boundary spec: the bare access
integrity rule is preserved; interpolation is a presentation context, not a
value access. Diagnostics stay payload-free: the conversion error names
`Body::Bytes` and `text`, never byte contents (consistent with the existing
`stream_body_error` pattern).

Single-phase change: no delivery-phase decomposition needed — one crate, one
assembly path, one docs line.

## Alternatives considered

- **Decode Bytes to text at `body_value`** — rejected: it would break the bare
  `${body}` array projection and the canonical "Bytes read as a byte-sequence
  array" scenario, and would change predicate/comparison operands.
- **Introduce a typed `Value::Bytes` variant** — rejected: a cross-crate value
  model change far out of scope for an ergonomics fix.
- **Document-only (bd alternative)** — rejected by the mission order; the
  binding design fixes the behavior.
