# Tasks: open-uri-param-namespace

## Execution order and dependencies

This is a single-phase change (no `## Phase N` headings). Tasks MUST be executed in
this order — each task consumes symbols or artifacts produced by an earlier task:

1. **Task 1.1** (camel-api contract) — no dependencies. Introduces `UriOptionMatch`,
   `UriOption.pattern`, `UriOption::pattern_prefix`.
2. **Task 2.1** (macro parser + guardrails + codegen) — depends on **Task 1.1**
   (consumes `UriOption::pattern_prefix`, `UriOptionMatch::Prefix`).
3. **Task 2.2** (trybuild UI tests) — depends on **Task 2.1** (the guardrails are
   the source of truth for the error text the snapshots capture). MUST dispatch
   strictly after 2.1 has landed.
4. **Task 3.1** (lint `resolve_option`) — depends on **Task 1.1** (consumes
   `UriOptionMatch::Prefix`).
5. **Task 4.1** (docs) — depends on **Tasks 1.1, 2.1, 3.1** (documents the surfaces
   they introduce).
6. **Task 5.1** (schema regen) — depends on **Task 1.1** (the contract change is
   what the schema regen captures).

Each task block below restates its immediate prerequisite where relevant, so an
isolated worker dispatch remains safe.

## Change-level verification (run before declaring PHASE 3 complete)

After all six tasks land, the conductor MUST run these workspace-wide gates in
addition to each task's per-crate acceptance. Any failure loops back to the
owning task:

