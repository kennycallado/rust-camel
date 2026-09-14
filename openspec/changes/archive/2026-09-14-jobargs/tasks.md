# Tasks: jobargs

## Phase 1: Declare and parse string arguments

### camel-cli document parser

#### Task 1.1: Add strict top-level argument declarations

**Files:**
- `crates/camel-cli/src/commands/job/document.rs` (modified)
- `crates/camel-cli/src/commands/job/document_tests.rs` (modified)

**Steps:**
1. Add `JobArgumentDeclaration` and `JobArgumentDeclarations` to the raw and normalized job document models; accept only `required`, `default`, and `description`, with string defaults and identifier-constrained names.
2. Carry optional declarations through `JobDocument` without changing route-source or `execute:` parsing.
3. Return `JobDocError` with argument-specific diagnostics for invalid names, invalid types, and unknown per-argument fields.
4. Add parser tests for sibling placement, valid fields, malformed `requried`, invalid names, absent `args:`, and empty `args:` selecting declared mode.

**Tests:**
- `parse_declared_job_args`: arrange a `.job.yaml` with top-level `args` beside `execute`; act with `parse_job_document`; assert declarations preserve required/default/description; command `cargo test -p camel-cli commands::job::document_tests::parse_declared_job_args`; expected pass after implementation.
- `reject_unknown_argument_field`: arrange `requried: true`; act with parser; assert error names unknown field and caller maps it to exit 2; command `cargo test -p camel-cli commands::job::document_tests::reject_unknown_argument_field`; expected pass after implementation.
- `reject_invalid_argument_name`: arrange `customer-id` declaration; act with parser; assert argument-name diagnostic; command `cargo test -p camel-cli commands::job::document_tests::reject_invalid_argument_name`; expected pass after implementation.
- `empty_args_select_declared_mode`: arrange `args: {}` and a CLI pair; act with parser/model inspection; assert declarations are present and legacy-header mode is false; command `cargo test -p camel-cli commands::job::document_tests::empty_args_select_declared_mode`; expected pass after implementation.

**Acceptance:**
- `args:` is a top-level sibling of `execute:` and no declaration key is silently ignored.
- Argument names match `[A-Za-z_][A-Za-z0-9_]*`; values remain string-only.
- `cargo test -p camel-cli commands::job::document_tests` exits 0.

- [x] 1.1

## Phase 2: Generalize shared interpolation

### camel-dsl and camel-lint interpolation

#### Task 2.1: Resolve `arg:` through the shared interpolation scanner

**Files:**
- `crates/camel-dsl/src/env_interpolation.rs` (modified)
- `crates/camel-dsl/src/lib.rs` (modified)
- `crates/camel-lint/src/env_interpolation.rs` (modified)

**Steps:**
1. Generalize the existing token scanner and tree-walk lookup so `env:` retains its current behavior and `arg:` accepts only identifier names without `:-fallback`; dispatch namespace before lookup so `arg:` never falls through to ambient environment.
2. Preserve `$${env:NAME}` and `$$` escapes, sanitization, string typing, provenance behavior, and unresolved-name errors.
3. Expose one lookup-injectable interpolation seam usable by job and embedded runtime callers; do not add a registry or typed argument machinery.
4. Mirror the scanner change in camel-lint and add parity tests for direct, embedded, escaped, mixed, and unresolved tokens.

**Tests:**
- `interpolate_arg_tokens_at_shared_stage`: arrange lookup values for `env:HOST` and `arg:NAME`; act on URI/body/header-like strings; assert both namespaces resolve through one call path; command `cargo test -p camel-dsl env_interpolation`; expected pass after implementation.
- `reject_arg_fallback_syntax`: arrange `${arg:NAME:-fallback}`; act with interpolation; assert unresolved/invalid arg diagnostic rather than fallback; command `cargo test -p camel-dsl env_interpolation`; expected pass after implementation.
- `lint_scanner_matches_dsl_scanner`: arrange identical mixed tokens; act through both scanners; assert identical output and errors; command `cargo test -p camel-lint env_interpolation`; expected pass after implementation.
- `arg_namespace_does_not_fall_through`: arrange ambient `HOME` but no declared `HOME`; act on `${arg:HOME}`; assert unresolved argument error; command `cargo test -p camel-dsl env_interpolation`; expected pass after implementation.

