# Design: mcpblank

## Context

Sibling fix rc-n3t73 (4c8a0563, closed) established the pattern for this defect
class on `mcp[].server.tls.cert_path`/`key_path`: a `deserialize_with` blank
rejection in `crates/camel-dsl/src/mcp.rs` + a `schemars(regex(pattern = r"\S"))`
mirror so ROUTE_SCHEMA (and therefore R-SCHEMA lint) rejects the same values at
document validation. The rschema.rs rule already has a generic PATTERN
de-collapse arm that surfaces each pattern violation on its own leaf — no lint
mechanism work is needed beyond refreshing a stale comment.

Runtime rejections (already in place, untouched by this change):

- `name` (server/tool/resource): `validate_mcp_name` at lowering — charset
  `[A-Za-z0-9._-]+`; blank (empty or whitespace) fails it.
- `bind`: consumer-config parse in `crates/components/camel-component-mcp/src/config.rs`
  — `mcp.declared.bind` value with `trim().is_empty()` fails at consumer start.

## Goals / Non-Goals

Goals:
- Loader rejects what the runtime will, for the BLANK class only.
- Lint (R-SCHEMA) and load (serde) stay consistent with each other.

Non-goals (noted, not fixed — adjacent classes, not the blank class):
- Charset mismatch on names: schema accepts e.g. `café` or `a/b` (non-blank)
  that lowering rejects. Different defect class; file as deferral note.
- Malformed non-blank `bind` (fails `SocketAddr` parse at consumer start).
- `resource.uri` blank handling (verify runtime stance during sweep; only fix
  if the identical blank-reject mismatch exists).

## Decisions

### D1 — Return RAW, not trimmed (deviation from the TLS sibling)

The TLS sibling's `non_empty_path` trims and returns the trimmed path; that
mirrors the consumer side, which ALSO trims TLS paths (`non_empty_path`
deserializers in `camel-component-mcp/src/config.rs`). For `bind` and `name` the
runtime uses the RAW string:

- `bind` flows verbatim into the lowered `mcp.declared.bind` parameter and is
  `SocketAddr`-parsed raw — a padded `" 127.0.0.1:9100 "` is rejected today
  ("not an IP:port literal"); trimming at deserialize would silently ACCEPT it.
- `name` is charset-checked verbatim by `validate_mcp_name` — a padded
  `" crm "` is rejected at lowering today, naming the offending key.

Therefore the new deserializer rejects when `trim().is_empty()` but returns the
ORIGINAL string. Blank class is rejected at load (mission scope); every
non-blank value keeps today's exact runtime outcome. "Runtime behavior
unchanged" is literal.

### D2 — Schema pattern is unanchored `\S` (sibling-identical)

`regex(pattern = r"\S")` = "contains at least one non-whitespace character" =
not-blank-after-trim. Identical to the TLS fields, so lint diagnostics reuse the
existing PATTERN de-collapse arm and message shape.

### D3 — Sweep scope: tool/resource `name` fixed; `uri` verified-only

`RouteDslMcpTool.name` and `RouteDslMcpResource.name` are the same blank class
(runtime `validate_mcp_name` rejects blank; schema accepts) → fixed in-mission
with the identical pattern. `RouteDslMcpResource.uri`: verified during
implementation — no runtime layer rejects a blank `uri` (lowering
percent-encodes it verbatim, `mcp.rs:275-278`; no consumer-side blank check
exists), so there is no schema/runtime mismatch and no fix is warranted.

### D4 — Error message format

`"<field> must not be empty"` — sibling `non_empty_path` format, field named
(`bind`, `name`). Shared helper `non_blank_verbatim(deserializer, field)` +
thin per-field wrappers, mirroring `deserialize_cert_path`/`deserialize_key_path`.

## Risks / Trade-offs

- Schema-copy churn: `route-schema.json` (both copies) gains four `pattern`
  keywords; regenerated via `cargo xtask schema`, checked by the schema-check
  gate.
- Corpus baseline: the new negative-witness fixture must be added to
  `lint-corpus-baseline.ron` exactly once (both directions of the gate are
  exact-match).

## Phases

Single delivery phase (small, single-pattern change across three crates).