- `cargo build --workspace` exits 0.
- `cargo test --workspace --lib` exits 0.
- `cargo fmt --check --all` exits 0.
- `cargo clippy --workspace --all-features --exclude camel-cli --exclude camel-component-kafka --exclude security-keycloak --exclude security-wasm-policy -- -D warnings` exits 0 (per the AGENTS.md clippy matrix; the excluded crates have their own per-crate clippy invocations if touched — none are touched by this change).
- `cargo clippy -p camel-component-kafka --all-targets -- -D warnings` exits 0 (not touched, but the matrix runs it).
- `cargo clippy -p camel-cli -- -D warnings` exits 0.
- `cargo test -p camel-cli --test lint_corpus` passes with unchanged baseline.
- `cargo xtask schema --check` exits 0.
- `cargo xtask lint-non-exhaustive` exits 0.
- `cargo xtask lint-context-citations` exits 0.
- `git diff --stat crates/components/camel-xj/ crates/components/camel-xslt/` is empty (Change 1 must not touch Change 2's components).

## camel-api

### Task 1.1: Add UriOptionMatch enum + UriOption.pattern field + pattern_prefix builder

**Files:**
- `crates/camel-api/src/component_metadata.rs` (modified)

**Steps:**
1. Add a new `#[non_exhaustive]` enum `UriOptionMatch` directly below the existing
   `OptionKind` enum (around `component_metadata.rs:18-26`). The enum uses Rust's
   default externally-tagged serde representation with `#[serde(rename_all = "snake_case")]`
   on both the enum and on the inner struct. Initial body:

   ```rust
   #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
   #[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
   #[serde(rename_all = "snake_case")]
   #[non_exhaustive]
   pub enum UriOptionMatch {
       Prefix {
           #[cfg_attr(feature = "schema", schemars(default))]
           separator: String,
       },
   }
   ```

2. Add a new optional field `pattern: Option<UriOptionMatch>` to the `UriOption`
   struct (currently at `component_metadata.rs:35-51`), placed immediately after the
   `secret` field. Annotations:

   ```rust
   #[cfg_attr(feature = "schema", schemars(default))]
   #[serde(default, skip_serializing_if = "Option::is_none")]
   pub pattern: Option<UriOptionMatch>,
   ```

3. Extend the `UriOption::new` constructor (around `component_metadata.rs`) to
   initialize `pattern: None`.
4. Add a consuming builder `pattern_prefix` immediately after the existing `secret()`
   builder in the `impl UriOption` block:

   ```rust
   #[must_use]
   pub fn pattern_prefix(mut self, separator: &str) -> Self {
       self.pattern = Some(UriOptionMatch::Prefix {
           separator: separator.to_string(),
       });
       self
   }
   ```

5. Add `UriOptionMatch` to the public export surface. The camel-api crate has no
   `pub use` re-export for `UriOption`/`OptionKind` — they are simply `pub` inside
   `pub mod component_metadata`. The `pub enum UriOptionMatch` declaration in step 1
   is sufficient; **no separate export-list edit is needed**.

**Tests:** (all in `crates/camel-api/src/component_metadata.rs` under `#[cfg(test)] mod tests`)
- `pattern_prefix_sets_prefix_variant`:
  - **name:** `pattern_prefix_sets_prefix_variant`
  - **setup:** an empty `UriOption::new("param", "desc", OptionKind::String)` with no builder calls yet.
  - **action:** call `.pattern_prefix("param.")` on it.
  - **assert:** `option.pattern == Some(UriOptionMatch::Prefix { separator: "param.".to_string() })` AND original fields (`name`, `description`, `kind`, `secret`, `required`, `default_value`, `aliases`, `deprecated`) are unchanged.
  - **command:** `cargo test -p camel-api --lib pattern_prefix_sets_prefix_variant`
  - **expected:** before implementation, the test fails to compile (`pattern_prefix` method missing); after implementation, passes.
- `pattern_defaults_to_none`:
  - **name:** `pattern_defaults_to_none`
  - **setup:** `UriOption::new("foo", "desc", OptionKind::String)` with no builder call.
  - **action:** read `option.pattern`.
  - **assert:** it is `None`.
  - **command:** `cargo test -p camel-api --lib pattern_defaults_to_none`
  - **expected:** before implementation fails to compile (field missing); after implementation passes.
- `serialize_pattern_none_omits_field`:
  - **name:** `serialize_pattern_none_omits_field`
  - **setup:** `UriOption::new("foo", "desc", OptionKind::String)`; a fixture string built by `serde_json::to_string` of the pre-change struct shape.
  - **action:** `serde_json::to_string(&option)`.
  - **assert:** the serialized string does NOT contain the substring `"pattern"`; the bytes equal the fixture string.
  - **command:** `cargo test -p camel-api --lib serialize_pattern_none_omits_field`
  - **expected:** before implementation passes trivially (no `pattern` field exists); after implementation continues to pass (proves byte-identical legacy output).
- `serialize_pattern_some_emits_externally_tagged_snake_case`:
  - **name:** `serialize_pattern_some_emits_externally_tagged_snake_case`
  - **setup:** `UriOption::new("param", "desc", OptionKind::String).pattern_prefix("param.")`.
  - **action:** `serde_json::to_string(&option)`.
  - **assert:** the serialized string CONTAINS the exact substring `"pattern":{"prefix":{"separator":"param."}}`.
  - **command:** `cargo test -p camel-api --lib serialize_pattern_some_emits_externally_tagged_snake_case`
  - **expected:** before implementation fails to compile; after implementation passes.
- `deserialize_pattern_some_roundtrips`:
  - **name:** `deserialize_pattern_some_roundtrips`
  - **setup:** the JSON string produced by the previous test.
  - **action:** `serde_json::from_str::<UriOption>(...)`.
  - **assert:** `result.pattern == Some(UriOptionMatch::Prefix { separator: "param.".to_string() })`.
  - **command:** `cargo test -p camel-api --lib deserialize_pattern_some_roundtrips`
  - **expected:** before implementation fails to compile; after implementation passes.

**Acceptance:**
- `cargo build -p camel-api --all-features` exits 0.
- `cargo test -p camel-api --lib` passes (all existing tests still green; new tests pass).
- `cargo clippy -p camel-api -- -D warnings` exits 0.
- `cargo xtask lint-non-exhaustive` exits 0 (the new enum carries `#[non_exhaustive]`).

- [x] 1.1

## camel-endpoint-macros

### Task 2.1: Extend UriParamAttr with pattern key + guardrails + codegen emission

**Depends on:** Task 1.1 (consumes `UriOption::pattern_prefix`, `UriOptionMatch::Prefix`).

**Files:**
- `crates/camel-endpoint-macros/src/uri_config.rs` (modified)
- `crates/camel-endpoint/tests/endpoint_macros_derive_integration_test.rs` (modified)

**Steps:**
1. In `crates/camel-endpoint-macros/src/uri_config.rs`, locate the `UriParamAttr`
   struct (around line 8) and add a new field `pattern: Option<String>` initialized
   to `None` in its `Default` impl (or in every constructor site — match the existing
   pattern used for `secret`/`required`/`aliases`/etc.).
2. Extend the `#[uri_param]` key parser (around lines 100-130 where unknown keys
   are rejected with `"unknown attribute key"`) to accept `pattern = "..."`. Parse
   the value as a string literal (same shape as the existing `desc = "..."` parser).
3. Add a field-type check: in the macro's per-field processing loop (around lines
   850-870 where `#[uri_param]` presence is detected), when `attr.pattern.is_some()`,
   verify the field type is exactly `Vec<(String, String)>`. Type-check by inspecting
   the `syn::Type` of the field — accept only `Vec<(String, String)>` (the canonical
   form). On mismatch, emit a spanned compile error:
   `"`pattern` is only valid on fields of type `Vec<(String, String)>`"
   pointing at the field's span.
4. Add the eight guardrail compile-error checks in the same per-field processing site,
   each emitting a spanned error on the field's span when triggered:
   - `pattern` + `required` → `"#[uri_param] cannot have both `pattern` and `required`; an open namespace cannot require a single key"`
   - `pattern` + `default` → `"#[uri_param] cannot have both `pattern` and `default`; an open namespace has no default value"`
   - `pattern` + `secret` → `"#[uri_param] cannot have both `pattern` and `secret`; an open namespace has no single secret value"`
   - `pattern` + `name` (explicit override) → `"#[uri_param] cannot have both `pattern` and `name`; the name is derived from the separator"`
   - `pattern` + `aliases` → `"#[uri_param] cannot have both `pattern` and `aliases`; a namespace matches by prefix, not by exact alias"`
   - `pattern` + non-`string` `kind` (i.e. `kind` is set and is not `"string"`) → `"#[uri_param] `kind` on a pattern field must be `string` or omitted"`
   - `pattern = ""` → `"#[uri_param] `pattern` separator must be non-empty"`
   - `pattern` whose value does not end with `.` → `"#[uri_param] `pattern` separator must end with `.` (the only permitted separator shape in this version)"`
   The "secret + default" check at `uri_config.rs:732-738` is the precedent for the
   shape of these errors; mirror it.
5. In `build_uri_option_entry` (around line 725-756, where each `UriOption::new(...)`
   call is constructed), when `attr.pattern.is_some()`, modify the generated tokens:
   - Compute `name` as the separator with the trailing `.` removed (the compile-time
     guardrail guarantees the trailing `.` exists).
   - Force `kind` to `OptionKind::String` (override any inference; the guardrails
     already rejected non-`string` `kind` values).
   - Append `.pattern_prefix(<separator_literal>)` to the builder chain, where
     `<separator_literal>` is the parsed separator string emitted as a `&str` literal.

**Tests:**
- `pattern_field_produces_namespace_option` (integration test in `endpoint_macros_derive_integration_test.rs`): setup — derive a `#[derive(UriConfig)]` struct with `#[uri_scheme = "x"]`, one path field, and one `#[uri_param(pattern = "param.")] params: Vec<(String, String)>` field; action — call `TestStruct::uri_options()`; assert — result has one entry, `entry.name == "param"`, `entry.kind == OptionKind::String`, `entry.pattern == Some(UriOptionMatch::Prefix { separator: "param.".to_string() })`.
- `pattern_field_with_other_scalar_params_coexist` (integration test): setup — derive a struct with `#[uri_param] scalar: String`, `#[uri_param(pattern = "param.")] params: Vec<(String, String)>`, and a path field; action — call `uri_options()`; assert — two entries, one with `name == "scalar"` and `pattern == None`, one with `name == "param"` and `pattern.is_some()`.

