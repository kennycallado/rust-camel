# Tasks: argdrop

Single-phase removal. Tasks run in order; each ends compile-green and
test-green. All work stays inside `crates/camel-cli`.

## Task 1 — remove-arg-flag-cli-surface

Remove the `--arg` static flag, its exclusive plumbing, and rework the
src-side unit tests that pin the old surface (design D1–D8).

**Files:**
- `crates/camel-cli/src/commands/job/mod.rs` (modified)
- `crates/camel-cli/src/commands/job/document.rs` (modified)
- `crates/camel-cli/src/commands/job/help.rs` (modified)
- `crates/camel-cli/src/commands/job/flags_tests.rs` (modified)
- `crates/camel-cli/src/commands/job/document_tests.rs` (modified)
- `crates/camel-cli/src/commands/job/help_tests.rs` (modified)
- `crates/camel-cli/src/commands/job/tests.rs` (modified — only where
  grep hits)
- `crates/camel-cli/src/commands/job/job_effective_config_tests.rs`
  (modified — only where grep hits)
- `crates/camel-cli/src/commands/job/batch_drain_tests.rs` (modified)
- `crates/camel-cli/src/compile/runtime.rs` (modified — doc comments
  only)

**Steps:**
1. `mod.rs`: delete the `pub args: Vec<(String, String)>` field and its
   `#[arg(long = "arg", ...)]` attribute from `JobArgs`; delete
   `fn parse_arg_pair`.
2. `mod.rs` `lower_dynamic_flags`: drop the `arg_pairs: &[(String, String)]`
   parameter; every `LoweredDynamic { pairs: arg_pairs.to_vec(), .. }`
   seed becomes `pairs: Vec::new()`; update the function doc comment to
   remove "phase-1 `--arg` pairs, then tail `--arg` pairs" precedence
   prose (pairs now come only from dynamic flags).
3. `mod.rs`: in the tail re-parse (`matches.get_many::<(String, String)>
   ("args")` block), delete the tail-pair recovery lines; `pairs` starts
   empty and only dynamic flags extend it.
4. `mod.rs`: delete `DynamicFlagError::CrossFormConflict` (variant,
   Display arm, doc comment) and its detection site (the loop comparing
   lowered dynamic names against pair keys).
5. `mod.rs`: reword Display strings that mention `--arg`:
   - `BoolFlagValue`: `boolean flag '--{name}' takes no value; use
     '--{name}' for true or '--no-{name}' for false` (drop the
     `or '--arg {name}=false'` clause).
   - `UndeclaredDocument`: `dynamic flag '{flag}' requires the document
     to declare an 'args:' block; declare the argument` (drop the
     `or use '--arg {flag}=VALUE'` clause).
6. `mod.rs`: delete `const LEGACY_ARG_DEPRECATION`, the
   `legacy_header_args` binding, and the deprecation-note `eprintln!`
   in `run_job`; delete the `cli_args:` field construction from the
   `JobRun` literal; delete `cli_args: Vec<(String, String)>` from
   `JobRun`; delete the `cli_args: Vec::new()` sites on embedded run
   paths; drop the `cli_args` parameter from
   `send_with_startup_retry` and the send-time header-injection
   `for (k, v) in cli_args` loop it consumed; update `JobRun` and
   call-site doc comments accordingly.
7. `document.rs`: delete `JobDocument::legacy_arg_headers`; delete the
   `JobDocError::UnknownArgumentName` variant (enum arm, Display arm,
   and its production site in argument resolution); reword
   `MissingRequiredArgument` Display from
   `pass --arg {name}=<value>` to `pass --{name} <value>`; change
   `RESERVED_ARGUMENT_NAMES` from `[&str; 4]` to `[&str; 3] =
   ["help", "config", "report"]` and update its doc comment; sweep
   doc comments referencing `--arg` (JobDocument::args field doc,
   `parse_job_document_with_args` docs, resolution-stage comments)
   to say dynamic-flag pairs.
8. `help.rs`: change `FLAG_SPELLING_NOTE` to
   `"  (settable as --<name> <VALUE>; bool as --<name> / --no-<name>)"`
   and update its doc comment (drop the `--arg` pair clause).
