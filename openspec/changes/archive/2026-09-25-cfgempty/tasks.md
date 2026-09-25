# Tasks: cfgempty

## Task 1 — Empty `--config` guard in `load_config_or_default` (run + job)

- [x] 1.1

**Files**:

- `crates/camel-cli/src/commands/run.rs` (guard in `load_config_or_default`)
- `crates/camel-cli/src/commands/run_tests.rs` (unit tests)

**Steps**:

1. In `load_config_or_default`, add the empty-value guard as the first
   statement, mirroring the existing `CamelError::Config` error paths
   (design D1):

   ```rust
   if config_path.is_empty() {
       return Err(camel_api::CamelError::Config(
           "--config requires a non-empty path; pass a Camel.toml path or omit the flag"
               .to_string(),
       ));
   }
   ```

2. Update the doc comment on the function so the empty-value rejection
   is stated next to the absent-file fallback rule.

**Tests** (in `run_tests.rs`, following the file's existing style):

1. `load_config_or_default_empty_path_errors_naming_config_flag`
   - **Arrange**: nothing on disk; call `load_config_or_default("")`.
   - **Act**: match on the `Result`.
   - **Assert**: `Err(CamelError::Config(msg))` where `msg` contains
     `--config` and `non-empty` (covers the flag, tail-override, and
     `CAMEL_CONFIG_FILE=""` spellings, which all feed this function).
2. Confirm (and only add if genuinely absent) an existing test
   witnessing the absent-non-empty-file default fallback and an
   existing-file load — these prove "omitted = default" and "valid path
   unchanged" stay intact. If either witness is missing, add
   `load_config_or_default_missing_file_yields_default` /
   `load_config_or_default_existing_file_loads` in the same style.

**Acceptance**:

- `cargo test -p camel-cli --lib load_config_or_default` green, with the
  new empty-path test present and asserting the message names
  `--config`.
- Existing tests untouched except where the doc comment asks for the
  guard note.

## Task 2 — Same guard at `camel compile` selection boundary + sweep

- [x] 2.1

**Files**:

- `crates/camel-cli/src/commands/compile.rs` (empty-value rejection
  before `sources::resolve`, via the existing `eprintln!` +
  `EXIT_REJECTION` path, message identical to Task 1)
- `crates/camel-cli/src/commands/compile.rs` test module (or the
  compile command's existing test file) for the unit test

**Steps**:

1. After the `args.config.is_none()` ambient-Camel.toml check and
   before constructing `SourceSelection`, reject
   `args.config.as_deref() == Some("")` with the same message naming
   `--config`, printing `camel compile: --config requires a non-empty
   path; pass a Camel.toml path or omit the flag` and returning
   `EXIT_REJECTION` (exit 2) — mirrors the surrounding error style.
2. Sweep the adjacent path/URI-taking flags on the same subcommands for
   the identical empty-value class (document findings in the test
   comments or park notes; only fix cases that are BOTH silent on empty
   AND trivially guardable at the same boundary — otherwise note for
   park): `camel run --routes=`, `camel run --otel-endpoint=`,
   `camel job --report=`.

**Tests**:

1. `compile_config_empty_value_is_named_rejection_exit_2`
   (in-process unit test, compile.rs `empty_config_tests`):
   - **Arrange**: `CompileArgs` with `config: Some(PathBuf::from(""))`
     and a `.yaml` document (the guard fires before any file read).
   - **Act**: `run_compile(&args)`.
   - **Assert**: returns 2 (`EXIT_REJECTION`).
   - Note (recorded per r_glm finding): the CLI cannot reach this
     guard (clap `PathBufValueParser` rejects empty values first,
     naming the flag, exit 2 — verified empirically), and
     `run_compile` prints via `eprintln!` with no capture seam, so
     the in-process assertion is the exit code alone; the stderr
     text is asserted at the clap boundary by the spawn-based
     `compile_command_test` suite only insofar as clap's own
     message names the flag.
2. For each swept flag, one-line documentation of observed empty-value
   behavior (loud/silent) in the park report — no test required when
   behavior is loud or out of class.

**Acceptance**:

- `cargo test -p camel-cli --lib compile` green with the new test.
- `camel compile --config=` exits 2 with the flag-naming message (the
  old `MissingSource("")` diagnostic no longer reachable for the empty
  value).

## Task 3 — Gates + verification evidence

- [x] 3.1

**Files**: none (verification only).

**Steps/Tests** (each command run from the worktree root, observed exit
codes recorded in the park report):

1. `cargo fmt --check --all`
2. clippy leg 1: `cargo clippy --workspace --all-features --exclude
   camel-cli --exclude camel-component-kafka --exclude security-keycloak
   --exclude security-wasm-policy -- -D warnings` (workspace leg,
   unchanged by this diff — rerun anyway per gate list)
3. clippy leg 2: `cargo clippy -p camel-component-kafka --all-targets -- -D warnings`
4. clippy leg 3: `cargo clippy -p camel-cli -- -D warnings` and
   `cargo clippy -p camel-cli --no-default-features --features
   flavor-regular,exec --all-targets -- -D warnings`
5. `cargo test -p camel-cli --lib`
6. Manual repro pair (documented verbatim in park report): before-fix
   `camel job --config=` behavior vs after-fix error text; confirm
   `camel job` (omitted) and `camel job --config <valid>` unchanged.

**Acceptance**: all gates exit 0; repro evidence recorded.