**Acceptance:**
- `cargo build -p camel-endpoint-macros` exits 0.
- `cargo test -p camel-endpoint --lib` passes (existing tests still green; new integration tests pass).
- `cargo test -p camel-endpoint --test endpoint_macros_derive_integration_test` passes.
- `cargo clippy -p camel-endpoint-macros -- -D warnings` exits 0.
- `cargo fmt --check --all` exits 0.

- [x] 2.1

### Task 2.2: Add trybuild UI compile-fail tests for all pattern guardrails

**Depends on:** Task 2.1 (the macro guardrails are the source of truth for the
diagnostic text the snapshots capture). This task MUST dispatch strictly after 2.1
has landed.

**Toolchain note:** `TRYBUILD=overwrite` captures rustc-rendered diagnostics. Snapshot
drift can result from using a different Rust toolchain than the repo's pinned one,
NOT necessarily from a macro defect. The worker MUST generate and verify snapshots
using the repo's pinned toolchain (run `cargo --version` and confirm it matches CI).
On drift, compare the diagnostic message and span under that toolchain before
changing macro behavior.

**Files:**
- `crates/camel-endpoint/tests/ui/pattern_on_non_vec_field_fail.rs` (new)
- `crates/camel-endpoint/tests/ui/pattern_on_non_vec_field_fail.stderr` (new)
- `crates/camel-endpoint/tests/ui/pattern_on_vec_wrong_inner_fail.rs` (new)
- `crates/camel-endpoint/tests/ui/pattern_on_vec_wrong_inner_fail.stderr` (new)
- `crates/camel-endpoint/tests/ui/pattern_with_required_fail.rs` (new)
- `crates/camel-endpoint/tests/ui/pattern_with_required_fail.stderr` (new)
- `crates/camel-endpoint/tests/ui/pattern_with_default_fail.rs` (new)
- `crates/camel-endpoint/tests/ui/pattern_with_default_fail.stderr` (new)
- `crates/camel-endpoint/tests/ui/pattern_with_secret_fail.rs` (new)
- `crates/camel-endpoint/tests/ui/pattern_with_secret_fail.stderr` (new)
- `crates/camel-endpoint/tests/ui/pattern_with_name_fail.rs` (new)
- `crates/camel-endpoint/tests/ui/pattern_with_name_fail.stderr` (new)
- `crates/camel-endpoint/tests/ui/pattern_with_aliases_fail.rs` (new)
- `crates/camel-endpoint/tests/ui/pattern_with_aliases_fail.stderr` (new)
- `crates/camel-endpoint/tests/ui/pattern_with_non_string_kind_fail.rs` (new)
- `crates/camel-endpoint/tests/ui/pattern_with_non_string_kind_fail.stderr` (new)
- `crates/camel-endpoint/tests/ui/empty_pattern_separator_fail.rs` (new)
- `crates/camel-endpoint/tests/ui/empty_pattern_separator_fail.stderr` (new)
- `crates/camel-endpoint/tests/ui/pattern_separator_without_dot_fail.rs` (new)
- `crates/camel-endpoint/tests/ui/pattern_separator_without_dot_fail.stderr` (new)

