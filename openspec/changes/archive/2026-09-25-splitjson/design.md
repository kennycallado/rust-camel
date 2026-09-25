# Design: splitjson

## Context

Mission 251 (bd rc-etf0q, 938bea3d) fixed the declarative split's array
branch: `Value::String` elements map to `Body::Text`, all other elements keep
`Body::Json`. It left the programmatic `split_body_json_array()` untouched and
pinned that old behavior with a transitional test
(`split_body_json_array_string_elements_stay_json`) plus a spec scenario
("Programmatic body_json_array keeps JSON typing"), both naming bd rc-fflhv as
the owner of the future change.

## Decision

Mirror 251's fix exactly at the programmatic seam — the element-driven match
in `split_body_json_array`:

```rust
match val {
    serde_json::Value::String(s) => fragment_exchange(exchange, Body::Text(s.clone())),
    other => fragment_exchange(exchange, Body::Json(other.clone())),
}
```

The declarative split is the precedent and does not change. Alternatives
(global `${body}` unwrap of `Body::Json(String)`; docs-only) were evaluated
and rejected during 251; the same ruling applies here.

## Consequences

- Cross-path consistency: string elements type `Body::Text` in both paths.
- `${body}` rendering of string fragments loses the JSON quote characters.
- `collect_all` aggregation output is unchanged: both fragment typings
  aggregate to the same `Value::String`.
- The transitional pin test and the spec scenario flip from "keeps JSON
  typing" to element-driven parity.

## Phases

Single delivery phase: one seam, one crate, ~6 production lines plus tests,
docs, and the spec delta. No phase decomposition.

## Scope scan (order item 3)

`camel-api` splitter fragment construction sites:

- `split_body_lines` → `Body::Text` per line — already correct.
- `split_body(f)` → the custom closure owns its typing policy — by design.
- Streaming split → fragment typing checked during implementation; findings go
  to the park report (fix only if trivial and in-crate, else note for the
  park).