**Acceptance:**
- `${env:}` behavior and all escape forms remain unchanged.
- `${arg:NAME}` and `${env:NAME}` use the same interpolation stage; fallback syntax for `arg:` is rejected.
- `cargo test -p camel-dsl env_interpolation` and `cargo test -p camel-lint env_interpolation` exit 0.

- [x] 2.1

## Phase 3: Validate and execute arguments

### camel-cli job execution

#### Task 3.1: Validate CLI arguments and preserve legacy headers

**Files:**
- `crates/camel-cli/src/commands/job/mod.rs` (modified)
- `crates/camel-cli/src/commands/job/document.rs` (modified)
- `crates/camel-cli/src/commands/job/tests.rs` (modified)

**Steps:**
1. Add pure pair-resolution/default/validation functions in `document.rs`; have `mod.rs` resolve repeatable `--arg NAME=VALUE` pairs against declarations before boot, using deterministic last-value semantics, and reject unknown names and missing required values with exit-2 diagnostics naming the argument.
2. Apply declaration defaults and pass resolved values to the shared interpolation seam before validating `to`, `body`, headers, and timeout; keep declared interpolation values out of implicit headers.
3. For documents without `args:`, retain header injection after document headers and emit one deprecation note; keep raw values and batch behavior.
4. Keep the existing setup_booted_job/EarlyJobFailure teardown border intact on all validation and boot failures.
5. Add tests for unknown, missing, default, explicit-over-default, all four field positions, legacy headers, deprecation output, and exit 2.

**Tests:**
- `declared_args_validate_before_boot`: arrange declared `name` and `tier`; act with unknown `other` or omit required `name`; assert exit 2 and named diagnostic before boot; command `cargo test -p camel-cli commands::job::tests::declared_args_validate_before_boot`; expected pass after implementation.
- `declared_defaults_and_explicit_values`: arrange optional default and explicit `--arg`; act through execution; assert default applies and explicit value wins; command `cargo test -p camel-cli commands::job::tests::declared_defaults_and_explicit_values`; expected pass after implementation.
- `legacy_args_remain_headers_with_deprecation`: arrange no `args:` and document header collision; act with repeated `--arg`; assert last raw header wins and stderr contains deprecation note; command `cargo test -p camel-cli commands::job::tests::legacy_args_remain_headers_with_deprecation`; expected pass after implementation.
- `declared_arg_is_not_implicit_header`: arrange a declared `name` used in body and a route that records headers; act with `--arg name=John`; assert body is `John` and recorded headers do not contain `name`; command `cargo test -p camel-cli commands::job::tests::declared_arg_is_not_implicit_header`; expected pass after implementation.

**Acceptance:**
- Declared validation occurs before boot and all validation failures return exit 2.
- `${arg:}` resolves in `to`, `body`, `headers`, and `timeout` before field validation.
- Legacy no-`args:` documents preserve header behavior with explicit deprecation output.
- `cargo test -p camel-cli commands::job::tests` exits 0.

- [x] 3.1

### compiled artifact runtime

#### Task 3.2: Apply embedded defaults without widening artifact arguments

**Files:**
- `crates/camel-cli/src/compile/runtime.rs` (modified)
- `crates/camel-cli/src/compile/policy.rs` (modified)
- `crates/camel-cli/src/compile/manifest.rs` (modified)
- `crates/camel-cli/tests/compiled_artifact_test.rs` (modified)

**Steps:**
1. Feed embedded job declarations through the same namespace-specific runtime interpolation lookup used by normal jobs, using embedded defaults only.
2. Reject required declarations without defaults at artifact startup with exit 2 and keep `ArtifactArgs` rejecting `--arg` as an unknown argument.
3. Ensure compile-time asset policy continues to permit job declarations while rejecting unsupported route assets.
4. Keep the manifest env-name scan an env-only subset of the shared interpolation grammar (jobargs Task 2.1): bare `${arg:NAME}` and escaped `$${arg:NAME}` tokens never enter `env_names`, and the SYNC comment tracks the jobargs-generalized camel-dsl `env_regex()`.
5. Add artifact tests for default resolution parity, required-without-default rejection, `--arg` rejection, and identical field interpolation.

