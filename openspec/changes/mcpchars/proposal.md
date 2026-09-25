# Proposal: mcpchars — encode MCP name charset and bind grammar in ROUTE_SCHEMA (bd rc-vh9dt + rc-38iiz)

## Why

ROUTE_SCHEMA still accepts two malformed classes that the runtime rejects
later. Both are deferred siblings of rc-sghtz (mcpblank, landed 11717bf8),
found during the mission 264 sweep:

- Name charset (bd rc-vh9dt): `mcp[].server.name`, `mcp[].tools[].name`,
  and `mcp[].resources[].name` carry only the rc-sghtz `\S` (non-blank)
  pattern, but DSL lowering rejects every name outside `[A-Za-z0-9._-]+`
  (`camel-dsl/src/mcp.rs`, `validate_mcp_name`, bd rc-ap58). A name such as
  `café` or `a/b` lints clean (0 diagnostics), then fails at route build.
- Bind grammar (bd rc-38iiz): `mcp[].server.bind` carries only `\S`, but
  consumer start requires an IP-literal `SocketAddr`
  (`crates/components/camel-component-mcp/src/config.rs` and `registry.rs`):
  `bind 'localhost:9100' is not an IP:port literal (hostnames are not
  allowed)`. A hostname bind lints clean, then fails at consumer start.

This is the same lint/runtime contract-drift family as rc-n3t73 (blank MCP
TLS paths) and rc-sghtz (blank bind/name): the loader should flag what the
boot path will reject (fail-early principle). Lint currently approves
documents that boot rejects.

## What Changes

Mirror the mcpblank mechanism exactly — no new validation machinery:

1. `crates/camel-dsl/src/mcp.rs` — tighten the schemars pattern on the
   three `name` fields from `\S` to the anchored runtime charset
   `^[A-Za-z0-9._-]+$`. Schema-side mirror only: load keeps the
   lowering-owned charset check and its error (which names the offending
   key and the `?`/`/` hazards).
2. `crates/camel-dsl/src/mcp.rs` — tighten `server.bind` from `\S` to the
   anchored IP-literal shape
   `^((\d{1,3}\.){3}\d{1,3}|\[[0-9A-Fa-f:.]+\]):\d{1,5}$`, and extend
   `deserialize_mcp_bind` past blank rejection with the runtime's own
   predicate: a `std::net::SocketAddr` parse of the verbatim value (the
   consumer parses the identical string), rejecting at load with the
   runtime's message text.
3. Regenerate schema copies (`cargo xtask schema`):
   `schemas/dsl/route-schema.json` + `crates/camel-lint/schema/route-schema.json`.
4. R-SCHEMA pins for both classes in `crates/camel-lint/src/rules/rschema/tests.rs`,
   two lint-corpus negative witnesses (`mcp-name-charset.yaml`,
   `mcp-bind-grammar.yaml`), and their `lint-corpus-baseline.ron` entries.
5. Spec delta `route-lint`: two ADDED requirements (name charset, bind
   grammar) and one MODIFIED requirement (the rc-sghtz blank requirement —
   every current scenario name carried exactly; its "deeper validation
   stays runtime-owned" clause moves to the new requirements).

## Acceptance Criteria

- A document with `server.name: café` (or `a/b`) emits exactly one
  R-SCHEMA Error anchored on the name value; a document with
  `bind: localhost:9100` or `bind: 127.0.0.1` (no port) emits exactly one
  R-SCHEMA Error anchored on the bind value.
- Valid documents stay clean: `[::1]:9100`, `127.0.0.1:9100`, and names
  with `.`/`_`/`-` separators (`crm-api_v2.prod`) produce zero R-SCHEMA
  diagnostics; the clean corpus witness `mixed-routes-rest-mcp.yaml` still
  lints clean end to end.
- Load behavior: a hostname or port-less `bind` fails DSL parse with
  `not an IP:port literal`; names keep their verbatim load (charset
  rejection stays at lowering, byte-identical error).
- `cargo test -p camel-dsl --lib` green (830-test baseline, additions
  only); `cargo test -p camel-lint rschema` green including all rc-sghtz
  blank pins; `cargo test -p camel-cli --test lint_corpus` green in both
  directions.
- `cargo xtask schema --check` passes (copies regenerate byte-stable).

## Impact / Risk Budget

Risk: rejecting documents that previously loaded. The in-tree sweep found
only IP-literal binds (`docs/src/components/mcp.md`, `docs/src/yaml-dsl/step-verbs.md`,
all lint-corpus fixtures), so no valid in-tree document changes outcome.
The one deliberate behavior shift is the padded bind `" 127.0.0.1:9100 "`:
mcpblank left its rejection to consumer start; this change rejects it at
load with the same verdict, earlier (design.md D4). Lint-side patterns are
anchored (design.md D2) — a deviation from mcpblank's unanchored `\S`,
required because charset and grammar are full-string properties.
