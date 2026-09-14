# Tasks: jobtyped

## camel-cli job documents

### Task 1: Type model and strict `type:` declaration parsing

**Files:**
- `crates/camel-cli/src/commands/job/document.rs` (modified)
- `crates/camel-cli/src/commands/job/document_tests.rs` (modified)
- `crates/camel-cli/src/commands/job/help_tests.rs` (modified — compile fix only)

**Steps:**
1. Verify single-source assumption: search `crates/camel-cli/src` for any strict per-argument declaration parser besides `job_argument_declaration` in `document.rs` (rg for `UnknownArgumentField` and `required.*default.*description` match arms). If a second copy exists, fold it onto the one in `document.rs` before proceeding; report the finding in the task result.
2. Add `pub(crate) enum JobArgType { String, Int, Bool, Enum(Vec<String>) }` with `impl JobArgType { pub(crate) fn render(&self) -> String }` rendering `string`, `int`, `bool`, and `enum[` + members joined `,` + `]` (ONE renderer, shared later by coercion diagnostics and the help column); add field `pub(crate) arg_type: JobArgType` on `JobArgumentDeclaration`; an omitted `type:` key constructs `JobArgType::String`.
3. Add `JobDocError::InvalidArgumentType { argument: String, raw: String }` and `JobDocError::ArgumentCoercion { name: String, expected: JobArgType, raw: String }` with Display lines: InvalidArgumentType names the argument and the raw value (for example: `invalid type 'flot' for argument 'count': expected string, int, bool, or enum[...]`); ArgumentCoercion names the argument, `expected.render()`, and the raw value.
4. In `job_argument_declaration`, admit key `type` with a string-scalar value; parse it with new `fn parse_arg_type(raw: &str) -> Result<JobArgType, String>`: exact words `string`, `int`, `bool`; otherwise the value MUST start `enum[` and end `]` — split the interior on `,`, trim each member, reject the whole value when any member is empty after trim, when a member contains `[`, `]`, `,`, CR, or LF, when a duplicate member appears after trim, or when the list is empty. Malformed values return `InvalidArgumentType`; a non-string `type` scalar returns `InvalidArgumentDeclaration` with detail `type must be a string`.
5. Add `fn coerce_argument(value: &str, arg_type: &JobArgType) -> Option<String>` returning the canonical string form: `String` → verbatim; `Int` → `i64::from_str(value).ok().map(|i| i.to_string())` (no whitespace tolerated, overflow rejected); `Bool` → match `value.to_ascii_lowercase()` exactly `true`/`false` (NOT `1`/`0`) and canonicalize lowercase; `Enum(members)` → `members.iter().find(|m| *m == value)` verbatim.
6. In `normalize_job_args` (after building each declaration): when `arg_type` is not `String` and a `default` is declared, run `coerce_argument(default, arg_type)`; on `None` return `JobDocError::ArgumentCoercion { name, expected, raw }`. Store the declaration with the RAW default text unchanged — canonicalization happens at resolution (Task 2); the load-time check is rejection only.
7. In `help_tests.rs`, add `arg_type: JobArgType::String` to every `JobArgumentDeclaration` struct literal (helper `declared_interface_info()` and render tests) so the crate compiles; help output stays byte-identical until Task 4.
8. Add the declaration tests below to `document_tests.rs` following the existing helper style.

**Tests:** (all: command `cargo test -p camel-cli commands::job::document_tests` — expected pass after implementation)
- `arg_type_each_spelling_accepted`: arrange a document with `a: {type: string}`, `b: {type: int}`, `c: {type: bool}`, `d: {type: "enum[x,y]"}`; act `parse_job_document`; assert four declarations parsed with matching `JobArgType` values and `a` indistinguishable from an untyped declaration.
- `arg_type_unknown_word_rejected`: arrange `count: {type: flot}`; act parse; assert `InvalidArgumentType` naming `count` and `flot`.
- `arg_type_malformed_enum_grammar_rejected`: arrange three documents with `type: "enum[]"`, `type: "enum[a,,b]"`, `type: "enum[a,a]"`; act parse on each; assert each fails with `InvalidArgumentType` naming the argument and the raw value.
- `arg_type_enum_members_trimmed_and_case_preserved`: arrange `type: "enum[gold, silver]"`; act parse; assert members are exactly `gold`, `silver`.
- `arg_type_non_string_scalar_rejected`: arrange `type: 42` (YAML integer); act parse; assert `InvalidArgumentDeclaration` detail says `type` must be a string.
- `arg_type_typed_default_failing_coercion_fails_load`: arrange `count: {type: int, default: "abc"}`; act parse; assert `ArgumentCoercion` naming `count`, `int`, `abc`.
- `arg_type_omitted_defaults_to_string`: arrange a declaration without `type`; act parse; assert `arg_type == JobArgType::String` and the raw `default` text survives verbatim.

