# Tasks: mcpblank

## Task 1.1 — camel-dsl: blank-reject deserializer + schema-mirror attrs on mcp name/bind

Files:
- `crates/camel-dsl/src/mcp.rs` (modified)

Steps:
1. In `crates/camel-dsl/src/mcp.rs`, next to the existing `non_empty_path`
   helper, add:
   `fn non_blank_verbatim<'de, D>(deserializer: D, field: &'static str) -> Result<String, D::Error>`
   — deserialize a `String`, and when `value.trim().is_empty()` return
   `Err(serde::de::Error::custom(format!("{field} must not be empty")))`;
   otherwise return the ORIGINAL (untrimmed) `String`. Do NOT trim the
   returned value (design.md D1: runtime uses the raw string for both
   `SocketAddr` parse and the `validate_mcp_name` charset check).
2. Add thin wrappers mirroring `deserialize_cert_path`/`deserialize_key_path`:
   `fn deserialize_mcp_name<'de, D>(d: D) -> Result<String, D::Error>` calling
   `non_blank_verbatim(d, "name")`, and
   `fn deserialize_mcp_bind<'de, D>(d: D) -> Result<String, D::Error>` calling
   `non_blank_verbatim(d, "bind")`.
3. Annotate four fields with BOTH attributes (copy the TLS-field shape
   verbatim):
   - `RouteDslMcpServer.name`: `#[serde(deserialize_with = "deserialize_mcp_name")]` + `#[cfg_attr(feature = "schema", schemars(regex(pattern = r"\S")))]`
   - `RouteDslMcpServer.bind`: `#[serde(deserialize_with = "deserialize_mcp_bind")]` + the same `schemars(regex(pattern = r"\S"))` attr
   - `RouteDslMcpTool.name` and `RouteDslMcpResource.name`: the `deserialize_mcp_name` pair (sweep — same blank class).
4. Doc-comment the helper: one line stating it rejects blank-after-trim values
   but returns the raw string, unlike `non_empty_path`, because the runtime
   consumers of `bind`/`name` use the raw value.

Tests (in the `mcp.rs` tests module, mirroring existing YAML-parse tests):
- `blank_server_name_rejected_at_load`
  - setup: minimal `mcp:` YAML doc with `server.name: ""` and a valid `bind`
  - action: parse the doc into the DSL AST
  - assert: parse errors; error text contains `name must not be empty`
  - command: `cargo test -p camel-dsl --lib mcp::`
  - expected: FAILS before step 1-3 (doc parses today — this is the
    reproduction evidence), passes after
- `blank_server_bind_whitespace_rejected_at_load`
  - setup: doc with `server.bind: "   "` and a valid `name`
  - action: parse
  - assert: error text contains `bind must not be empty`
  - expected: fails before, passes after
- `blank_tool_and_resource_names_rejected_at_load`
  - setup: valid server, one tool with `name: ""`, one resource with
    `name: "  "`
  - action: parse
  - assert: error text contains `name must not be empty`
  - expected: fails before, passes after
- `non_blank_padded_values_load_verbatim`
  - setup: doc with `server.name: " crm "` and `server.bind: " 127.0.0.1:9100 "`
  - action: parse
  - assert: parse SUCCEEDS and `server.name == " crm "` and
    `server.bind == " 127.0.0.1:9100 "` (raw preserved — the padded values
    keep their pre-change runtime fate: charset/`SocketAddr` rejection)
  - expected: passes before AND after (runtime behavior unchanged)
- `blank_name_still_rejected_at_lowering`
  - setup: construct `RouteDslMcp` with `server.name: ""` directly (bypassing
    serde), plus a minimal valid tool
  - action: call `lower_all_mcp_to_routes`
  - assert: `Err` whose text mentions `name` and `invalid`
  - expected: passes before AND after (runtime lowering untouched)

Acceptance:
- `cargo test -p camel-dsl --lib` passes (existing suite included — no
  behavior regression on valid documents)