**Tests:**
- `compiled_job_uses_declared_default`: arrange artifact with `value: {default: hello}` and `${arg:value}`; act by running artifact; assert same route/message value as normal job and exit 0; command `cargo test -p camel-cli --test compiled_artifact_test compiled_job_uses_declared_default`; expected pass after implementation.
- `compiled_job_rejects_required_without_default`: arrange embedded required declaration without default; act by starting artifact; assert named exit-2 diagnostic; command `cargo test -p camel-cli --test compiled_artifact_test compiled_job_rejects_required_without_default`; expected pass after implementation.
- `compiled_job_rejects_arg_flag`: arrange valid artifact; act with `--arg value=other`; assert existing unknown-argument exit 2; command `cargo test -p camel-cli --test compiled_artifact_test compiled_job_rejects_arg_flag`; expected pass after implementation.
- `manifest_env_scan_ignores_arg_tokens`: arrange a document embedding `${arg:value}` and `$${arg:lit}` beside `${env:HOST}`; act with `manifest::derive`; assert `env_names` is exactly `HOST` (arg tokens contribute nothing); command `cargo test -p camel-cli --lib compile::manifest::manifest_env_scan_ignores_arg_tokens`; expected pass after implementation.

**Acceptance:**
- Artifact payload remains pre-interpolation and uses the shared runtime resolver.
- Artifact CLI surface remains `--report`, `--help`, `--version`, and `--manifest`.
- `cargo test -p camel-cli --test compiled_artifact_test` exits 0.

Deviation note: implementation also touched `crates/camel-cli/src/commands/job/mod.rs`
(the embedded parse seam `run_embedded_job`) and `crates/camel-cli/src/commands/job/document.rs`
(`parse_job_document` retained as the pure-grammar seam) — step 1 was not
implementable without them; flagged by the Task 3.2 spec review.

- [x] 3.2

## Phase 4: Contract and regression coverage

### Specifications and context

#### Task 4.1: Align job contract and context documentation

**Files:**
- `openspec/changes/jobargs/specs/cli-jobs/spec.md` (modified)
- `crates/camel-cli/CONTEXT.md` (modified)
- `CONTEXT-MAP.md` (modified)
- `crates/camel-cli/tests/job_one_shot_test.rs` (modified)

**Steps:**
1. Add cross-path end-to-end fixtures covering declared interpolation in `to`, `body`, `headers`, and `timeout`, plus legacy no-`args:` behavior in one-shot and batch modes.
2. Update camel-cli context and the cross-cutting context map with the final declared-argument vocabulary, legacy deprecation note, identifier grammar, and artifact narrow-surface boundary.
3. Keep the delta spec scenarios aligned with the implemented error text and exit-2 taxonomy.
4. Run schema and context citation checks and resolve only findings within the jobargs scope.

**Tests:**
- `job_args_end_to_end_field_matrix`: arrange a job fixture with four declared values and a mock route; act in one-shot and batch modes; assert interpolated values, successful drain, and exit 0; command `cargo test -p camel-cli --test job_one_shot_test job_args_end_to_end_field_matrix`; expected pass after implementation.
- `job_args_validation_exit_two`: arrange unknown and missing-required invocations; act through CLI subprocess; assert each exits 2 and names the offending argument; command `cargo test -p camel-cli --test job_one_shot_test job_args_validation_exit_two`; expected pass after implementation.
- `job_args_interpolation_failure_exit_two`: arrange a declared job referencing `${arg:ghost}` without declaration; act through CLI subprocess; assert exit 2 and named unresolved-argument diagnostic; command `cargo test -p camel-cli --test job_one_shot_test job_args_interpolation_failure_exit_two`; expected pass after implementation.

**Acceptance:**
- Every added and modified spec scenario has executable coverage.
- `cargo xtask schema --check` and `cargo xtask lint-context-citations` exit 0.
- Context prose states artifact `--arg` remains unsupported and legacy header behavior is deprecated.

- [x] 4.1
