# Design: mcpchars

## Context

mcpblank (11717bf8, bd rc-sghtz) closed the blank class: `non_blank_verbatim`
deserialize rejection plus an unanchored `\S` schemars pattern on the MCP
server/tool/resource `name` and server `bind` fields. Mission 264 filed two
adjacent classes in the same lint/runtime contract-drift family; this change
closes both, mirroring the landed mechanism (no new validation machinery).

Runtime contracts being mirrored:

- Name charset — `camel-dsl/src/mcp.rs` `validate_mcp_name` (bd rc-ap58):
  non-empty, every char ASCII alphanumeric or `.`/`_`/`-`. Enforced at
  lowering; names travel verbatim into lowered `mcp:<server>/<kind>/<name>`
  URI path segments (`?` truncates the URI and can shadow the `schema`
  param; `/` breaks the segment shape).
- Bind grammar — `crates/components/camel-component-mcp/src/config.rs`
  `validate_bind_policy` and `registry.rs`: `std::net::SocketAddr` parse of
  the verbatim string; hostnames rejected
  (`bind 'localhost:9100' is not an IP:port literal (hostnames are not
  allowed)`).

## Goals / Non-Goals

Goals:

- R-SCHEMA flags name-charset violations and malformed binds that the
  runtime rejects (one Error per offending value, anchored on the value).
- Bind grammar is also enforced at load (DSL deserialize) with the
  runtime's own predicate and message text.
- Blank-class behavior from rc-sghtz is preserved exactly (tighter
  patterns are supersets of `\S`).

Non-Goals:

- No change to the runtime crates (`camel-component-mcp` untouched).
- No load-time charset rejection for names (lowering keeps it — its error
  names the offending key and explains the `?`/`/` hazards; moving it to
  serde would duplicate that machinery for no runtime gain).
- No exhaustive IPv6 validation in the schema pattern (regex cannot
  faithfully encode RFC 4291 compression rules; see D3).

## Decisions

### D1 — Name fields: schema-side mirror only

The three `name` fields replace `\S` with the anchored charset regex
`^[A-Za-z0-9._-]+$` (bd rc-vh9dt fix pattern verbatim: "schemars regex
mirror of the charset on the name fields + schema copies synced"). The
deserialize side keeps only the rc-sghtz blank check. Load behavior for
names is unchanged byte-for-byte: charset violations still fail at lowering
with the existing rich error.

### D2 — Anchored patterns (deviation from mcpblank's unanchored `\S`)

JSON Schema `pattern` is an unanchored partial match (ECMA-262 semantics;
the `jsonschema` crate matches this). mcpblank's `\S` was unanchored
because blank-vs-non-blank is an existence property. Charset and bind
grammar are full-string properties: an unanchored `[A-Za-z0-9._-]+` would
match the `caf` inside `café` and pass the violation. Therefore both new
patterns are anchored `^...$`.

### D3 — Bind: two-layer mirror, load is exact, lint is shape

- Load (deserialize): `deserialize_mcp_bind` keeps the blank check, then
  runs the runtime's own predicate — `value.parse::<std::net::SocketAddr>()`
  (the consumer parses the identical string at config.rs:285) — and
  rejects with the runtime's message text: `bind '{value}' is not an
  IP:port literal (hostnames are not allowed)`. This is the exact runtime
  mirror: zero drift possible, no approximation.
- Lint (schema pattern): `^((\d{1,3}\.){3}\d{1,3}|\[[0-9A-Fa-f:.]+\]):\d{1,5}$`
  — dotted-quad v4 or bracketed v6 (hex/colons, embedded-v4 dots allowed),
  colon, 1–5 digit port. Necessary approximation: a regex cannot faithfully
  validate IPv6 zero-compression or octet ranges. It is shape-exact in the
  no-false-positive direction — every string `SocketAddr` parse accepts
  matches it (v4 octets are 1–3 digits; v6 literals use only hex digits,
  colons, and embedded-v4 dots; ports are 1–5 digits) — so no valid bind
  ever lints dirty. Invalid values inside the shape (`999.1.1.1:80`) pass
  lint but die at load, where the exact parse runs.

### D4 — Padded bind fate moves to load (extends mcpblank D1's fate map)

mcpblank D1 preserved `" 127.0.0.1:9100 "` (padded bind) to its consumer-start
rejection. With D3's deserialize parse, the padded value now fails at DSL
load — same verdict, earlier, and now with the runtime's own message. The
padded-NAME half of that contract is untouched (charset stays
lowering-owned). The mcpblank pin `non_blank_padded_values_load_verbatim`
is restructured: padded names still load verbatim; the padded bind case
becomes an explicit load-rejection pin.

### D5 — Env tokens are safe on both layers

The boot path interpolates `${env:...}` before serde on every parse route
(discovery.rs `interpolate_for_parse` runs ahead of `serde_yml::from_str` /
`serde_json::from_str`; an unset no-default variable fails at interpolation
before deserialize), so the D3 deserialize parse only ever sees resolved
values. The lint path validates the interpolated copy (rc-93wct): a
whole-scalar `${env:BIND:-127.0.0.1:9100}` validates against the default;
no-default whole-scalar tokens are already explicit R-SCHEMA Errors today —
unchanged by this change.

## Risks / Trade-offs

- Rejecting previously-loading documents: the in-tree sweep (fixtures,
  docs, examples) found only IP-literal binds and charset-valid names, so
  no valid in-tree document changes outcome. The corpus gate
  (`lint_corpus`, exact-match both directions) guards against drift.
- Schema-copy drift: both `route-schema.json` copies regenerate from the
  schemars attributes; `cargo xtask schema --check` pins them.
- The rschema.rs module comment about the pattern-keyword set needs a
  refresh (comment only), mirroring mcpblank task 1.2.