- `cargo fmt --check` and `cargo clippy -p camel-dsl -- -D warnings` exit 0
- Report the exact lowering error text observed for the blank-name
  `lower_all_mcp_to_routes` call (evidence for bd rc-sghtz)

- [x] Task 1.1

## Task 1.2 — regenerate route-schema copies + refresh stale pattern comment

Files:
- `schemas/dsl/route-schema.json` (modified, generated)
- `crates/camel-lint/schema/route-schema.json` (modified, generated)
- `crates/camel-lint/src/rules/rschema.rs` (modified, comment only)

Steps:
1. Run `cargo xtask schema` (from the worktree root). Verify both copies now
   carry a `"pattern": "\\S"` on the four fields (`RouteDslMcpServer.name`,
   `RouteDslMcpServer.bind`, `RouteDslMcpTool.name`, `RouteDslMcpResource.name`
   definitions).
2. In `crates/camel-lint/src/rules/rschema.rs`, update the PATTERN de-collapse
   comment that currently reads "the schema's only pattern keywords are the
   two MCP TLS path fields (rc-n3t73)" to also name the four MCP name/bind
   fields (rc-sghtz). Comment-only change; no code edits.

Tests:
- `cargo xtask schema --check`
  - setup: regenerated copies from step 1
  - action: run the check
  - assert: exits 0 (copies in sync)
  - expected: fails before step 1 (copies lack the new patterns), passes after

Acceptance:
- `cargo xtask schema --check` exits 0
- `grep` shows no remaining "only pattern keywords" phrasing that excludes the
  name/bind fields

- [x] Task 1.2

## Task 1.3 — R-SCHEMA blank tests + lint-corpus fixture + baseline entry

Files:
- `crates/camel-lint/src/rules/rschema/tests.rs` (modified)
- `crates/camel-cli/tests/fixtures/lint-corpus/mcp-blank-bind-name.yaml` (new)
- `crates/camel-cli/tests/fixtures/lint-corpus-baseline.ron` (modified)

Steps:
1. In `rschema/tests.rs`, mirroring the rc-n3t73 blank-TLS pins (see
   `rschema_mcp_tls_blank_cert_path_empty_rejected` for the harness pattern),
   add:
   - `rschema_mcp_blank_bind_empty_rejected` — doc with `server.bind: ""`,
     valid `name`: exactly one R-SCHEMA Error, anchored on the blank `bind`
     value (slice the anchored raw text and assert it trims to empty)
   - `rschema_mcp_blank_name_whitespace_rejected` — `server.name: "   "`,
     valid `bind`: exactly one Error anchored on the blank `name` value
   - `rschema_mcp_blank_tool_name_rejected` — valid server, tool
     `name: ""`: one Error anchored on the tool `name` value
   - `rschema_mcp_blank_values_keep_nonblank_clean` — valid `name`+`bind`
     (e.g. `crm` / `127.0.0.1:9100`): zero R-SCHEMA diagnostics (non-blank
     stays out of the blank class; deeper bind/charset validation is
     runtime-owned)
2. New corpus fixture `mcp-blank-bind-name.yaml` — negative witness whose
   header comment states the contract (mirror `mcp-tls-blank-paths.yaml`
   phrasing, cite rc-sghtz): body carries `server.name: "   "` and
   `server.bind: ""` (both blank classes in one file).
3. Add the fixture's expected R-SCHEMA Error entries (two: one per blank
   field) to `lint-corpus-baseline.ron` in the same format as the
   `mcp-tls-blank-paths.yaml` entry.

Tests:
- `cargo test -p camel-lint rschema` — new pins pass; full rschema set green
- `cargo test -p camel-cli --test lint_corpus` — exact-match gate passes in
  BOTH directions (every emitted diagnostic baselined; every baseline
  diagnostic emitted)
- `cargo test -p camel-component-mcp --lib` — untouched runtime crate stays
  green (runtime behavior unchanged)

Acceptance:
- All three commands above exit 0
- Fixture header cites rc-sghtz and states no other rule fires on the
  mcp-only document (R-URI skips the mcp subtree)

- [x] Task 1.3
