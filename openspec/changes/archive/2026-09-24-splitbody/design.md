# Design: splitbody

## Approach

In the `DeclarativeSplit` compiled `SplitExpression`
(`crates/camel-core/src/lifecycle/adapters/step_compilers/splitting.rs`), the
`Value::Array` branch currently maps every element through `Body::from(v)`
which resolves to `impl From<serde_json::Value> for Body` → `Body::Json`
(camel-api `body.rs`). Change the per-element mapping to an inner match:

```rust
Value::Array(arr) => Ok(arr
    .into_iter()
    .map(|v| {
        let mut fragment = exchange.clone();
        fragment.input.body = match v {
            Value::String(s) => Body::Text(s),
            other => Body::Json(other),
        };
        fragment
    })
    .collect()),
```

(`camel_api::Value` is a type alias for `serde_json::Value`, so the element
match is on the same type the closure already received.)

Verified causal chain (bd rc-etf0q, do not re-litigate):

1. `camel-language-xpath/src/lib.rs` nodeset→Value: `1 match => String`,
   `N matches => Array[String]` — count-dependent shape, kept by design for
   predicates.
2. `splitting.rs` `Value::Array` branch: `Body::from(v)` → `Body::Json` per
   element — the defect.
3. `camel-language-simple/src/evaluator.rs:26`:
   `Body::Json(v) => Value::String(v.to_string())` — serde adds the quotes;
   `Body::Text` renders raw.

Guard placement:

- (a)(b)(c) `crates/camel-test/tests/xpath_test.rs` — e2e through YAML
  `split: {expression: {xpath: ...}}` routes (CamelTestContext + mock; the
  file already registers the xpath language and asserts on mock endpoints).
- (d)(e) `crates/camel-test/tests/jsonpath_test.rs` — string-array and
  number/object element splits.
- (f) `crates/camel-api/src/splitter.rs` unit tests — pin
  `split_body_json_array` string fragments as `Body::Json` (unchanged; a
  separate bd owns that splitter's typing).

## Affected crates

- camel-core: one match arm in `splitting.rs` (production change).
- camel-test (tests only): guards a–e.
- camel-api (tests only): guard f pin.
- docs: `docs/src/eip/splitter.md` fragment-typing callout.
- openspec: `eip-splitter` delta spec (this change).

## Architecture boundaries

Data-plane only: fragment `Body` construction inside the compiled split
closure. No control-plane, DSL schema, component, or language-crate changes —
camel-language-xpath and camel-language-simple are untouched. ADR-0001: still
compiles into the same Tower `Service<Exchange>` step. ADR-0025: outcome-aware
split semantics untouched — Stop/aggregation flow unchanged; `CollectAll`
(`aggregate_completed`) maps `Body::Text` and `Body::Json(String)` to the same
`Value::String`, so aggregation output is byte-identical.

## Alternatives considered

- Option B — unwrap `Body::Json(Value::String)` inside the simple-language
  `${body}` evaluator: global semantics change for every route; rejected
  (e_glm-validated Option A).
- Option C — docs-only: leaves the defect; rejected.
- Always-array xpath nodeset shape: breaks predicate truthiness and header
  use; rejected.

Single-phase change; no `## Phase N` grouping (one production edit plus
tests/docs/spec riding the same slice).
