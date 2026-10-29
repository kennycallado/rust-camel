# Design: xpath-nonfinite-refusal

## Approach

Replace the fallible-but-silent number arm in `run_query`
(`crates/languages/camel-language-xpath/src/lib.rs`) with an explicit
projection function:

```rust
fn number_to_json(n: f64) -> Result<JsonValue, LanguageError>
```

**Refusal.** `!n.is_finite()` returns
`LanguageError::EvalFailure { class: ExpressionErrorClass::Arithmetic,
position: None, detail: Some(...) }` where the detail names the non-finite
class with a stable label: `xpath number result is not finite: +Inf`,
`-Inf`, or `NaN`. Labels are bounded enum-class names, not document-derived
content, so the redaction rule (error messages exclude exchange data) holds.

Class choice — `EvalFailure` + `Arithmetic`, not `TypeMismatch`. The mission
asks this to be stated: `TypeMismatch` carries `expected`/`actual` TYPE names
and is for type mismatches; here the type (number) is correct and the VALUE is
unrepresentable, which is a value-refusal, not a type error.
`ExpressionErrorClass::Arithmetic` documents exactly this case ("overflow,
division by zero, NaN rejection") and `1 div 0` is an arithmetic overflow.
This also matches the audit's sealed Q4 ruling ("refuse loudly") for the
conversion surface. Rhai's `type-mismatch` refusals live on its
serialization surface with a different taxonomy; the XPath surface has the
finer class available and uses it.

**Fidelity.** For finite `n`, project by exactness-checked intervals:

1. `n.fract() == 0.0 && (-2^63 as f64) <= n && n < (2^63 as f64)` →
   `Number::from(n as i64)` (integer form).
2. else `n.fract() == 0.0 && (2^63 as f64) <= n && n < (2^64 as f64)` →
   `Number::from(n as u64)` (integer form). The `n >= 2^63` guard is
   mandatory: without it, a large NEGATIVE integral such as `-2^64` would
   enter this branch and the `as u64` cast would saturate to `0` — a lossy
   cast. Out-of-interval values fall through to float form.
3. else → `Number::from_f64(n)` (float form).

The bounds are half-open and exactness is guaranteed: every integral `f64`
strictly below `2^63` is at most `2^63 - 1024` (previous representable f64),
and every integral `f64` in `[2^63, 2^64)` is at most `2^64 - 2048`, so the
`as` casts are exact — not lossy. The half-open upper bound avoids the `2^63`
saturation trap (`2^63 as i64` would saturate to `i64::MAX`, a silent
off-by-one). The rule is documented as a doc comment next to the conversion.
`count()` then yields `2` in integer form. Note: serde_json 1.0.151 (the
locked version) compares `Number` values variant-strictly — integer `2` and
float `2.0` are UNEQUAL — so the existing `count` test expectation must be
updated from `from_f64(2.0)` to the integer form as part of this change;
new tests pin the form via `Number::as_i64/as_u64`.

**Verbs.** Both verbs route through `run_query`, so the refusal covers
expression and predicate. The predicate keeps its sealed Q3 strict-bool rule;
a non-finite number now fails earlier with the Arithmetic refusal instead of
today's silent Null → `TypeMismatch("null")`. Pass/fail behavior does not
change on the predicate (it failed before, fails after); the diagnosis
improves.

**Tests.** In-crate `#[cfg(test)]` only; no new dependencies (proptest is not
in the workspace and adding deps is out of bounds).

- Regression (audit R2): `1 div 0` (+Inf), `-1 div 0` (-Inf), `0 div 0`
  (NaN) on both verbs assert `EvalFailure`/`Arithmetic` and the named class;
  no `JsonValue::Null`. Fidelity cases: `count()` → integer form;
  `2.5` fractional stays float; `2^63` yields exact u64 integer form while
  `2^64`-scale integrals (positive and negative) stay float (no
  saturation); redaction: the refusal detail never contains document content.
- Property (depth-bounded roundtrip): deterministic xorshift64\* PRNG with a
  fixed seed, fixed iteration count (256), bounded depth (≤ 4) and width
  (≤ 3) XML generator from a fixed alphabet. Properties: (a) conversion is
  total and typed — finite → `Ok`, non-finite → `Err` naming the class;
  (b) integral fidelity — in-range integrals roundtrip exactly through the
  JSON number; (c) XML roundtrip — generated node text survives
  document → nodeset → string-value → document unchanged.
- Conformance harness (per-verb × audit seed): a table-driven module whose
  entries are labeled with the audit repro they pin. R1 (predicate error
  propagates, not coerced to false), R2 (non-finite refusal, both verbs),
  R4 (strict bool — number/string predicate results are `TypeMismatch`, no
  truthiness), R5 (error text excludes a document sentinel). R3
  (`${body}` stringification) is recorded N/A for XPath — it is a
  simple-language surface, not an XPath conversion surface.

## Affected crates

- `camel-language-xpath`: conversion arm + tests + docs. No other crate.

## Architecture boundaries

Languages layer only. No `camel-language-api` type changes (existing
`EvalFailure`, `ExpressionErrorClass::Arithmetic`, `TypeMismatch` variants
are reused). No Runtime/DSL/Components/Services surface moves; control-plane
error mapping (`to_expression_failed`) already forwards `EvalFailure` classes
and is exercised by existing tests. Contract authority:
`openspec/specs/language-value-boundary` (delta in this change), decisions
sealed in the e_opus papal ruling and the mission-340 audit matrix.