9. `flags_tests.rs`:
   - Update every `lower_dynamic_flags(...)` call to the 2-arg
     signature.
   - `lower_bool_value_form_rejected_naming_spellings`: assert the new
     message (must contain `--verbose` and `--no-verbose`, must NOT
     contain `--arg`).
   - `lower_dynamic_on_undeclared_document_errors`: assert the new
     UndeclaredDocument message (contains `args:` block wording, NOT
     `--arg`); the statics-only sub-case (`--arg a=1` on an undeclared
     doc) now maps to `UndeclaredDocument { flag: "arg" }` — keep a
     sub-case asserting an unknown long flag on an undeclared doc
     yields `UndeclaredDocument` naming the flag.
   - `lower_tail_statics_recovered`: drop the `--arg a=1` token; keep
     `--report` recovery over `--name w`; add an assertion that a tail
     `--arg x=1` is NOT a recoverable static (it fails lowering).
   - DELETE `lower_cross_form_conflict_both_orders`.
   - ADD `lower_value_flag_missing_value_is_clap_error` (see Tests).
10. `document_tests.rs`: DELETE tests exercising unknown `--arg` names
    (grep `UnknownArgumentName` and `unknown argument`); rework the
    mode-selection assertions at ~792 and ~803
    (`empty_args_select_declared_mode`,
    `absent_args_select_legacy_mode`) from the deleted
    `legacy_arg_headers()` accessor to the equivalent observable
    (`doc.args.is_some()` / `doc.args.is_none()`) — the parser's
    declared/raw-fields mode split stays, only the accessor dies;
    reword missing-required assertions to the new
    `pass --name <value>` spelling; convert `--arg verbose=TRUE`-style
    bool-coercion tests to default-based declarations (see Tests);
    keep typed-default tests unchanged except comments mentioning
    `--arg`.
11. `help_tests.rs`: update the byte-pinned `FLAG_SPELLING_NOTE`
    assertions to the new string.
12. `tests.rs`, `job_effective_config_tests.rs`,
    `batch_drain_tests.rs`: grep for `--arg`, `arg_pairs`, `cli_args`,
    `legacy_arg` — convert declared-path invocations to dynamic flags
    (e.g. `--arg tier=gold` → `--tier gold`), reword comments. In
    `tests.rs` specifically: DELETE whole legacy-path tests
    (`legacy_args_remain_headers_with_deprecation`, tests.rs:216-262 —
    no-args doc, repeated `--arg`, header-override + deprecation
    assertions) and apply the task-2 step-3 disposition to
    form-equivalence tests (`binary_dynamic_flag_runs_identical_to_arg`
    at tests.rs:1217: drop the `--arg` half; keep a single-run
    dynamic-flag version if it is the only end-to-end declared-path
    coverage in that file, else delete).
13. Add the reserved-name flip coverage (see Tests:
    `argument_named_arg_is_accepted`).
14. `compile/runtime.rs`: reword the module and function doc comments
    (~line 29 `EMPTY --arg`, ~line 67 `job arguments (--arg)`) to the
    dynamic-flag wording (embedded runs carry no dynamic flags and
    use embedded defaults).

**Tests:**
- name: `lower_value_flag_missing_value_is_clap_error`
  setup: a declarations set with a string arg `name`
  action: call `lower_dynamic_flags` with the tail `["--name"]`
  (flag present, no value, last token)
  assert: result is `Err(DynamicFlagError::Clap(_))` (clap's own
  missing-value diagnostic)
  command: `cargo test -p camel-cli --lib lower_value_flag_missing_value_is_clap_error`
  expected: passes after step 9.
- name: `argument_named_arg_is_accepted`
  setup: a job document declaring `args: {arg: {default: "x"}}`
  action: parse with `parse_job_document_with_args`
  assert: parse succeeds; the declaration is kept (the name `arg` is
  no longer reserved)
  command: `cargo test -p camel-cli --lib argument_named_arg_is_accepted`
  expected: passes after step 7.
