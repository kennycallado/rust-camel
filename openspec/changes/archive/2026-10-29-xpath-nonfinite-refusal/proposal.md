# Proposal: xpath-nonfinite-refusal

## Why

Mission 340's language-boundary audit (finding V2, repro R2) showed that a
non-finite XPath number silently degrades at the language → engine boundary:
`crates/languages/camel-language-xpath/src/lib.rs` converts `SxdValue::Number(n)`
with `serde_json::Number::from_f64(n).unwrap_or(JsonValue::Null)`. `1 div 0`
(+Inf) therefore becomes `JsonValue::Null`, and `set_body` maps that Null to
`Body::Empty` — the exchange continues downstream with its body silently
erased and no error anywhere. The same conversion also degrades integral
results: `count(/root/item)` yields the float form `2.0` instead of the JSON
integer `2`, a silent M-type change for typed consumers.

The boundary contract (`openspec/specs/language-value-boundary`) already
seals the rule for Rhai: unrepresentable values are refused, never stringified
or nullified, and the audit sealed Q4 "refuse loudly" for the XPath number
surface. This change brings the XPath surface into compliance with that
contract.

## What Changes

- Non-finite XPath numbers (`+Inf`, `-Inf`, `NaN`) are refused with a typed
  `LanguageError::EvalFailure` carrying `ExpressionErrorClass::Arithmetic` and
  a payload-free detail naming the non-finite class. `JsonValue::Null` is
  never produced from a non-finite number. Applies to both the expression and
  predicate verbs (the predicate path routes through the same conversion).
- A documented number-fidelity rule: finite integral values project to JSON
  integer form when exact (bounded, exactness-checked intervals — no lossy
  cast); fractional and out-of-range values keep float form.
- An audit R2 regression test (Inf/NaN refusal on both verbs) plus
  finite-number fidelity cases.
- A depth-bounded roundtrip property test for the conversion surface
  (deterministic seeded PRNG; no new dependencies — proptest is not in the
  workspace) and a minimal per-verb conformance harness seeded from audit
  R1–R5 where they apply to XPath (R2 refusal, R1 predicate-error
  propagation, R4 strict-bool, R5 redaction; R3 is N/A for XPath).
- Delta spec on `language-value-boundary` and XPath docs updates (refusal,
  number rule, strict no-truthiness boundary).

Excluded: nodeset subtree shape (rc-oz4f7), truthiness changes, JSONPath/
Simple/Rhai semantics, new dependencies.

## Acceptance criteria

- `cargo test -p camel-language-xpath` passes, including the new refusal,
  fidelity, property, and conformance tests.
- `cargo fmt --check --all`, `cargo clippy -p camel-language-xpath
  --all-targets -- -D warnings`, the usual xtask lints, and
  `RUSTDOCFLAGS="-D warnings" cargo doc -p camel-language-xpath --no-deps`
  pass.
- `cd docs && mdbook test` and `openspec validate --all --strict` pass.

## Risk budget

Low. The refusal changes one conversion arm; the only behavioral risk is
routes that depended on Inf/NaN becoming Null — exactly the silent downgrade
the audit requires us to break. Integer-form projection preserves the
represented numeric value but changes the JSON representation form
(`2` vs `2.0`), which is an intentional compatibility change for consumers
that compare `serde_json::Number` variant-sensitively; exactness is checked
before any cast.

Bd: rc-29htd