**Acceptance:**
- Untyped documents parse through the exact prior code path values (`arg_type` defaults without altering A2 diagnostics).
- `cargo test -p camel-cli commands::job::document_tests` exits 0.
- `cargo clippy -p camel-cli -- -D warnings` exits 0.

- [x] 1

### Task 2: Resolution-time coercion with pinned precedence

**Files:**
- `crates/camel-cli/src/commands/job/document.rs` (modified)
- `crates/camel-cli/src/commands/job/document_tests.rs` (modified)

**Steps:**
1. In `resolve_job_args`, keep the existing loop order UNCHANGED (unknown-name check during pair iteration, then missing-required scan, then default fill) so precedence unknown-name > missing-required > coercion holds structurally.
2. After the default-fill loop, add a final pass over `declarations.entries` in BTreeMap order: for each entry with non-`String` `arg_type` whose name is in `resolved`, call `coerce_argument`; on `Some(canonical)` overwrite `resolved[name] = canonical`, on `None` return `JobDocError::ArgumentCoercion { name, expected: arg_type, raw }` (the variant from Task 1; `expected` renders `int`, `bool`, or `enum[a,b,c]` listing members).
3. Extend the `resolve_job_args` doc comment with the precedence statement and the canonical-overwrite rule.
4. Add the resolution tests below to `document_tests.rs`.

**Tests:** (command `cargo test -p camel-cli commands::job::document_tests` — expected pass after implementation)
- `resolve_int_coerces_canonical_form`: arrange declarations `count: {type: int}`; act `resolve_job_args` with pairs `[("count", "007")]`; assert map holds `count -> "7"`.
- `resolve_int_rejects_non_integer`: act with pairs `[("count", "abc")]` and separately `[("count", "3.5")]` and `[("count", " 42")]`; assert `ArgumentCoercion` naming `count`, `int`, and the raw value in each case.
- `resolve_int_plus_sign_and_overflow`: act with `[("count", "+5")]` asserting `count -> "5"`; act with `[("count", "99999999999999999999")]` asserting `ArgumentCoercion`.
- `resolve_bool_case_insensitive_canonical_lowercase`: act with `[("verbose", "TRUE")]` asserting `verbose -> "true"` and `[("verbose", "False")]` asserting `verbose -> "false"`.
- `resolve_bool_rejects_numeric_and_unknown`: act with `[("verbose", "1")]`, `[("verbose", "0")]`, `[("verbose", "yes")]`; assert `ArgumentCoercion` naming `verbose`, `bool`, and the raw value each time.
- `resolve_enum_member_verbatim_and_outsider_lists_members`: arrange `tier: {type: "enum[bronze,gold]"}`; act with `[("tier", "gold")]` asserting `tier -> "gold"`; act with `[("tier", "silver")]` asserting `ArgumentCoercion` whose rendered message contains `enum[bronze,gold]`; act with `[("tier", "Gold")]` asserting the same rejection (membership is case-sensitive).
- `resolve_typed_default_canonicalizes_without_pair`: arrange `count: {type: int, default: "007"}`; act `resolve_job_args` with empty pairs; assert map holds `count -> "7"`.
- `resolve_unknown_name_precedes_coercion`: arrange `count: {type: int}` only; act with pairs `[("ghost", "1"), ("count", "abc")]`; assert `UnknownArgumentName` naming `ghost` (not a coercion error).
- `resolve_missing_required_precedes_coercion`: arrange `name: {type: string, required: true}` (no default) and `count: {type: int}`; act with pairs `[("count", "abc")]` and no `name` pair; assert `MissingRequiredArgument` naming `name`.
- `resolve_untyped_values_stay_verbatim`: arrange untyped `tier: {default: gold}`; act with pairs `[("tier", "007")]`; assert `tier -> "007"` verbatim (A2 bit-identical).

**Acceptance:**
- `cargo test -p camel-cli commands::job::document_tests` exits 0.
- `cargo clippy -p camel-cli -- -D warnings` exits 0.

- [x] 2

### Task 3: Canonical forms substitute through the interpolation stage

**Files:**
- `crates/camel-cli/src/commands/job/tests.rs` (modified)