- name: `bool_typed_default_canonicalizes_case`
  setup: a job document declaring `verbose: {type: bool, default: "TRUE"}`
  action: parse + resolve without any dynamic flag
  assert: `${arg:verbose}` substitutes canonical `true`
  command: `cargo test -p camel-cli --lib bool_typed_default_canonicalizes_case`
  expected: passes after step 10 (convert any existing
  `--arg verbose=TRUE` test to this name and shape; add it if absent).
- verify: `cargo test -p camel-cli --lib` green (whole lib).
- verify: `cargo fmt --check --all` and
  `cargo clippy -p camel-cli -- -D warnings` and
  `cargo clippy -p camel-cli --no-default-features
  --features flavor-regular,exec --all-targets -- -D warnings` exit 0.

**Acceptance:**
- `grep -rn -- '--arg' crates/camel-cli/src/` returns hits ONLY in
  `flags_tests.rs` negative assertions (diagnostics must NOT contain
  `--arg`) and rejection invocations (`--arg` must fail) — the
  negative-coverage literals step 9 mandates.
- `grep -rn 'parse_arg_pair\|LEGACY_ARG\|CrossFormConflict\|
  UnknownArgumentName\|legacy_arg_headers\|cli_args'
  crates/camel-cli/src/` returns no hits.
- `cargo test -p camel-cli --lib` exits 0.
- fmt and both clippy legs exit 0.

- [x] remove-arg-flag-cli-surface

## Task 2 — convert-integration-tests

Convert the integration fleet: delete legacy implicit-header coverage,
move declared-path coverage to dynamic flags, keep artifact rejection
coverage.

**Files:**
- `crates/camel-cli/tests/job_one_shot_test.rs` (modified)
- `crates/camel-cli/tests/compiled_artifact_test.rs` (modified)

**Steps:**
1. `job_one_shot_test.rs` around line 2461 (section header
   `-- --arg header injection (add-job-args-batch)`): DELETE the
   legacy-path tests — single-and-repeated args reach the route,
   CLI arg overrides a colliding document header, malformed `--arg`
   value is a usage error. These test the removed path; document
   `send.headers` coverage elsewhere in the file stays as-is. Remove
   or rewrite the section header comment.
2. `job_one_shot_test.rs` around line 439 (`Batch mode + --arg
   injection`): declare `batch_id` in the fixture document's `args:`
   block (default can stay absent if the flag supplies it), change the
   invocation to `["job.job.yaml", "--batch_id", "42"]`, and update
   the test name/doc to `batch works with a dynamic flag`. If the
   fixture's route reads the value as a header via `${arg:batch_id}`
   interpolation, keep the recording assertion; if it relied on
   implicit header injection, rewrite the fixture to interpolate
   `${arg:batch_id}` into `send.headers` so the recorded assertion
   still holds.
3. `job_one_shot_test.rs` around line 1117 (dynamic-flag vs legacy
   `--arg` comparison test): DELETE the `--arg name=world` comparison
   half; if the test's only purpose was form-equivalence, delete the
   whole test (the equivalence target no longer exists); if it also
   proves the declared dynamic path end-to-end, keep a single-run
   dynamic-flag version.
4. `compiled_artifact_test.rs`: keep the artifact-rejects-`--arg`
   tests (artifacts still reject it as an unknown flag) — reword
   comments that describe `--arg` as the CLI surface; the assertion
   `!combined.contains("pass --arg")` still holds under the new
   missing-required wording; sweep remaining comment mentions
   (`--arg stays outside the artifact surface`) to describe an unknown
   flag, not a sibling CLI flag.
5. Grep both files for residual `--arg` occurrences and resolve each:
   convert (declared path), delete (legacy path), or reword (rejection
   coverage).

**Tests:**
- name: batch dynamic flag (renamed existing test)
  setup: batch job declaring `batch_id` whose worker route records
  headers to `mock:`
  action: run `["job.job.yaml", "--batch_id", "42"]`
  assert: exit 0, job drains, recorded exchange carries the resolved
  `batch_id` value
  command: `cargo test -p camel-cli --test job_one_shot_test`
  expected: passes after step 2.