**Steps:**
1. For each of the 10 guardrails added in task 2.1, create a `*_fail.rs` file following
   the exact shape of `crates/camel-endpoint/tests/ui/secret_with_default_fail.rs`:
   - Imports `use camel_endpoint_macros::UriConfig;`
   - Defines a `#[derive(UriConfig)]` struct with `#[uri_scheme = "bad"]`, a path
     field, and the offending `#[uri_param(...)]` field.
   - The `pattern_on_non_vec_field_fail.rs` case uses a `String` field (not a Vec),
     with `#[uri_param(pattern = "param.")] bad: String` — expected error text:
     `` "`pattern` is only valid on fields of type `Vec<(String, String)>`" ``.
   - The `pattern_on_vec_wrong_inner_fail.rs` case uses a `Vec<String>` field (a Vec
     but wrong inner type — must be `Vec<(String, String)>`), with
     `#[uri_param(pattern = "param.")] bad: Vec<String>` — expected error text:
     `` "`pattern` is only valid on fields of type `Vec<(String, String)>`" ``.
   - `pattern_with_required_fail.rs`: `#[uri_param(pattern = "param.", required)] bad: Vec<(String, String)>` —
     expected error: `` "#[uri_param] cannot have both `pattern` and `required`; an open namespace cannot require a single key" ``.
   - `pattern_with_default_fail.rs`: `#[uri_param(pattern = "param.", default = "x")] bad: Vec<(String, String)>` —
     expected error: `` "#[uri_param] cannot have both `pattern` and `default`; an open namespace has no default value" ``.
   - `pattern_with_secret_fail.rs`: `#[uri_param(pattern = "param.", secret)] bad: Vec<(String, String)>` —
     expected error: `` "#[uri_param] cannot have both `pattern` and `secret`; an open namespace has no single secret value" ``.
   - `pattern_with_name_fail.rs`: `#[uri_param(pattern = "param.", name = "x")] bad: Vec<(String, String)>` —
     expected error: `` "#[uri_param] cannot have both `pattern` and `name`; the name is derived from the separator" ``.
   - `pattern_with_aliases_fail.rs`: `#[uri_param(pattern = "param.", aliases = ["x"])] bad: Vec<(String, String)>` —
     expected error: `` "#[uri_param] cannot have both `pattern` and `aliases`; a namespace matches by prefix, not by exact alias" ``.
   - `pattern_with_non_string_kind_fail.rs`: `#[uri_param(pattern = "param.", kind = "duration")] bad: Vec<(String, String)>` —
     expected error: `` "#[uri_param] `kind` on a pattern field must be `string` or omitted" ``.
   - `empty_pattern_separator_fail.rs`: `#[uri_param(pattern = "")] bad: Vec<(String, String)>` —
     expected error: `` "#[uri_param] `pattern` separator must be non-empty" ``.
   - `pattern_separator_without_dot_fail.rs`: `#[uri_param(pattern = "param/")] bad: Vec<(String, String)>` —
     expected error: `` "#[uri_param] `pattern` separator must end with `.` (the only permitted separator shape in this version)" ``.
   - Ends with `fn main() {}`.
   - **Dispatch order note:** task 2.2 MUST dispatch strictly after task 2.1 has
     landed (the macro guardrails are the source of truth for the error text; the
     `.stderr` snapshots must byte-match the macro's emitted diagnostics).
2. Run `TRYBUILD=overwrite cargo test -p camel-endpoint --test endpoint_macros_ui_tests`
   once to generate the initial `.stderr` snapshots.
3. Inspect each generated `.stderr` file and verify:
   - The error message matches the exact text in the bullet list above.
   - The error span points at the offending field (the `^^^` marker is under the
     field name, mirroring `secret_with_default_fail.stderr`).
4. Re-run `cargo test -p camel-endpoint --test endpoint_macros_ui_tests` WITHOUT
   `TRYBUILD=overwrite` to confirm every snapshot matches.

**Tests:**
- The trybuild runner `endpoint_macros_ui_tests.rs` already globs `tests/ui/*_fail.rs`. No code change to the runner is needed; the new test files are auto-discovered.
- `ui_tests_pass`:
  - **name:** `ui_tests_pass`
  - **setup:** the 10 new `*_fail.rs` files and their `.stderr` snapshots exist on disk under `crates/camel-endpoint/tests/ui/`; the trybuild runner `endpoint_macros_ui_tests.rs` is unchanged.
  - **action:** run `cargo test -p camel-endpoint --test endpoint_macros_ui_tests`.
  - **assert:** the command exits 0; all 17 cases (7 pre-existing + 10 new) report `PASS`, zero `FAIL`, zero stderr drift.
  - **command:** `cargo test -p camel-endpoint --test endpoint_macros_ui_tests`
  - **expected:** before implementation exits non-zero (cases missing); after implementation exits 0.

**Acceptance:**
- `cargo test -p camel-endpoint --test endpoint_macros_ui_tests` exits 0 (all 17 cases pass).
- For each of the 10 new `.stderr` files, its first line matches `^error: ` AND contains the exact error message text inlined in step 1's bullet list. Verified by `head -1 crates/camel-endpoint/tests/ui/<name>_fail.stderr | grep -E '^error: .+#\[uri_param\]'`.
- `cargo fmt --check --all` exits 0.

- [x] 2.2

## camel-lint

### Task 3.1: Extend resolve_option with pattern matching + tests

**Depends on:** Task 1.1 (consumes `UriOptionMatch::Prefix` from `camel_api::component_metadata`).

**Files:**
- `crates/camel-lint/src/route_view.rs` (modified)
- `crates/camel-lint/src/rules/ruriknown.rs` (modified — add tests)

**Steps:**
1. In `crates/camel-lint/src/route_view.rs`, locate `resolve_option` at line 201.
   The current body is:
   ```rust
   let key = opt.key.value.as_str();
   uri_options
       .iter()
       .find(|uo| uo.name == key || uo.aliases.contains(&opt.key.value))
   ```
   Replace it with a two-phase resolution that implements the order spec'd in the
   blessed delta spec requirement "Open namespace URI options". Phase 1 intentionally
   preserves the existing combined `name == key || aliases.contains(...)` pass (it is
   semantically equivalent to separate name-then-alias steps UNLESS a metadata alias
   shadows another option's name — an authoring error we do not need to second-guess;
   do NOT split the existing pass).
   ```rust
   let key = opt.key.value.as_str();
   // Phase 1: exact-name and alias, only on options whose pattern is None.
   if let Some(hit) = uri_options.iter().find(|uo| {
       uo.pattern.is_none() && (uo.name == key || uo.aliases.contains(&opt.key.value))
   }) {
       return Some(hit);
   }
   // Phase 2: pattern match, longest separator first, non-empty suffix required.
   let mut pattern_hits: Vec<&UriOption> = uri_options
       .iter()
       .filter(|uo| match &uo.pattern {
           Some(UriOptionMatch::Prefix { separator }) => {
               !separator.is_empty()
                   && key.len() > separator.len()
                   && key.starts_with(separator.as_str())
           }
           _ => false,
       })
       .collect();
   pattern_hits.sort_by(|a, b| {
       let la = match &a.pattern { Some(UriOptionMatch::Prefix { separator }) => separator.len(), _ => 0 };
       let lb = match &b.pattern { Some(UriOptionMatch::Prefix { separator }) => separator.len(), _ => 0 };
       lb.cmp(&la) // descending
   });
   pattern_hits.first().copied()
   ```
2. Add `use camel_api::component_metadata::UriOptionMatch;` to the imports at the top
   of `route_view.rs` if not already present (match how `UriOption` itself is imported).
3. In `crates/camel-lint/src/rules/ruriknown.rs`, add the following tests in the
   existing `#[cfg(test)] mod tests` block. Each test builds a small `LintOption` /
   `UriOption` fixture and calls `resolve_option` directly (it is `pub(crate)`, so
   the test must be inside the crate; place it in `ruriknown.rs`'s test mod which
   already has access via `crate::route_view::resolve_option`).

**Tests:**
- `pattern_prefix_resolves_non_empty_suffix`: setup — `uri_options = vec![UriOption::new("param", "namespace", OptionKind::String).pattern_prefix("param.")]`, `opt.key.value = "param.foo"`; action — `resolve_option(&opt, &uri_options)`; assert — returns `Some(&)` to the namespace option.
- `pattern_prefix_rejects_empty_suffix`: setup — same `uri_options`, `opt.key.value = "param."`; action — `resolve_option(&opt, &uri_options)`; assert — returns `None` (the bare `param.` key does not match; caller will treat as UnknownOption).
- `pattern_prefix_rejects_unrelated_key`: setup — same `uri_options`, `opt.key.value = "direction"`; action — `resolve_option(&opt, &uri_options)`; assert — returns `None`.
- `discrete_option_wins_over_pattern_on_name_collision`: setup — `uri_options = vec![UriOption::new("param.foo", "discrete", OptionKind::String), UriOption::new("param", "namespace", OptionKind::String).pattern_prefix("param.")]`, `opt.key.value = "param.foo"`; action — `resolve_option(&opt, &uri_options)`; assert — returns the discrete option (the one with `name == "param.foo"` and `pattern == None`), not the pattern option.
- `discrete_option_wins_when_pattern_derived_name_collides`: setup — `uri_options = vec![UriOption::new("param", "discrete", OptionKind::String), UriOption::new("param", "namespace", OptionKind::String).pattern_prefix("param.")]` (both options have `name == "param"`; the second has `pattern = Some(Prefix{separator:"param."})`), `opt.key.value = "param"` (no suffix); action — `resolve_option(&opt, &uri_options)`; assert — returns the discrete option (the one with `pattern == None`), NOT the pattern option. This proves that patterned options' derived `name` does not participate in Phase-1 exact-name matching (spec scenario "patterned option name does not collide with a discrete name").
- `longest_pattern_separator_wins`: setup — `uri_options = vec![UriOption::new("param", "short", OptionKind::String).pattern_prefix("param."), UriOption::new("param.foo", "long", OptionKind::String).pattern_prefix("param.foo.")]`, `opt.key.value = "param.foo.bar"`; action — `resolve_option(&opt, &uri_options)`; assert — returns the option whose `separator == "param.foo."` (the long one).
- `shorter_pattern_wins_when_longer_does_not_match`: setup — same `uri_options` as above, `opt.key.value = "param.baz"`; action — `resolve_option(&opt, &uri_options)`; assert — returns the option whose `separator == "param."` (the short one).
- `alias_match_skipped_for_pattern_options`: setup — `uri_options = vec![UriOption::new("param", "namespace", OptionKind::String).with_alias("legacy").pattern_prefix("param.")]`, `opt.key.value = "legacy"`; action — `resolve_option(&opt, &uri_options)`; assert — returns `None` (pattern options' aliases do not participate in step 2; only step 3 prefix-matching applies, and `"legacy"` does not start with `"param."`).

**Acceptance:**
- `cargo build -p camel-lint` exits 0.
- `cargo test -p camel-lint --lib` passes (existing tests still green; 8 new tests pass).
- `cargo clippy -p camel-lint -- -D warnings` exits 0.
- `cargo test -p camel-cli --test lint_corpus` passes with **unchanged baseline** (no new diagnostics for xj/xslt; this proves Change 1 didn't drag in Change 2).

- [x] 3.1

## Documentation

### Task 4.1: ADR-0041 amendment + CONTEXT-MAP + per-crate CONTEXT.md updates

**Depends on:** Tasks 1.1, 2.1, 3.1 (documents the surfaces they introduce).

**Files:**
- `docs/adr/0041-component-metadata-capabilities-schema.md` (modified)
- `CONTEXT-MAP.md` (modified)
- `crates/camel-api/CONTEXT.md` (modified)
- `crates/camel-endpoint/CONTEXT.md` (modified)
- `crates/camel-lint/CONTEXT.md` (modified)

**Note:** No `GLOSSARY.md` exists in the rust-camel repo (verified via `fd -t f GLOSSARY`).
The canonical home for cross-cutting terms is `CONTEXT-MAP.md` under its `## Key Terms`
section (starts at `CONTEXT-MAP.md:96`); per-crate terms live in each crate's
`CONTEXT.md`. Do NOT create a new `GLOSSARY.md` file.

**Steps:**
1. In `docs/adr/0041-component-metadata-capabilities-schema.md`, append a new
   amendment section titled `## Amendment: Open Namespace Pattern Matching`
   immediately after the existing `## Amendment: Macro-Derived URI Options via
   #[derive(UriConfig)]` section (which ends before the document's final heading).
   The new amendment covers:
   - **Rationale:** some components accept `param.<name>=<value>` pairs into a
     `Vec<(String, String)>` field (e.g. `camel-xj`, `camel-xslt` stylesheet params).
     The exact-name `UriOption` model cannot describe this open namespace.
   - **Decision:** add `UriOptionMatch::Prefix` (`#[non_exhaustive]`), `UriOption.pattern`
     (`Option<UriOptionMatch>`, serialized `skip_serializing_if = "Option::is_none"`),
     the `#[uri_param(pattern = "param.")]` macro key with eight guardrails, and the
     `resolve_option` three-step resolution (discrete name → alias → pattern,
     longest-prefix-wins, empty-suffix rejected).
   - **Forward-compat cost:** each future `UriOptionMatch` variant expands a closed
     JSON-Schema union; compatibility review and regenerated downstream consumers
     are required per variant.
   - **Out of scope:** component migration (xj, xslt) is Change 2.
2. In `CONTEXT-MAP.md`, add three new entries to the **Key Terms** section (which
   starts at line 96), matching the existing entry format `- **<Term>** — <description>`:
   - `open namespace` — "A URI query-key namespace of the form `<prefix>.<name>`
     where any non-empty `<name>` is valid. Modeled by `UriOptionMatch::Prefix`."
   - `UriOptionMatch` — "`#[non_exhaustive]` enum in `camel-api` describing how a
     `UriOption` with `pattern: Some(_)` matches URI query keys. Initial variant:
     `Prefix { separator }`."
   - `pattern prefix` — "The compile-time guarantee that a `#[uri_param(pattern = "..")]`
     separator ends with `.`; runtime matching uses `starts_with(separator)` plus a
     non-empty-suffix check."
3. In `crates/camel-api/CONTEXT.md`, add a subsection (or extend the existing
   contract-surface section) documenting: the new `UriOptionMatch` enum, the new
   `UriOption.pattern` field with its serde annotation, and the new
   `UriOption::pattern_prefix(&str)` consuming builder.
4. In `crates/camel-endpoint/CONTEXT.md`, document the new `#[uri_param(pattern = "..")]`
   key in the macro authoring surface: syntax, the `Vec<(String, String)>`-only
   constraint, the trailing-`.` precondition, and the eight incompatible-key guardrails.
5. In `crates/camel-lint/CONTEXT.md`, document the extended `resolve_option`
   semantics: two-phase order (Phase 1 = combined exact-name OR alias on `pattern.is_none()`
   options; Phase 2 = longest-prefix-wins on `pattern.is_some()` options), and the
   non-empty-suffix requirement.

**Tests:**
- `lint_context_citations_pass`:
  - **name:** `lint_context_citations_pass`
  - **setup:** the five documentation files listed above have been edited; the new terms `open namespace`, `UriOptionMatch`, and `pattern prefix` are each mentioned in `CONTEXT-MAP.md`'s Key Terms section AND in the relevant crate `CONTEXT.md`.
  - **action:** run `cargo xtask lint-context-citations`.
  - **assert:** exit code 0 (the gate fails if any contract-crate symbol referenced in code lacks a CONTEXT.md citation, or if any CONTEXT.md term is unreachable).
  - **command:** `cargo xtask lint-context-citations`
  - **expected:** before doc edits exits non-zero; after edits exits 0.
- `adr_amendment_heading_level_correct`:
  - **name:** `adr_amendment_heading_level_correct`
  - **setup:** the new amendment section has been appended to `docs/adr/0041-component-metadata-capabilities-schema.md`.
  - **action:** run `rg '^## Amendment: Open Namespace Pattern Matching$' docs/adr/0041-component-metadata-capabilities-schema.md`.
  - **assert:** the command prints exactly one matching line (no `rg -n` prefix because we did not pass `-n`; the heading level is `##`, matching the existing `## Amendment: Macro-Derived URI Options` precedent).
  - **command:** `rg '^## Amendment: Open Namespace Pattern Matching$' docs/adr/0041-component-metadata-capabilities-schema.md`
  - **expected:** exit code 0 with exactly one printed line.

**Acceptance:**
- `cargo xtask lint-context-citations` exits 0.
- `rg '^## Amendment: Open Namespace Pattern Matching$' docs/adr/0041-component-metadata-capabilities-schema.md` exits 0 and prints exactly one line (verified by `| wc -l` equals `1`).
- `rg -c 'open namespace' CONTEXT-MAP.md` returns at least 1 (the term is reachable from the canonical Key Terms section).
- `rg -c 'UriOptionMatch' crates/camel-api/CONTEXT.md` returns at least 1.
- `rg -c 'pattern' crates/camel-endpoint/CONTEXT.md` returns at least 1.
- `rg -c 'resolve_option' crates/camel-lint/CONTEXT.md` returns at least 1.
- `cargo fmt --check --all` exits 0 (formatting of any embedded code blocks is consistent).

- [x] 4.1

## Schema regeneration

### Task 5.1: Regenerate and commit schemas/component-metadata.json

**Depends on:** Task 1.1 (the schema regen captures the contract change to `UriOption`
+ the new `UriOptionMatch` enum).

**Files:**
- `schemas/component-metadata.json` (modified — regenerated)

**Steps:**
1. From the worktree root, run `cargo xtask schema` (the regen subcommand; check
   `cargo xtask schema --help` for the exact flag set if the default invocation
   does not regenerate `component-metadata.json`). This emits the new JSON Schema
   artifact including the `pattern` property on `UriOption` and the `UriOptionMatch`
   definition.
2. Run `git diff --stat schemas/component-metadata.json` and verify the diff is
   purely additive: a new `pattern` property under `UriOption`'s `properties`, and
   a new `UriOptionMatch` definition under `$defs` (or the schema's equivalent
   definitions section). No existing property should be removed or renamed.
3. Run `cargo xtask schema --check` to confirm the regen is idempotent.
4. Commit the regenerated artifact as part of this task's deliverable (the conductor
   will fold this commit into the broader Change 1 commit at PHASE 4).

**Tests:**
- `schema_check_passes`:
  - **name:** `schema_check_passes`
  - **setup:** task 1.1's contract changes have landed; `cargo xtask schema` has been run once to regenerate `schemas/component-metadata.json`.
  - **action:** run `cargo xtask schema --check`.
  - **assert:** exit code 0 (the on-disk schema equals the freshly-generated schema).
  - **command:** `cargo xtask schema --check`
  - **expected:** before regen exits non-zero; after regen exits 0.
- `schema_diff_has_no_deletions`:
  - **name:** `schema_diff_has_no_deletions`
  - **setup:** the regenerated `schemas/component-metadata.json` is staged.
  - **action:** run `git diff --cached --unified=0 schemas/component-metadata.json | rg '^-' | rg -v '^---'`.
  - **assert:** the command exits 1 (no matches — there are no deletion lines, only additions). This is the executable no-deletion check.
  - **command:** `git diff --cached --unified=0 schemas/component-metadata.json | rg '^-' | rg -v '^---'`
  - **expected:** exit code 1 (zero matches).
- `schema_contains_pattern_property_and_urioptionmatch_definition`:
  - **name:** `schema_contains_pattern_property_and_urioptionmatch_definition`
  - **setup:** the regenerated `schemas/component-metadata.json` is on disk.
  - **action:** run `rg '"pattern"' schemas/component-metadata.json && rg -i 'urioptionmatch' schemas/component-metadata.json`.
  - **assert:** both sub-commands exit 0 (the optional `pattern` property exists on `UriOption`, and the `UriOptionMatch` definition exists).
  - **command:** `rg '"pattern"' schemas/component-metadata.json && rg -i 'urioptionmatch' schemas/component-metadata.json`
  - **expected:** exit 0.
- `schema_regen_is_idempotent`:
  - **name:** `schema_regen_is_idempotent`
  - **setup:** the regenerated `schemas/component-metadata.json` is on disk; record its checksum: `BEFORE=$(sha256sum schemas/component-metadata.json | cut -d' ' -f1)`.
  - **action:** run `cargo xtask schema` a second time; then `AFTER=$(sha256sum schemas/component-metadata.json | cut -d' ' -f1)`.
  - **assert:** `[ "$BEFORE" = "$AFTER" ]` exits 0 (re-running produces zero diff).
  - **command:** `BEFORE=$(sha256sum schemas/component-metadata.json | cut -d' ' -f1) && cargo xtask schema && AFTER=$(sha256sum schemas/component-metadata.json | cut -d' ' -f1) && [ "$BEFORE" = "$AFTER" ]`
  - **expected:** exit 0.

**Acceptance:**
- `cargo xtask schema --check` exits 0.
- `git diff --cached --unified=0 schemas/component-metadata.json | rg '^-' | rg -v '^---'` exits 1 (no deletions).
- `rg '"pattern"' schemas/component-metadata.json` exits 0 AND `rg -i 'urioptionmatch' schemas/component-metadata.json` exits 0.
- The idempotency checksum command above exits 0.

- [x] 5.1
