# Tasks: mcpshape

## Task 1.1 — camel-dsl: input_schema object-ness at lowering + shape-test witnesses (bd rc-ap58)

Files:

- `crates/camel-dsl/src/mcp.rs` (modified — validation + tests)
- `docs/src/yaml-dsl/step-verbs.md` (modified — one-line table note)

Steps:

1. In the `tests` module of `crates/camel-dsl/src/mcp.rs`, add the
   following tests FIRST (after the existing
   `blank_name_still_rejected_at_lowering`), following the existing
   style (struct-literal arrange via `make_block()`, `lower_all_mcp_to_routes`
   act, `err.to_string()` message assertions). Verify each rejection
   test FAILS against current code (red) before implementing:
   - `input_schema_non_object_kinds_rejected_at_lowering` —
     arrange: `make_block()` with `tools = vec![RouteDslMcpTool { name:
     "lookup", input_schema: <kind> }]` for EACH kind in
     `[serde_json::json!("not an object"), json!([1, 2]), json!(7),
     json!(true), json!(null)]`; act: `lower_all_mcp_to_routes(&[block])`;
     assert: every kind returns `Err`, and the message contains the tool
     name `lookup`, the phrase `must be a JSON object`, and the kind word
     (`string`, `array`, `number`, `boolean`, `null` respectively).
   - `input_schema_rejection_is_lowering_owned_at_load` —
     arrange: a YAML document (`mcp:` block, server `crm` bind
     `127.0.0.1:9100`, tool `lookup` with `input_schema: "not an
     object"` as a plain YAML string); act 1: parse into
     `RouteDslRoutes` via `serde_yml::from_str`; assert 1: parse
     SUCCEEDS and the parsed `input_schema` equals
     `serde_json::Value::String` (verbatim load, mirroring the
     name-charset lowering-owned contract); act 2:
     `lower_all_mcp_to_routes` on the parsed blocks; assert 2: `Err`
     naming `lookup`.
   - `input_schema_empty_object_still_lowers` — arrange: `make_block()`
     with tool `lookup`, `input_schema: serde_json::json!({})`; act:
     lower; assert: `Ok`, exactly one route, `from` starts with
     `mcp:crm/tool/lookup?schema=` (the empty object is the boundary
     case: it IS an object and the consumer accepts it — design.md D2).
   - `slash_hash_percent_tool_names_rejected_at_lowering` — arrange:
     `make_block()` with one tool per name in `["a/b", "a#b", "a%b"]`
     (object schema); act: lower; assert: each name yields `Err` whose
     message contains the offending name and the phrase `is invalid`
     (witnesses for the mcpchars charset classes not yet covered by DSL
     unit tests; behavior already landed — design.md Context).
   - `server_and_resource_name_charset_rejected_at_lowering` —
     arrange A: `make_block()` with `server.name = "bad/name"` (valid
     tool); arrange B: `make_block()` with one resource
     `RouteDslMcpResource { name: "bad?name", uri: "crm://x" }`; act:
     lower each; assert: `Err` containing `server`/`resource` kind word
     respectively and the offending name.
2. Implement the validation (all tests above must turn green):
   - Add a private helper in `crates/camel-dsl/src/mcp.rs` next to
     `validate_mcp_name`, mirroring its documentation and error style
     (cite bd rc-ap58 and the consumer parity:
     `camel-component-mcp/src/endpoint.rs` applies
     `!input_schema.is_object()` at consumer start — design.md D2/D3):
     ```rust
     fn validate_tool_input_schema(
         tool_name: &str,
         schema: &serde_json::Value,
     ) -> Result<(), CamelError>
     ```
     returning `Ok(())` when `schema.is_object()`, else
     `Err(CamelError::RouteError(...))` with a message naming the tool
     and the actual JSON kind word (`string` / `array` / `number` /
     `boolean` / `null`), e.g. `mcp tool 'lookup' input_schema is
     invalid: must be a JSON object, got a JSON string — consumer start
     would reject it late (bd rc-ap58)`. NO stricter grammar: any JSON
     object passes (design.md D2 — zero drift with the consumer).
   - Call it in `lower_all_mcp_to_routes`'s tool loop, immediately
     after `validate_mcp_name("tool", &tool.name)?`.
   - Do NOT touch: the `input_schema` field type or its schemars
     attributes, the resource loop, `expand_mcp_into`, or anything in
     `crates/components/camel-component-mcp/` (defense-in-depth stays,
     design.md D5).
3. Docs note: in `docs/src/yaml-dsl/step-verbs.md`, the mcp block
   table row `| tools | list | no | Tool declarations: `name`,
   `input_schema` |` — extend the cell to state `input_schema` must be
   a JSON object (lowering rejects non-object shapes, bd rc-ap58).
   Prose only; no other doc file.
4. Run in this order from the worktree root and fix until clean:
   `cargo test -p camel-dsl --lib` (all green, no existing test
   modified except none expected), `cargo fmt --check --all`,
   `cargo clippy -p camel-dsl --all-features --all-targets -- -D
   warnings`.

Tests:

- Name/arrange/act/assert per test enumerated in step 1 — those ARE
  the executable test specs (they run under `cargo test -p camel-dsl
  --lib mcp::`).
- Red-first evidence: step 1 ends with a `cargo test -p camel-dsl
  --lib mcp` run where the five new rejection tests fail and the
  happy-path/`empty_object` behavior is as asserted; step 3 of the
  loop ends with the same command fully green.

Acceptance criteria:

- `cargo test -p camel-dsl --lib` green, including the five new tests
  and every pre-existing test (baseline count from main plus the new
  tests, none dropped).
- A non-object `input_schema` cannot reach route lowering output: the
  only failure point before consumer start is
  `lower_all_mcp_to_routes`, with an error naming the tool and JSON
  kind.
- Load stays permissive: non-object `input_schema` still deserializes
  (pinned by `input_schema_rejection_is_lowering_owned_at_load`).
- `schema --check` needs NO regen (no schemars attribute touched).
- `docs/src/yaml-dsl/step-verbs.md` table states the object-ness rule.

- [x] 1.1