**Steps:**
1. No production change is expected in this task — `interpolate_declared_fields` already consumes the resolved map produced by `resolve_job_args`; verify (by reading the call chain in `mod.rs`) that one-shot and batch execution pass the SAME resolved map (single `parse_job_document_with_args` call before mode dispatch), then write the tests that pin the behavior. If the chain differs from expectation, STOP and report `integration-gap: <what>` instead of restructuring.
2. Add one-shot integration tests (existing booted-route helper style in `tests.rs`: `run_camel_job`, `job_test_binary`, `write_tap_route`) exercising canonical substitution in every field surface.
3. Add one batch-mode test in `tests.rs` beside the existing harness, following the `mode: batch` fixture shape from `tests/job_one_shot_test.rs` (seda fan-out recording headers to `mock:`).

**Tests:**
- `typed_args_interpolate_canonical_forms_all_fields`: arrange a job declaring `target: {type: "enum[direct:in,direct:out]"}`, `count: {type: int, default: "7"}`, `verbose: {type: bool}` with `to: direct:${arg:target}`-style references, `body: "n=${arg:count} v=${arg:verbose}"`, `headers: {tier: "${arg:target}"}`, and `timeout: "${arg:wait}s"` backed by `wait: {type: int, default: "30"}`; act run with `--arg verbose=false --arg target=direct:out --arg count=007` (adapt the enum members to the job's real consumer route names in the test fixture; the explicit `count=007` pair exercises CLI-pair canonicalization while `wait` exercises default canonicalization); assert recorded route/message data show `direct:out`, `n=7 v=false`, header `tier=direct:out`, and an exit 0 with the timeout accepted; command `cargo test -p camel-cli commands::job::tests::typed_args_interpolate_canonical_forms_all_fields`; expected pass after implementation.
- `typed_coercion_failure_exits_2_before_boot`: arrange the same job; act run with `--arg count=abc`; assert exit 2, stderr names `count`, `int`, `abc`, and no boot/report side effects; command `cargo test -p camel-cli commands::job::tests::typed_coercion_failure_exits_2_before_boot`; expected pass after implementation.
- `untyped_document_behavior_unchanged`: arrange the A2-style job (string defaults only, one `--arg` override with leading zeros text value); act run; assert the value passes through verbatim exactly as A2 did; command `cargo test -p camel-cli commands::job::tests::untyped_document_behavior_unchanged`; expected pass after implementation.
- `batch_typed_arg_coerces_and_drains`: arrange a batch job (in `tests.rs`, fixture shape from `tests/job_one_shot_test.rs` — seda fan-out observed via a `file:` write through a `transform: {simple: ...}` header stamp, exactly like the existing `batch_works_with_arg_injection` fixture) declaring `batch_id: {type: int}` (identifier grammar forbids `-`); act run with `--arg batch_id=007`; assert the observed file content carries the coerced value `batch_id=7` (canonical, not `007`) and the job drains and exits 0; command `cargo test -p camel-cli commands::job::tests::batch_typed_arg_coerces_and_drains`; expected pass after implementation.

**Acceptance:**
- `cargo test -p camel-cli commands::job::tests commands::job::batch` exits 0.
- `cargo clippy -p camel-cli -- -D warnings` exits 0.

- [x] 3

## camel-cli help projection

### Task 4: Help renders the declared type with aligned column

**Files:**
- `crates/camel-cli/src/commands/job/help.rs` (modified)
- `crates/camel-cli/src/commands/job/help_tests.rs` (modified)

**Steps:**
1. In `render_job_help`, compute `type_width` = max `arg_type.render().len()` across the job's declarations (same per-job width strategy as `name_width`); pass it to `render_argument_row`.
2. In `render_argument_row`, replace the hardcoded `"  string  "` with: the existing two-space name separator, then `declaration.arg_type.render()` padded on the right to `type_width`, then two spaces (so the `required`/`optional` marker starts at a column aligned across all rows; a job with only untyped args renders exactly as before — `string` is both the widest and only value, reproducing today's output). Import `JobArgType` from `super::document`; do NOT add a second renderer — reuse `JobArgType::render` from Task 1.
3. Add the help tests below to `help_tests.rs` in the existing style.

**Tests:** (command `cargo test -p camel-cli commands::job::help_tests` — expected pass after implementation)
- `help_renders_declared_type_per_argument`: arrange a `JobHelpInfo` with `count: {type: int}`, `verbose: {type: bool}`, `tier: {type: "enum[bronze,gold]"}`, and one untyped `name`; act `render_job_help`; assert rows show `int`, `bool`, `enum[bronze,gold]`, and `string` respectively.
- `help_type_column_aligns_across_rows`: arrange `count: {type: int}` and `tier: {type: "enum[bronze,gold]"}`; act render; assert the index of the `required`/`optional` marker is identical on both rows and equals its index on a job where every row is that widest type.
- `help_untyped_job_renders_string_column_unchanged`: arrange a declarations set identical to an existing A3 golden test case; act render; assert output byte-identical to the pre-change expectation (the type column still reads `string` with the old width).
- `help_declaration_error_exits_2`: arrange a document with `count: {type: int, default: "abc"}`; act `parse_job_document_for_help`; assert `ArgumentCoercion` error surfaces (help shares declaration checks); command `cargo test -p camel-cli commands::job::help_tests`; expected pass after implementation.

**Acceptance:**
- `cargo test -p camel-cli commands::job::help_tests` exits 0.
- `cargo clippy -p camel-cli -- -D warnings` exits 0.

- [x] 4

## compiled artifacts and docs

### Task 5: Compile-time declaration validation, artifact parity, and CONTEXT-MAP glossary

**Files:**
- `crates/camel-cli/src/commands/job/document.rs` (modified)
- `crates/camel-cli/src/commands/compile.rs` (modified)
- `crates/camel-cli/src/compile/runtime.rs` (modified — verification and at most comment updates; no behavioral edit expected)
- `crates/camel-cli/tests/compiled_artifact_test.rs` (modified)
- `CONTEXT-MAP.md` (modified)

**Steps:**
1. Add `pub(crate) fn validate_job_declarations_for_compile(text: &str) -> Result<(), JobDocError>` in `document.rs`: parse `text` as UNTYPED `serde_yaml::Value` (NOT `JobDocumentDoc` — the full strict shape would drag document-structure checks into compile time); extract only the `args:` mapping (an `args:` present but not a mapping fails with the existing argument-declaration error class; absent `args:` returns `Ok(())`); convert the mapping to `BTreeMap<String, serde_yaml::Value>` and run `normalize_job_args` on it, dropping the result. Deliberately narrow — declaration checks ONLY (name grammar, unknown fields, `type` grammar, typed-default coercion); NO document-structure or execution-value validation.
2. In `run_compile` (compile.rs), when the trailer kind is Job, call `validate_job_declarations_for_compile` on the document text BEFORE `write_artifact`; on `Err(e)`, follow the file's existing convention exactly: `eprintln!("camel compile: {e}"); return EXIT_REJECTION;` (the `EXIT_REJECTION` const is already 2 — do NOT hardcode a literal 2) with NO artifact written. Route documents keep the exact prior path.
3. Verify (read, do not restructure) that `compile/runtime.rs` resolves embedded job declarations through the shared parser path (`parse_job_document_with_args` with empty pairs at artifact startup) so typed-default coercion applies unchanged. If the path diverges, STOP and report `integration-gap: <what>`.
4. Add artifact tests: typed default coerces identically to the normal job; compile-time rejection of a bad typed default; compile-time rejection of an A2-style malformed declaration (`requried:` typo) in the same class.
5. Update the CONTEXT-MAP.md glossary entry `Declared job arguments (args:)`: after the `description` clause add that each declaration MAY carry `type` (`string` default, `int`, `bool`, `enum[...]` single string scalar; members trimmed, non-empty, unique), typed values coerce at resolution with canonical string forms substituted by `${arg:NAME}`, coercion and typed-default declaration failures exit 2, compile runs the argument-declaration checks for job documents (exit 2, no artifact), and `--help` renders the declared type. Keep the entry's existing authority/crate citation style; artifacts keep the closed `--arg` surface with embedded typed defaults coerced at startup.

**Tests:**
- `compiled_job_coerces_typed_default`: arrange a job declaring `count: {type: int, default: "007"}` with `${arg:count}` in `to`, compile it into an artifact, run the artifact AND the normal job; assert both interpolate `7` (identical send target) and both exit 0; command `cargo test -p camel-cli --test compiled_artifact_test compiled_job_coerces_typed_default`; expected pass after implementation.
- `compile_rejects_bad_typed_default`: arrange the same document with `default: "abc"`; act `camel compile`; assert exit 2 with the `ArgumentCoercion` diagnostic naming `count` and no artifact file produced; command `cargo test -p camel-cli --test compiled_artifact_test compile_rejects_bad_typed_default`; expected pass after implementation.
- `compile_rejects_malformed_declaration`: arrange a job document with the `requried:` typo in `args:`; act `camel compile`; assert exit 2 with the unknown-field diagnostic and no artifact produced; command `cargo test -p camel-cli --test compiled_artifact_test compile_rejects_malformed_declaration`; expected pass after implementation.

**Acceptance:**
- `cargo test -p camel-cli --test compiled_artifact_test` exits 0.
- CONTEXT-MAP.md entry mentions `type`, `enum[...]`, canonical coercion, exit 2, and the closed artifact `--arg` surface; `cargo xtask lint-context-citations` exits 0.
- `cargo clippy -p camel-cli -- -D warnings` exits 0.

- [x] 5
