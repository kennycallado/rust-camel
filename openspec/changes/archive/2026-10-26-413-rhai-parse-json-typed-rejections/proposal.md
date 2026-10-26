# Proposal: 413-rhai-parse-json-typed-rejections

## Why

`parse_json` in the Rhai host module has a single native overload taking an
`ImmutableString`. When a script passes a non-string body — a `Blob` from
`Body::Bytes`, or a native map/array from `Body::Json` — Rhai finds no matching
overload and raises `ErrorFunctionNotFound` at the call position. The step then
reports `function-not-found`, which is indistinguishable from a missing host
function and sent a downstream team hunting for a registration/tier bug
(bd rc-2nx48).

The `language-value-boundary` spec currently forbids new failing overloads:
"these guards are the only failing overloads added". That sentence is now
false: the implementation adds exact-type `parse_json` guards so the call
resolves and fails with a typed mismatch.

## What Changes

- Amend the classifier requirement to allow the `parse_json` exact-type guards
  as failing overloads that classify as `type-mismatch` (expected `string`,
  actual is the static Rhai type name), never `function-not-found`.
- Add the scenario pinning the non-string argument behaviour and the no-data
  redaction rule.
- No other requirement changes: the string-only contract, body binding, limits,
  wrapper arithmetic (`function-not-found`), comparison guards, and malformed
  parse classification all stay as specified.
