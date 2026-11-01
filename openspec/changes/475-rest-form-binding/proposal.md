# Proposal: 475-rest-form-binding

## Why

REST DSL v2 currently offers exactly two binding modes: `json` (structured,
default) and `raw` (verbatim passthrough). Any non-JSON body — including
`application/x-www-form-urlencoded`, the media HTML forms and countless
legacy endpoints emit — forces route authors into `binding: raw` plus a
hand-written parse step. Real demand exists today (bd `rc-6s2rg` notes,
2026-10-29: a camel-astro contact form currently parses form bodies with a
rhai script and manual JSON-Schema wiring).

This change adds structured binding for exactly one media family:
`application/x-www-form-urlencoded`. It is the high-value slice of umbrella
`rc-yaqrf` (XML and multipart remain there; e_gpt viability split the
umbrella).

## What Changes

- New REST DSL binding mode `binding: form` (`RouteDslRestBinding::Form`):
  media validation (consumes/produces MUST be
  `application/x-www-form-urlencoded`, optional `; charset=utf-8`), lowering
  to `Unmarshal { unmarshal: "form" }` / `Marshal { marshal: "form" }`,
  everything else in the injected pipeline unchanged. `request_schema` /
  `response.schema` ARE allowed (form body becomes JSON before schema
  validation).
- New bounded built-in data format `form` (`name() == "form"`) in
  `crates/camel-processor/src/data_format/`, registered in
  `builtin_data_format_with_config`. Unmarshal `Body::Text`/`Body::Bytes` →
  `Body::Json` object (duplicate keys → arrays, no lossy overwrite); strict
  percent-decoding, `+` = space, UTF-8 only, hard caps on field count and
  total bytes, fail-closed typed errors. Marshal JSON object → form text
  (arrays repeat keys; nested structures rejected). `Body::Empty` is a
  no-op (mirrors json); `Body::Stream` fails closed and streams are never
  consumed.
- Tests (parser matrix, marshal matrix, DSL validation matrix, executed
  end-to-end pipeline test), docs (REST DSL binding section + data-format
  reference), and spec deltas (`rest-dsl` + `data-formats`).

Excluded (stay in umbrella `rc-yaqrf` or non-goals): XML binding, multipart,
header auto-mapping, query-parameter binding, non-UTF-8 charsets, new crate,
publish-registration changes, user-facing configuration beyond format caps.

## Acceptance criteria

- `binding: form` validates media with an actionable `RouteError` naming the
  operation; unknown binding strings still fail closed at deserialize time.
- Form unmarshal produces a JSON object; duplicate keys become arrays with
  values in first-seen (encounter) order; no lossy object overwrite.
- Malformed `%` escapes, invalid UTF-8, over-cap field counts and over-cap
  input bytes all fail closed with typed errors.
- Marshal rejects nested objects/arrays and non-object top-level values;
  `Body::Empty` passes through unchanged (matching the JSON format — a 204
  handler must not fail at marshal); `Body::Stream` fails closed and streams
  are never consumed.
- Raw and JSON pipelines stay byte-identical (regression tests untouched).
- `cargo test -p camel-processor -p camel-dsl` green; fmt/clippy/xtask
  lints/schema-check/doc gates green; `openspec validate --strict` green.

## Risk budget

Acceptable: new enum variant + new module, additive only. Out of bounds:
touching JSON/raw lowering semantics, enabling serde_json `preserve_order`
(would change JSON output ordering workspace-wide), new crates, stream
consumption in the form format. bd: `rc-6s2rg` (P2), umbrella `rc-yaqrf`.
