# Proposal: splitbody

## Why

Declarative split fragment `Body` type depends on how many nodes the split
expression matches (bd rc-etf0q, external report camel-cache 2026-09-24,
100% repro on a 48-item RSS feed). With `split: {expression: {xpath:
"//item/link"}}`:

- 1 match: the xpath language collapses the nodeset to a scalar string, the
  split compiler's `Value::String` branch runs, and `${body}` renders the raw
  text (`https://example.org/bar.xml`).
- 2+ matches: the xpath language yields an array of strings, the compiler's
  `Value::Array` branch wraps every element via `Body::from(serde_json::Value)`
  → `Body::Json`, and the simple language renders `Body::Json` with
  `v.to_string()` — literal quote characters (`"https://..."`). Downstream
  `recipient_list` then builds invalid endpoint URIs.

Same defect class hits any language that returns `Value::Array` of strings
(jsonpath over `["a","b"]`).

## What Changes

Included:

- `camel-core` `splitting.rs` `Value::Array` branch: string elements map to
  `Body::Text` (exact parity with the 1-match path); number/bool/object/array
  elements keep `Body::Json` (their JSON text form carries no quotes).
- Six guard tests (e_glm-validated conditions a–f): xpath repro, match-count
  parity, collect_all aggregate indifference, jsonpath string-array, non-string
  elements stay JSON, programmatic `split_body_json_array` unchanged.
- Split docs callout: fragment typing rule (`docs/src/eip/splitter.md`).
- Spec delta: `eip-splitter` fragment body typing requirement.

Excluded (tracked by separate bds already filed as dependents of rc-etf0q):
programmatic `split_body_json_array` own typing change; `//item` subtree
serialization enhancement; xpath nodeset count-dependent shape (by design for
predicates, unchanged).

## Acceptance criteria

- xpath split with 1 vs 2 vs 3 string-node matches: fragment bodies
  identical (`Body::Text`, raw string, no quote characters).
- jsonpath string-array split → `Body::Text` fragments.
- Number/object array elements → `Body::Json` fragments (unchanged).
- `collect_all` aggregation output byte-identical before/after the fix.
- Programmatic `split_body_json_array` behavior unchanged (pin test).
- Gates green: fmt, clippy (all 4 legs), camel-core `--lib` + relevant
  camel-test/camel-api tests, lint-unbounded-wait (ratchet 296), schema-check,
  doc-build.

## Risk budget

Low: one match arm in one compiler closure; no public API change; no DSL
schema change; data-plane only. Accepted delta: empty-string array elements
render `` instead of `""` through `${body}`. Out of bounds: touching
`split_body_json_array`, the simple evaluator, or the xpath nodeset shape.

Affected crates: camel-core (fix), camel-test + camel-api (guard tests),
docs/src (callout). bd: rc-etf0q.