- name: artifact rejects `--arg` (existing test kept)
  setup: compiled artifact from a job declaring `value`
  action: run artifact with `["--arg", "value=other"]`
  assert: exit != 0, output rejects the unknown flag
  command: `cargo test -p camel-cli --test compiled_artifact_test`
  expected: passes unchanged behavior.

**Acceptance:**
- `cargo build -p camel-cli` exits 0 (binary present for tests).
- `cargo test -p camel-cli --test job_one_shot_test --test
  compiled_artifact_test` exits 0.
- `grep -n -- '--arg' crates/camel-cli/tests/` hits only artifact
  rejection coverage and its comments.

- [x] convert-integration-tests

## Task 3 — docs-context-and-residual-sweep

Update the crate's durable docs and prove the surface is gone
repo-wide (within the zone).

**Files:**
- `crates/camel-cli/CONTEXT.md` (modified)
- `crates/camel-cli/src/commands/job/tests.rs` (modified — step 5)
- `crates/camel-cli/src/commands/job/document_tests.rs` (modified —
  step 6)

**Steps:**
1. `CONTEXT.md` line ~116: replace the sentence asserting a
   `--arg NAME=VALUE` pair on a declared document must name a
   declaration with the dynamic-flag equivalent (a declared argument
   is supplied through its `--<name>` flag; an undeclared flag fails
   as an unknown argument). Also rewrite the ~122-123 sentence
   "Declared documents inject no implicit headers; legacy documents
   keep raw send fields and header injection with one deprecation
   note on stderr": raw send fields on documents without `args:`
   stay, but there is no CLI header injection and no deprecation
   note anymore.
2. `CONTEXT.md` line ~128 exit-2 table row "Argument validation":
   drop `undeclared or missing-required --arg` (the undeclared-`--arg`
   class is gone), keep missing-required (dynamic flag absent),
   declaration/type-grammar, coercion, and `${arg:}`/`${env:}`
   unresolved classes.
3. Verify `crates/camel-cli/README.md` and `examples/` contain no
   `--arg` usage (grep; expected zero today — if a hit appears, update
   it to the dynamic-flag form).
4. Residual sweep across the zone:
   `grep -rn -- '--arg\|parse_arg_pair\|LEGACY_ARG\|CrossFormConflict\|
   UnknownArgumentName\|legacy_arg_headers\|cli_args'
   crates/camel-cli --include='*.rs' --include='*.md'` — hits are
   allowed ONLY as: (a) `tests/` artifact-rejection coverage and its
   comments, (b) `src/commands/job/flags_tests.rs` negative
   assertions and rejection invocations (the negative coverage task 1
   mandates). Everything else must be zero.
5. Byte-pin the missing-required remedy (r_glm minor, task-1 review):
   in `src/commands/job/tests.rs` `declared_args_validate_before_boot`,
   add one stderr assert `stderr.contains("pass --name <value>")`.
6. Polish (r_glm minor, task-1 review): in
   `src/commands/job/document_tests.rs`, drop the redundant
   `assert!(doc.args.is_some())` after the `.expect(...)` in
   `empty_args_select_declared_mode`, and rename
   `absent_args_select_legacy_mode` to
   `absent_args_select_raw_fields_mode`.
7. Run `cargo xtask lint-context-citations` (CONTEXT.md was edited).
8. Run `cargo test -p camel-cli --lib` (steps 5-6 touched src tests)
   and `openspec validate argdrop --type change`.

**Tests:**
- name: residual-surface-zero
  setup: tasks 1–2 complete
  action: run the step-4 grep
  assert: zero hits outside artifact-rejection comments
  command: (the grep in step 4)
  expected: zero.
- name: context-citations-clean
  action: `cargo xtask lint-context-citations`
  assert: exit 0
  command: `cargo xtask lint-context-citations`
  expected: exit 0.

**Acceptance:**
- Grep sweep zero outside the two documented exception classes.
- `cargo test -p camel-cli --lib` exits 0.
- `cargo xtask lint-context-citations` exits 0.
- `openspec validate argdrop --type change` reports 0 failed.

- [x] docs-context-and-residual-sweep
