# Tasks: namebloat

## camel-lint

### Task 1.1: Schema-context machinery + walk rewire in document.rs

**Files:**
- `crates/camel-lint/src/document.rs` (modified)

**Steps:**
1. Add `static SCHEMA: LazyLock<serde_json::Value>` parsing `ROUTE_SCHEMA` once (replaces the inline parse inside today's `CONTAINER_KEYS` initializer; `CONTAINER_KEYS` re-derives from `&SCHEMA` and STAYS as the fallback set only).
2. Add `fn root_context(root: &Value) -> Vec<&'static serde_json::Value>`: mapping with `routes`/`rest`/`mcp` key → `vec![&SCHEMA]`; other mapping → `vec![&SCHEMA["$defs"]["RouteDslRoute"]]` (bare-route); sequence handled by the caller walking each item with the same bare-route candidates. Mirror of `rschema.rs` envelope detection (search for `envelope_depth` there).
3. Add `fn lookup_property<'a>(ctx: &[&'a serde_json::Value], key: &str) -> Option<&'a serde_json::Value>`: composition-aware search of `properties` through `anyOf`/`oneOf`/`allOf` and `$ref` (reuse/extend `resolve_ref`). Cycle guard = ACTIVE-CHAIN set of `$ref` strings pushed on entry and popped on exit, created FRESH per top-level call — a def reachable via two branches expands both times; only a ref already on the current chain is cut. First candidate that declares `key` wins.
4. Add `fn mapping_candidates<'a>(ctx: &[&'a serde_json::Value]) -> Vec<&'a serde_json::Value>`: expand ctx through `$ref`/composition, dropping branches that cannot validate a MAPPING instance (`type: "null"`, scalar-only, and array-only branches). Nullable-object properties arrive as `anyOf: [$ref, {type: "null"}]` (`response`, `error_handler`, `security_policy`) — the null branch must never be consulted for permissiveness or ctx, or it leaks "permitted" through `additionalProperties`-absent. EVERY consumer of ctx that interprets mapping keys (`lookup_property`, `key_permitted`, container recursion ctx, object-form URI ctx) runs on `mapping_candidates`, not raw ctx.
5. Add `fn key_permitted(ctx: &[&serde_json::Value], key: &str) -> bool`: true when any MAPPING-CAPABLE candidate (after `mapping_candidates` expansion) permits undeclared keys — `additionalProperties` absent, `true`, or a schema object. `additionalProperties: false` does not permit. (Without the mapping filter, `error_handler: {to: log:dl}` — a tolerated shape today — would become undeclared+permitted via its null branch and lose its endpoint.)
6. Add `fn is_free_form(node: &serde_json::Value) -> bool`: the resolved node is NOT array-typed and NOT scalar-only, and EVERY resolved object branch (per `mapping_candidates` semantics) is object-typed (a `"type": ["object", "null"]` LIST counts as object-typed — string-equality on `type` misses `security_policy.config`) with NO `properties` key of its own — i.e. an `additionalProperties`-keyed user map. Array subschemas (`RouteDslRoute.steps`) and composition nodes with declared branches are NOT free-form.
7. Tighten `fn is_container(node, root) -> bool`: return `false` when `is_free_form(resolved)` — free-form maps drop out of `CONTAINER_KEYS` derivation. All other classification branches unchanged.
 8. Rewire `walk`: add parameter `ctx: &[&serde_json::Value]`. Root call sites (`Document::parse`) compute `root_context(&root)`. Inside the mapping loop, keep the `parameters` skip, `mcp` skip, and first-wins `from` slot capture EXACTLY as today (they run before schema lookup). Then for each remaining key `k`:
    - `let declared = lookup_property(ctx, k)`:
      - `Some(ps)` + `URI_KEYS.contains(k)`: Mapping child → object-form recursion with child ctx = `mapping_candidates([ps])`; when that expansion is EMPTY (ps resolves to no mapping-capable branch — e.g. `from`'s `type: string`, the tolerated shape `from:\n  uri: direct:start\n`), pass the EMPTY ctx so the nested walk runs in legacy global-name mode (preserves today's capture); non-Mapping child → `emit_endpoints(child, &effective)` (unchanged). Only the Mapping case carries `&effective` inherited params and `OptionOrigin::ConfigParameters`, exactly as today's object-form `enrich` behavior.
      - `Some(ps)` + structured container (`is_container` true, non-free-form) → Branch recursion with child ctx = `mapping_candidates([ps])` (null/scalar branches dropped). When the child is a Sequence and the declared shape is an object, walk each item against the DECLARED ctx itself (shape tolerance — this is how `multicast:` direct-sequence works: `multicast` is DECLARED on MulticastStep, items get MulticastData ctx, and the nested `to` inside them is undeclared there (MulticastData is `additionalProperties: false`) so it resolves via the legacy fallback); when the declared shape is an array, item ctx comes from `items`.
      - `Some(ps)` + free-form (`is_free_form`) → OPAQUE LEAF: no recursion, no emission. This covers `headers` and `config` regardless of whether their `additionalProperties` is `true` or typed.
      - `Some(ps)` + scalar/other → ignore.
    - `None` + `key_permitted(ctx, k)` → permitted user data: when the permitting `additionalProperties` is a TYPED schema (object), the value dispatches on THAT schema (recursing only if it is a structured container; scalar-typed AP such as `config`'s `{type: string}` stays opaque — a string entry named `to` is data, not an endpoint); when `additionalProperties` is absent or `true` → opaque: ignore the key entirely (no global interpretation).
    - `None` + NOT permitted → legacy fallback: exactly today's behavior (`URI_KEYS.contains(k)` → emit; `CONTAINER_KEYS.contains(k)` → Branch recursion with `[]` ctx-for-children meaning "global-name mode"). For fallback recursion pass the empty ctx so children also resolve via fallback.
 9. Sequence arm of `walk`: derive item ctx from the current ctx's `items` subschemas when present; otherwise reuse the same ctx.
 10. `from` handling nuance: the from-slot capture and the fall-through URI emission for routes 2..N stay keyed on the `k == "from"` special case BEFORE declaration lookup (preserves envelope-with-stray-`from` tolerance), matching today's order.

**Tests:** (executable spec — name, arrange, act, assert)
- `root_context_envelope_vs_bare`: build `Value` via `serde_yaml`-free path — parse two sources with `cst::parse_document` (`routes:\n  - from: direct:a\n` and `from: direct:a\n`) → call `root_context` on each root value → assert the first returns the envelope (root) node and the second the `RouteDslRoute` def (compare against `SCHEMA["$defs"]["RouteDslRoute"]` by pointer equality). `command`: `cargo test -p camel-lint --lib document::tests::root_context_envelope_vs_bare`. `expected`: fails before implementation (functions absent), passes after.
- `lookup_property_finds_declared_to_through_step_anyof`: ctx = `[RouteDslStep]` → `lookup_property(ctx, "to")` resolves `ToStep.properties.to` (non-null `type: ["string","null"]` object). Also `lookup_property(ctx, "multicast")` resolves MulticastStep.multicast. `command`: `cargo test -p camel-lint --lib document::tests::lookup_property_finds_declared_to_through_step_anyof`. `expected`: fails before, passes after.
- `lookup_property_active_chain_allows_repeated_refs`: real recursive chain — `lookup_property([SCHEMA's DoTryData], "steps")` returns DoTryData's steps array (items ref `#/$defs/RouteDslStep`); then `lookup_property([RouteDslStep], "do_try")` returns DoTryStep's `do_try` ($ref DoTryData); then `lookup_property([DoTryData], "steps")` AGAIN still returns the steps subschema — the repeated `#/$defs/RouteDslStep`/`#/$defs/DoTryData` refs at different chain depths do not cut expansion. NOTE: RouteDslStep's branches do NOT declare `steps` directly — `lookup_property([RouteDslStep], "steps")` itself MUST return `None` (assert this too). `command`: `cargo test -p camel-lint --lib document::tests::lookup_property_active_chain_allows_repeated_refs`. `expected`: fails before, passes after.
- `key_permitted_at_permissive_root_key_permitted_at_strict_step`: `key_permitted([envelope root], "anything")` → true (root has no `additionalProperties`); `key_permitted([ToStep], "anything")` → false (`additionalProperties: false`); `key_permitted([error_handler's anyOf: [RouteDslErrorHandler, {type: null}]], "to")` → false (the null branch is dropped by `mapping_candidates`, the RouteDslErrorHandler branch rejects — so the tolerated `error_handler: {to: log:dl}` shape keeps its legacy fallback). `command`: `cargo test -p camel-lint --lib document::tests::key_permitted_at_permissive_root_key_permitted_at_strict_step`. `expected`: fails before, passes after.
- `is_free_form_type_list_and_typed_ap`: `is_free_form(headers-subschema)` → true (`type: object` + AP true, no properties); `is_free_form(config-subschema)` → true (`type: ["object","null"]` list counts as object-typed); `is_free_form(steps-subschema)` → false (array-typed); `is_free_form(RouteDslStep)` → false (composition with declared branches). `command`: `cargo test -p camel-lint --lib document::tests::is_free_form_type_list_and_typed_ap`. `expected`: fails before, passes after.
- `is_container_rejects_free_form_maps`: `is_container(headers-subschema, &SCHEMA)` → false; `is_container(steps-subschema, &SCHEMA)` → true. `command`: `cargo test -p camel-lint --lib document::tests::is_container_rejects_free_form_maps`. `expected`: fails before (headers classified container), passes after.

**Acceptance:**
- `cargo test -p camel-lint --lib` passes (all existing + new tests).
- `cargo clippy -p camel-lint -- -D warnings` exits 0.
- No `unwrap()` outside tests (existing `// allow-unwrap` markers preserved): `cargo xtask lint-unwrap` exits 0.

- [x] 1.1

### Task 1.2: Walk-level opacity + capture regression tests

**Files:**
- `crates/camel-lint/src/document.rs` (modified — tests module only)

**Steps:**
1. Add the following `#[cfg(test)]` tests to the existing `mod tests` in `document.rs`, after the existing walk-behavior tests. Each test parses a source, asserts on `doc.route_view.endpoints()`.
2. Run the full camel-lint suite; every pre-existing test must pass unmodified (endpoint-retention gate).

**Tests:** (executable spec)
- `rest_response_headers_named_uri_to_endpoints_are_opaque`: source = rest doc `rest:\n  - operations:\n      - method: GET\n        to: direct:ok\n        response:\n          headers:\n            uri: timer:foo?frequency=1s\n            to: log:out\n            endpoints:\n              - direct:a\n              - direct:b\n` → endpoints contain `direct:ok` ONLY; none of `timer:foo?frequency=1s`, `log:out`, `direct:a`, `direct:b` appears. `command`: `cargo test -p camel-lint --lib rest_response_headers_named_uri_to_endpoints_are_opaque`. `expected`: FAILS on current main (the FP bug), PASSES after Task 1.1.
- `security_policy_config_map_is_opaque` (retention pin): source = `from: direct:start\nsecurity_policy:\n  config:\n    to: log:leak\n` → endpoint set = `{direct:start}` only. NOTE: this passes on CURRENT main too — `config`'s `type: ["object", "null"]` list already misses today's string-equality `is_container` check, so opacity is currently incidental; the test PINS it against the rewrite (type-list-aware classification must not newly walk it). `command`: `cargo test -p camel-lint --lib security_policy_config_map_is_opaque`. `expected`: passes before AND after.
- `permissive_root_stray_keys_are_opaque`: source = `routes:\n  - from: direct:start\nresponse:\n  to: log:stray\n` (stray root-level `response:` — the permissive envelope accepts it) → endpoints = `{direct:start}` only. `command`: `cargo test -p camel-lint --lib permissive_root_stray_keys_are_opaque`. `expected`: fails before (global fallback walks it), passes after.
- `rest_operation_to_and_steps_still_captured`: source = rest doc with operation-level `to: timer:op` AND nested `steps:\n  - to: timer:nested` → endpoints contain both `timer:op` and `timer:nested` with distinct byte-exact spans slicing to their source text. `command`: `cargo test -p camel-lint --lib rest_operation_to_and_steps_still_captured`. `expected`: passes before AND after (retention).
- `nested_steps_to_captured_across_root_forms`: three sources — envelope (`routes:\n  - steps:\n      - to: log:nested\n`), bare (`steps:\n  - to: log:nested\n`), legacy array (`- steps:\n    - to: log:nested\n`) → each captures `log:nested` with span slicing exactly `log:nested`. `command`: `cargo test -p camel-lint --lib nested_steps_to_captured_across_root_forms`. `expected`: passes before AND after.
- `recursive_dotry_nested_to_captured`: source with `do_try:` inside `do_try:` (innermost `steps: [{to: log:deep}]`) → `log:deep` captured, span byte-exact. `command`: `cargo test -p camel-lint --lib recursive_dotry_nested_to_captured`. `expected`: passes before AND after.
- `multicast_direct_sequence_form_still_captured`: the existing `nested_child_step_uri_captured` fixture (`multicast:` with direct sequence) — do not modify it; add an explicit assertion sibling `multicast_direct_sequence_explicit` with the same shape asserting `log:nested` presence (documents the rejected-key fallback contract). `command`: `cargo test -p camel-lint --lib multicast_direct_sequence`. `expected`: passes before AND after.
- `object_form_from_uri_still_captured`: source = `from:\n    uri: direct:start\nsteps:\n  - to: log:out\n` → endpoints contain BOTH `direct:start` (nested `uri` under object-form `from`) and `log:out`. `command`: `cargo test -p camel-lint --lib object_form_from_uri_still_captured`. `expected`: passes before AND after (tolerated shape, no declared structure for it — legacy path must keep capturing).

**Acceptance:**
- `cargo test -p camel-lint --lib` green with ZERO modifications to pre-existing test functions.
- The two header/stray-root opacity tests (`rest_response_headers_named_uri_to_endpoints_are_opaque`, `permissive_root_stray_keys_are_opaque`) fail when the Task 1.1 diff is reverted (verified by the worker via `git stash` of document.rs non-test changes or equivalent local check) — demonstrating they pin the fix, not the fixture. (`security_policy_config_map_is_opaque` is a retention pin that passes before AND after.)

- [x] 1.2

### Task 1.3: Rule-level regression + cross-document isolation

**Files:**
- `crates/camel-lint/src/rules/ruriknown_tests.rs` (modified)

**Steps:**
1. Add rule-level tests using the file's existing `analyze`/`StubCatalog`/`meta_with_options` helpers and the `timer` catalog scheme with a `period: Duration` option (copy the catalog construction from `unknown_option_in_rest_operation_to`).
2. Cross-document isolation test runs two `analyze` calls in sequence on the same catalog.

**Tests:** (executable spec)
- `rest_response_header_uri_value_not_uri_linted` (bd rc-ni8qu repro): source = `rest:\n  - operations:\n      - method: GET\n        to: timer:foo?frequency=1s\n        response:\n          headers:\n            uri: timer:bar?frequency=1s\n` → exactly ONE `UnknownOption` diagnostic, its span slices to `frequency` in the OPERATION `to:` URI (the first `frequency` occurrence), and NO diagnostic references the header value (the second `timer:bar` / its `frequency`). Also with an empty catalog variant: zero `UnverifiedScheme` from the header value. `command`: `cargo test -p camel-lint --lib rest_response_header_uri_value_not_uri_linted`. `expected`: FAILS on current main (both frequency occurrences flagged), PASSES after.
- `rest_operation_to_validation_unchanged_after_fix`: the existing `unknown_option_in_rest_operation_to` must pass unmodified — do NOT edit it; this step only records that requirement (acceptance checks it by name). `command`: `cargo test -p camel-lint --lib unknown_option_in_rest_operation_to`. `expected`: passes before AND after.
- `poisoned_document_does_not_affect_clean_document_lint`: catalog with `timer`+`period`; source A (poisoned) = rest doc with `response.headers.uri/to/endpoints` URI-shaped values; source B (clean) = `from: direct:start\nsteps:\n  - to: timer:foo?frequency=1s\n`. Action: `analyze(A)` then `analyze(B)`. Assert: `analyze(B)` second-run diagnostics equal a fresh `analyze(B)` on a freshly built engine/catalog instance — identical count, codes, spans (compare via `format!("{:?}")` of the sorted diagnostic list). `command`: `cargo test -p camel-lint --lib poisoned_document_does_not_affect_clean_document_lint`. `expected`: passes before AND after (guards against per-document global state introduced by the fix).

**Acceptance:**
- `cargo test -p camel-lint --lib` green; existing `unknown_option_in_rest_operation_to` and `mcp_resource_uri_not_validated_as_endpoint` untouched.
- `cargo clippy -p camel-lint -- -D warnings` exits 0.

- [x] 1.3

### Task 1.4: CONTEXT.md walk section + mission gates

**Files:**
- `crates/camel-lint/CONTEXT.md` (modified)

**Steps:**
1. Update the walk/traversal description (the `LintRoute` section around line 163 that documents "Used by R-URI-known, R-SECRET, and R-DEPRECATED to walk route structure") to describe schema-CONTEXT-scoped traversal: declared keys dispatch on their own subschema; free-form maps (`headers`, `config`, `parameters`) are opaque leaves; undeclared-but-permitted keys are opaque; only schema-rejected keys use the legacy global-name fallback. Cite bd rc-ni8qu.
2. Run the full mission gate set from the worktree root and record exit codes.

**Tests:** (executable spec)
- `context_md_documents_context_scoped_walk`: grep-check — `CONTEXT.md` contains "opaque" and "rc-ni8qu" within the walk/LintRoute section. `command`: `grep -c "opaque" crates/camel-lint/CONTEXT.md`. `expected`: ≥1 after the edit.
- Gate checks (all from worktree root, all must exit 0):
  - `cargo fmt --check --all`
  - `cargo clippy -p camel-lint -- -D warnings`
  - `cargo test -p camel-lint`
  - `cargo test -p camel-cli --test lint_corpus`
- Corpus baseline: `crates/camel-cli/tests/fixtures/lint-corpus-baseline.ron` MUST remain byte-identical (`git diff --stat` shows no change to it). If the corpus test fails, STOP and report — do NOT regenerate the baseline (a baseline change means a legitimate diagnostic moved, which is a regression signal for this change).

**Acceptance:**
- All four gate commands exit 0.
- `git status --short` shows no modification to `crates/camel-cli/tests/fixtures/lint-corpus-baseline.ron`.
- `cargo xtask lint-unwrap` exits 0.

- [x] 1.4
