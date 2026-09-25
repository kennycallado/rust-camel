# Tasks: mcpchars

## Task 1.1 — camel-dsl + camel-lint: name-charset schema mirror (bd rc-vh9dt)

Files:

- `crates/camel-dsl/src/mcp.rs` (modified)
- `schemas/dsl/route-schema.json` (modified, generated)
- `crates/camel-lint/schema/route-schema.json` (modified, generated)
- `crates/camel-lint/src/rules/rschema.rs` (modified, comment only)
- `crates/camel-lint/src/rules/rschema/tests.rs` (modified)
- `crates/camel-cli/tests/fixtures/lint-corpus/mcp-name-charset.yaml` (new)
- `crates/camel-cli/tests/fixtures/lint-corpus-baseline.ron` (modified)

Steps:

1. In `crates/camel-dsl/src/mcp.rs`, replace the rc-sghtz `\S` schemars
   pattern with the anchored runtime charset on the THREE name fields —
   `RouteDslMcpServer.name`, `RouteDslMcpTool.name`,
   `RouteDslMcpResource.name`:
   `#[cfg_attr(feature = "schema", schemars(regex(pattern = r"^[A-Za-z0-9._-]+$")))]`.
   Do NOT touch the `deserialize_with` attributes (charset stays
   lowering-owned at load — design.md D1). Do NOT touch `bind` or the TLS
   path fields in this task.
2. Regenerate the schema copies: `cargo xtask schema`; verify the three
   name-field patterns changed in BOTH `schemas/dsl/route-schema.json` and
   `crates/camel-lint/schema/route-schema.json`, nothing else drifted.
3. Refresh the rschema.rs module-comment mention of the MCP pattern set
   (comment only): the name fields now carry the anchored charset, bind the
   IP-literal shape (lands with task 1.2), TLS paths keep `\S`.
4. In `rschema/tests.rs`, mirroring the rc-sghtz pins (see
   `rschema_mcp_blank_bind_empty_rejected` for the harness pattern), add:
   - `rschema_mcp_name_charset_server_rejected` — doc with
     `server.name: café`, valid `bind: 127.0.0.1:9100`: exactly one
     R-SCHEMA Error, anchored on the raw name token (slice and assert it
     equals `"café"`), message contains `does not match`; no diagnostic on
     the bind value
   - `rschema_mcp_name_charset_tool_name_rejected` — valid server, tool
     `name: a/b`: one Error anchored on the tool name value
   - `rschema_mcp_name_charset_valid_separators_clean` — names
     `crm-api_v2.prod` (server), `lookup_v2` (tool), `customers.list`
     (resource), valid bind: zero R-SCHEMA diagnostics
5. New corpus fixture `mcp-name-charset.yaml` — negative witness whose
   header comment states the contract and cites rc-vh9dt (mirror
   `mcp-blank-bind-name.yaml` phrasing): body carries
   `server.name: café` and a tool `name: a/b` (both violation shapes in one
   file; bind valid).
6. Add the fixture's expected entry to `lint-corpus-baseline.ron` in the
   mcpblank entry format (one `("R-SCHEMA", "error")` pair; the baseline is
   a set per file) with a justification comment.

Tests (camel-dsl — the schema attr is compile-time only, so pin the load
contract is UNCHANGED; reuse the existing lowering pin):

- existing `name_with_invalid_charset_rejected` (lowering) — must stay
  green untouched (charset rejection stays at lowering, D1)
- existing blank pins — must stay green (anchored charset is a superset of
  `\S` for the blank class)

Commands:

- `cargo test -p camel-dsl --lib mcp::` — green
- `cargo test -p camel-lint rschema` — new pins green, full set green
- `cargo test -p camel-cli --test lint_corpus` — exact-match both directions
- `cargo xtask schema --check` — passes

Acceptance:

- All commands above exit 0
- Both schema copies show `^[A-Za-z0-9._-]+$` on exactly the three name
  fields (grep `"pattern": "^\\[A-Za-z0-9._-\\]\\+$"` in both JSON copies)
