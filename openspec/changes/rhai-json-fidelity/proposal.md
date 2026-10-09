# Proposal: rhai-json-fidelity

## Why

Stock Rhai JSON helpers reject valid `\/` escapes, emit invalid Rust `Debug`
escapes, lose large-integer precision, and sort object keys. Downstream routes
fail far from the cause. The camel-112 team reported bd rc-qka42.

## What Changes

- Register a shared serde_json-backed host module (`parse_json`, `to_json`,
  wrapper) shadowing the stock built-ins on every sandbox engine.
- `parse_json` descends level at a time with `serde_json::value::RawValue`,
  validating iteratively with an owned depth cap of 128 (top scalar 0, top
  container 1; 128 accepted, 129 is `limit`). Input `max-string-size` is
  enforced before descent, cost is bounded by 128 × input bytes, and
  `arbitrary_precision` / `unbounded_depth` are NOT enabled.
- Private tree: exact number tokens, `indexmap::IndexMap` objects, `Arc`
  copy-on-write values. Integer-form tokens with no fraction or exponent that
  parse as `i64` project native; decimals and exponents are always `JsonNumber`.
- Wrapper surface: string/integer get/set through single `Dynamic` setters
  validated internally, chained assignment, dot fallback, negative indexes;
  `len`, `contains` (objects only), `keys`, `remove` (shift), `push`; unit to
  null; comparison-refusal guards, with no numeric comparison engine.
- Bounds: the depth-128 invariant applies to every set/push/conversion and
  native-container `to_json` (target path plus value depth; self-assignment
  refused); array/map caps and a total size estimate are checked at parse and on every
  mutation; the serializer is bounded.
- `converter.rs` `dynamic_to_value` adds outbound refusal arms with the stable
  labels `json value` / `json number`; the inbound converter is unchanged.
  Unavailable operations keep the existing classifier: `+`, `+=`, `values`, and
  `merge` are `function-not-found`, `for` is `runtime`, and every
  registered wrapper comparison fails `type-mismatch`.
- `docs/src/languages/rhai.md` and the crate `CONTEXT.md` are updated.

**Compatibility break.** Decimal and exponent JSON numbers are now
`JsonNumber`, not `f64`. Scripts that did arithmetic on them (or `+=`), or that
relied on them being `f64`, break explicitly. Registered wrapper comparisons
are refused. Explicit lossy comparison uses `JsonNumber.to_float()`.

**Excluded:** inbound converter and scope preparation stay unchanged (rc-m01r9
and rc-141om out of scope). Direct outbound wrapper conversion is deferred, so
`set_body(parse_json(...))` now fails as an explicit compatibility break;
persistence uses `to_json` or a native leaf.

## Acceptance criteria

- `to_json(parse_json(s))` re-emits equivalent JSON for `\/`, Unicode and
  control characters, full numeric magnitude, and key order. Wrapped number
  tokens remain exact; native i64 spelling may normalize (`-0` to `0`).
- Malformed JSON (including unpaired surrogates), non-finite floats, and
  unsupported values fail loudly with redacted typed errors.
- Depth 128 parses; mutation depth and self-assignment are refused with `limit`.
- Chained and dot access mutate the root; a bare read is isolated.
- Native `Map` / `Array` serialization, comparison guards, and outbound refusal
  labels are pinned by tests.

## Risk budget

Acceptable: the documented `parse_json` / `to_json` and fractional/exponent
compatibility breaks, and the per-eval cost of the shared host module. Out of
bounds: silent corruption, unredacted exchange data in errors, unbounded
allocation or recursion, and any workspace-wide serde_json feature expansion.
