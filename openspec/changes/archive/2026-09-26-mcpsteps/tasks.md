# Tasks: mcpsteps

## Task 1.1 — camel-dsl: opt-in `to`/`steps` on mcp tool/resource + lowering + tests (bd rc-23y2)

- [x] 1.1

Files:

- `crates/camel-dsl/src/mcp.rs` (modified — structs, lowering, tests)

Steps:

1. In the `tests` module of `crates/camel-dsl/src/mcp.rs`, add the tests
   listed under Tests FIRST, following the existing style (arrange via the
   existing block-builder helpers or struct literals, act via
   `lower_all_mcp_to_routes`, assert on `err.to_string()` / route fields).
   Verify each new test fails against current code (red) before
   implementing. Note: field-addition tests are red as compile errors
   until the struct fields exist — add the fields first as
   `#[serde(default)]` no-ops if needed to get meaningful red, then keep
   lowering unchanged until the tests exist.
2. Add to `RouteDslMcpTool` and `RouteDslMcpResource`
   (`crates/camel-dsl/src/mcp.rs`):
   - `#[serde(default)] pub to: Option<String>` — single endpoint URI
     shorthand (doc comment: one line, non-verbose, mirror rest's wording).
   - `#[serde(default)] pub steps: Vec<RouteDslStep>` — standard step list.
   Extend the `crate::route_ast` import in mcp.rs with `RouteDslStep` and
   `ToStep`; the tests module additionally imports `SetHeaderStep` and
   `SetHeaderData`.
   `deny_unknown_fields` stays; no other field changes. Pre-existing
   struct literals of the two structs (in the tests module) gain exactly
   `to: None, steps: Vec::new()` — nothing else in them changes.
3. Add a private helper next to `validate_mcp_name`:
   `fn effective_steps(kind: &str, name: &str, to: &Option<String>, steps: &[RouteDslStep]) -> Result<Vec<RouteDslStep>, CamelError>`
   which matches on `(to.as_ref(), steps.is_empty())`:
   - `(Some(_), false)` → `Err(CamelError::RouteError)` with message
     `mcp {kind} '{name}' cannot have both 'to' and 'steps'`.
   - `(Some(to), true)` → `Ok(vec![RouteDslStep::To(ToStep { to: to.clone(), parameters: BTreeMap::new() })])`.
   - `(None, _)` → `Ok(steps.to_vec())` (empty when neither is declared —
     the identity default).
   No `unwrap`/`expect` anywhere in the helper. This mirrors the
   construction in `rest.rs` `lower_operation` (its "User steps" block) —
   do not fork or wrap the steps.
4. Change `consumer_route(id: &str, from: String, security_policy: Option<RouteDslSecurityPolicy>)`
   to take a fourth parameter `steps: Vec<RouteDslStep>` and assign it to
   the route's `steps` field. Update both call sites in
   `lower_all_mcp_to_routes` (tool loop, resource loop) to pass
   `effective_steps("tool", &tool.name, &tool.to, &tool.steps)?` /
   `effective_steps("resource", &resource.name, &resource.to, &resource.steps)?`.
5. Run `cargo test -p camel-dsl --lib` — all tests green; pre-existing
   test ASSERTIONS are byte-unchanged (the only edits inside pre-existing
   tests are struct literals gaining `to: None, steps: Vec::new()`).

Tests:

- `tool_with_steps_lowers_to_pipeline`
  - setup: an MCP block with server `crm`, tool `lookup` (object
    `input_schema`).
  - action: set the tool's `steps` to
    `vec![RouteDslStep::To(ToStep { to: "log:audit".into(), parameters: BTreeMap::new() })]`
    and call `lower_all_mcp_to_routes(&[block])`.
  - assert: one route with id `mcp-crm-tool-lookup`, `from` starting with
    `mcp:crm/tool/lookup?schema=`, and `steps` equal to the declared
    single `To` step (length 1, target `log:audit`).
  - command: `cargo test -p camel-dsl --lib tool_with_steps_lowers_to_pipeline`
  - expected: red before implementation, green after.
- `tool_with_to_shorthand_lowers_single_to_step`
  - setup: same block; tool `to = Some("direct:out".into())`, no steps.
  - action: `lower_all_mcp_to_routes(&[block])`.
  - assert: route steps are exactly one `To` step targeting `direct:out`
    with empty `parameters`.
  - command: `cargo test -p camel-dsl --lib tool_with_to_shorthand_lowers_single_to_step`
  - expected: red before, green after.
- `resource_with_to_shorthand_lowers_single_to_step`
  - setup: block with resource `customers` (`uri: crm://customers`),
    `to = Some("direct:mirror".into())`.
  - action: `lower_all_mcp_to_routes(&[block])`.
  - assert: route id `mcp-crm-resource-customers`, steps exactly one `To`
    step targeting `direct:mirror`.
  - command: `cargo test -p camel-dsl --lib resource_with_to_shorthand_lowers_single_to_step`
  - expected: red before, green after.
- `resource_with_steps_lowers_to_pipeline`
  - setup: block with resource `customers` whose `steps` is a single
    `RouteDslStep::SetHeader(SetHeaderStep { set_header: SetHeaderData { key: "X-Source".into(), value: Some(serde_json::json!("mcp")), ..Default::default() } })`
    (only if `SetHeaderData` derives `Default`; otherwise populate the
    remaining `Option` fields with `None` explicitly — every field except
    `key` is optional).
  - action: `lower_all_mcp_to_routes(&[block])`.
  - assert: the resource route's steps equal the declared list verbatim.
  - command: `cargo test -p camel-dsl --lib resource_with_steps_lowers_to_pipeline`
  - expected: red before, green after.
- `identity_default_steps_empty_without_to_or_steps`
  - setup: block with one tool and one resource, neither `to` nor
    `steps`.
  - action: `lower_all_mcp_to_routes(&[block])`.
  - assert: both routes have empty `steps`, and their `from`/`id` values
    equal the exact strings the pre-change lowering produced (pin the
    full from-URI strings in the assertion).
  - command: `cargo test -p camel-dsl --lib identity_default_steps_empty_without_to_or_steps`
  - expected: red before, green after.
- `tool_to_and_steps_both_rejected_at_lowering`
  - setup: tool `lookup` with `to = Some("direct:out".into())` AND a
    non-empty `steps` vec.
  - action: `lower_all_mcp_to_routes(&[block])`.
  - assert: `Err` whose message contains `cannot have both 'to' and 'steps'`
    and the tool name `lookup`.
  - command: `cargo test -p camel-dsl --lib tool_to_and_steps_both_rejected_at_lowering`
  - expected: red before, green after.
- `resource_to_and_steps_both_rejected_at_lowering`
  - setup: resource `customers` with both `to` and a non-empty `steps`.
  - action: `lower_all_mcp_to_routes(&[block])`.
  - assert: `Err` whose message contains `cannot have both 'to' and 'steps'`
    and the resource name `customers`.
  - command: `cargo test -p camel-dsl --lib resource_to_and_steps_both_rejected_at_lowering`
  - expected: red before, green after.
- `invalid_step_shape_rejected_at_document_load`
  - setup: a YAML `mcp:` block whose tool declares
    `steps: [{ bogus_step: {} }]` (no `RouteDslStep` variant matches).
  - action: `serde_yml::from_str::<RouteDslRoutes>(yaml)` (the module's
    existing YAML-parse test path).
  - assert: `Err` — malformed step shapes are load-owned (the shared
    `RouteDslStep` untagged serde rejects them with the parser's
    location-carrying error); do not pin the exact serde message text,
    rejection is the contract.
  - command: `cargo test -p camel-dsl --lib invalid_step_shape_rejected_at_document_load`
  - expected: green before AND after implementation (witness test: the
    shared machinery already owns this rejection — documents the boundary,
    guards against a future bypass).

Acceptance:

- `cargo test -p camel-dsl --lib` exits 0, with exactly 8 new tests on
  top of the pre-change count (capture the count from the first red run;
  static baseline is 842 `#[test]` + 1 `#[tokio::test]`).
- `git diff crates/camel-dsl/src/mcp.rs` shows no pre-existing test
  assertion modified — only struct literals gaining
  `to: None, steps: Vec::new()`.
- `cargo clippy -p camel-dsl --all-targets -- -D warnings` exits 0.
- `cargo fmt --check -p camel-dsl` exits 0.

## Task 1.2 — schema regen + docs entry for the passthrough option (bd rc-23y2)

- [x] 1.2

Files:

- `schemas/dsl/route-schema.json` (modified — regenerated)
- `schemas/ts/RouteDslMcpTool.ts` (modified — regenerated)
- `schemas/ts/RouteDslMcpResource.ts` (modified — regenerated)
- `crates/camel-lint/schema/route-schema.json` (modified — regenerated copy)
- `crates/camel-dsl/README.md` (modified — MCP DSL section)
- `docs/src/components/mcp.md` (modified — Server (Consumer) section)

Steps:

1. Run `cargo xtask schema` in the worktree; it regenerates
   `schemas/dsl/route-schema.json`, the TS type exports under
   `schemas/ts/` (including `RouteDslMcpTool.ts` and
   `RouteDslMcpResource.ts`), and the
   `crates/camel-lint/schema/route-schema.json` copy. Inspect the diff:
   the `RouteDslMcpTool` and `RouteDslMcpResource` definitions gain
   optional `to` and `steps` (JSON Schema and TS alike); nothing else
   changes structurally (schemars may reorder keys — accept its
   canonical output).
2. Verify with `cargo xtask schema --check` (exit 0).
3. In `crates/camel-dsl/README.md` MCP DSL section: replace the sentence
   `The block is a step-less catalog declaration; tool and resource
   behavior lives in explicit routes that consume from the lowered mcp:
   URIs.` with a two-paragraph description: declarations without
   `to`/`steps` keep the identity pipeline (a `tools/call` returns its
   own arguments — v1 behavior), and an opt-in `to` or `steps` (not both)
   lowers onto the consumer route through the same step machinery the
   route and rest surfaces use. Add a short YAML example extending the
   existing `lookup` tool with `to: log:audit`.
4. In `docs/src/components/mcp.md`, add a `### Tool and resource behavior
   (steps/to passthrough)` subsection under `## Server (Consumer)`: same
   content as the README note (identity default; opt-in `to` XOR `steps`;
   both rejected at lowering with a named error).

Tests:

- `schema-contains-optional-passthrough-fields`
  - setup: regenerated schema files from step 1.
  - action: `python3 -c` reading both `route-schema.json` copies, locating
    the `RouteDslMcpTool` and `RouteDslMcpResource` definitions.
  - assert: each definition contains a `to` property (nullable/optional)
    and a `steps` property; both copies agree.
  - command: manual verification step (no persistent test file — the
    canonical guard is `cargo xtask schema --check`, which fails if the
    committed schema drifts from the derive).
  - expected: both definitions carry the fields; `--check` green.

Acceptance:

- `cargo xtask schema --check` exits 0.
- Both `route-schema.json` copies list `to` and `steps` under
  `RouteDslMcpTool` and `RouteDslMcpResource`; the TS types
  `schemas/ts/RouteDslMcpTool.ts` and `schemas/ts/RouteDslMcpResource.ts`
  carry the same two fields.
- `crates/camel-dsl/README.md` no longer contains the phrase
  `step-less catalog declaration`.
- `docs/src/components/mcp.md` contains the passthrough subsection under
  Server (Consumer).