- Commit with footer `Bd: rc-vh9dt`

- [x] Task 1.1

## Task 1.2 — camel-dsl + camel-lint: bind-grammar schema + load mirror (bd rc-38iiz)

Files:

- `crates/camel-dsl/src/mcp.rs` (modified)
- `schemas/dsl/route-schema.json` (modified, generated)
- `crates/camel-lint/schema/route-schema.json` (modified, generated)
- `crates/camel-lint/src/rules/rschema/tests.rs` (modified)
- `crates/camel-cli/tests/fixtures/lint-corpus/mcp-bind-grammar.yaml` (new)
- `crates/camel-cli/tests/fixtures/lint-corpus-baseline.ron` (modified)

Steps:

1. In `crates/camel-dsl/src/mcp.rs`, replace the `\S` schemars pattern on
   `RouteDslMcpServer.bind` with the anchored IP-literal shape:
   `#[cfg_attr(feature = "schema", schemars(regex(pattern = r"^((\d{1,3}\.){3}\d{1,3}|\[[0-9A-Fa-f:.]+\]):\d{1,5}$")))]`.
2. Extend `deserialize_mcp_bind` (design.md D3): after the
   `non_blank_verbatim` blank check, run the runtime's own predicate —
   `value.parse::<std::net::SocketAddr>()` — and on failure return
   `Err(serde::de::Error::custom(format!("bind '{value}' is not an IP:port literal (hostnames are not allowed")))`
   (the consumer's message text, config.rs:285). Update the helper's
   doc-comment to state the two layers: blank rejection (rc-sghtz) plus
   the `SocketAddr` grammar mirror (rc-38iiz), returning the ORIGINAL
   string on success.
3. Restructure the mcpblank pin `non_blank_padded_values_load_verbatim`
   (design.md D4): the padded-NAME half keeps verbatim load
   (`name: " crm "` parses, value byte-equal); split the padded-bind case
   into a new pin asserting load rejection with `not an IP:port literal`.
4. New camel-dsl tests (mcp.rs tests module, mirroring existing YAML-parse
   tests):
   - `bind_grammar_hostname_rejected_at_load` — `bind: localhost:9100`:
     parse fails, error contains `not an IP:port literal`
   - `bind_grammar_portless_rejected_at_load` — `bind: "127.0.0.1"`:
     parse fails, same message
   - `bind_v6_literal_loads_verbatim` — `bind: "[::1]:9100"`: parses,
     value byte-equal
5. In `rschema/tests.rs`, mirroring the rc-sghtz pins, add:
   - `rschema_mcp_bind_hostname_rejected` — `bind: localhost:9100`, valid
     `name: crm`: exactly one R-SCHEMA Error anchored on the raw bind
     token, message contains `does not match`; no diagnostic on `name`
   - `rschema_mcp_bind_portless_rejected` — `bind: "127.0.0.1"`: one Error
     anchored on the bind value
   - `rschema_mcp_bind_valid_v4_v6_clean` — `bind: 127.0.0.1:9100` and a
     second doc with `bind: "[::1]:9100"`, valid names: zero R-SCHEMA
     diagnostics
6. New corpus fixture `mcp-bind-grammar.yaml` — negative witness whose
   header cites rc-38iiz: body carries `bind: localhost:9100` with a valid
   name. Add its `("R-SCHEMA", "error")` entry to
   `lint-corpus-baseline.ron` with a justification comment.
7. Regenerate schema copies (`cargo xtask schema`) and verify only the
   bind pattern changed.

Commands:

- `cargo test -p camel-dsl --lib mcp::` — green
- `cargo test -p camel-lint rschema` — green
- `cargo test -p camel-cli --test lint_corpus` — green
- `cargo test -p camel-component-mcp --lib` — untouched runtime crate stays
  green
- `cargo xtask schema --check` — passes

Acceptance:

- All commands above exit 0
- Commit with footer `Bd: rc-38iiz`

- [x] Task 1.2
