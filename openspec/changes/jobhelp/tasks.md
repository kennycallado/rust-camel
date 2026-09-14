# Tasks: jobhelp

## camel-cli

### Task 1.1: projection parser `parse_job_document_for_help`

**Files:**
- `crates/camel-cli/src/commands/job/document.rs` (modified)
- `crates/camel-cli/src/commands/job/document_tests.rs` (modified)

**Steps:**
1. Add `pub(crate) struct JobHelpInfo` in `document.rs` with three fields: `mode: String` (the validated mode spelling, `"one-shot"` or `"batch"`), `send_to: String` (the raw `execute.send.to` text, unvalidated and uninterpolated), and `args: Option<JobArgumentDeclarations>` (the normalized declarations).
2. Add `pub(crate) fn parse_job_document_for_help(path: &Path, text: &str) -> Result<JobHelpInfo, JobDocError>` in `document.rs`. It repeats the structural prefix of `parse_job_document_impl` verbatim: the `camel_dsl::discovery::is_job_document` suffix check, the serde_yaml value parse, the `execute` presence check (`MissingExecute`), the `scenario` exclusivity check (`ExclusiveWithScenario`), the test-vocabulary check (`MixedVocabulary`), the strict `JobDocumentDoc` serde parse with `classify`, the route-source conflict check (`RouteSource`), `normalize_job_args(raw.args.take())?`, the mode spelling match (`MissingMode`/`UnsupportedMode`), `execute.send` presence (the explicit `JobDocError::Yaml("execute.send is required: exactly one send action")` check — `send` is `Option` with `#[serde(default)]`, so serde does not enforce it), and `execute.timeout` presence (`MissingTimeout`) WITHOUT parsing the duration. It does NOT run the `JOB_SEND_SCHEMES` scheme check. It returns `JobHelpInfo { mode, send_to, args }`. It performs NO pair resolution, NO defaulting, NO interpolation, and NO execution-value validation. Reuse `JobDocError` variants only; add none.
3. Do not modify `parse_job_document`, `parse_job_document_with_args`, or `parse_job_document_impl` — the projection parser is a sibling that shares their helpers, not a parameterization of them.

**Tests:** (in `document_tests.rs`; command `cargo test -p camel-cli commands::job::document_tests`)
- `help_parse_accepts_arg_tokens_in_to_and_timeout`: setup = a `*.job.yaml` text declaring `args: { target: { required: true } }` with `to: "${arg:target}"` and `timeout: "${arg:wait}"`; action = `parse_job_document_for_help(path, text)`; assert = `Ok` with `send_to == "${arg:target}"`, `mode == "one-shot"`, and `args` carrying the `target` declaration with `required == true`. Expected: fails before step 2 exists, passes after.
- `help_parse_accepts_unsatisfiable_required_argument`: setup = a text declaring a `required: true` argument with no default and no `--arg` machinery available; action = `parse_job_document_for_help`; assert = `Ok` (no `MissingRequiredArgument` can fire). Expected: fails before, passes after.
- `help_parse_rejects_structural_errors`: setup = three texts — one with an unknown top-level field, one with an unknown field inside an argument declaration (`requried:`), one missing `execute:`; action = `parse_job_document_for_help` on each; assert = `Err` with the same `JobDocError` variants the execution parser produces (`classify`d unknown-field, `UnknownArgumentField`, `MissingExecute`). Expected: fails before, passes after.
- `help_parse_requires_send_and_timeout_presence`: setup = two texts — one with no `execute.send`, one with no `execute.timeout`; action = `parse_job_document_for_help` on each; assert = `Err(Yaml("execute.send is required: exactly one send action"))` and `Err(MissingTimeout)` respectively. Expected: fails before, passes after.

**Acceptance:**
- `cargo test -p camel-cli commands::job::document_tests` passes.
- `cargo clippy -p camel-cli -- -D warnings` exits 0.
- No new `JobDocError` variant (verifiable by diff: only `JobHelpInfo` and `parse_job_document_for_help` added to `document.rs`).

- [x] 1.1

### Task 1.2: renderer `render_job_help` and description probe

**Files:**
- `crates/camel-cli/src/commands/job/help.rs` (new)
- `crates/camel-cli/src/commands/job/help_tests.rs` (new)
- `crates/camel-cli/src/commands/job/mod.rs` (modified: `mod help;`, `#[cfg(test)] mod help_tests;`, refactor `probe_description` to delegate)

**Steps:**
1. Create `help.rs` with `pub(crate) fn render_job_help(stem: &str, description: Option<&str>, info: &JobHelpInfo) -> String` — a pure function, no I/O. Output, in order: line 1 = `stem`; blank line; line 3 = `description` or `(no description)` when `None`; blank line; `Mode:` line; `Sends to:` line with `info.send_to` verbatim; blank line; `Arguments:` header; then either one row per declared argument or the single row `  (no arguments)`.
2. Label alignment: `Mode:` and `Sends to:` are each padded so the value starts at column 12 (label + trailing spaces; `Mode:` gets 6 trailing spaces, `Sends to:` gets 2). Example lines: `Mode:      one-shot`, `Sends to:  direct:ingest`.
 3. Argument rows: two leading spaces, the argument name padded on the right to the widest argument name in the table, then exactly two spaces, then `string`, then two spaces, then `required` or `optional`, then `  default=<value>` when the declaration has a default, then `  <description>` when the declaration has a description. Rows iterate `info.args` entries in `BTreeMap` order (lexical by name). When `info.args` is `None` OR its `entries` map is empty, print exactly `  (no arguments)` as the only row. Each argument always renders on one row: a maximal CR/LF run inside a `default` value or `description` collapses to one space. This algorithm supersedes the illustrative spacing in `design.md`.
4. Refactor `probe_description` in `mod.rs`: extract `fn probe_description_str(text: &str) -> Option<Option<String>>` containing the existing `JobListProbe` serde call; `probe_description(path)` reads the file then delegates. The help path calls `probe_description_str` on the in-memory text.
5. Wire `mod help;` and `#[cfg(test)] mod help_tests;` into `mod.rs`.

**Tests:** (in `help_tests.rs`; command `cargo test -p camel-cli commands::job::help_tests`)
- `render_full_declared_interface`: setup = `JobHelpInfo` with mode `one-shot`, `send_to` `direct:ingest`, and declarations `feed` (required, description `Feed identifier`) and `region` (optional, default `eu-west-1`, no description); action = `render_job_help("daily-sync", Some("Ingest the daily feed"), &info)`; assert = exact multi-line string equality with the full pinned block (stem, description, `Mode:      one-shot`, `Sends to:  direct:ingest`, `Arguments:`, both rows in lexical order — `feed` before `region` — with name column padded to `region`'s width). Expected: fails before, passes after.
- `render_no_description_placeholder`: same info with `description = None`; assert = line 3 is exactly `(no description)`. Expected: fails before, passes after.
- `render_absent_and_empty_args_print_no_arguments`: two infos — `args: None` and `args: Some` with empty `entries`; assert = both render `Arguments:` followed by exactly `  (no arguments)`, and both outputs are identical. Expected: fails before, passes after.
- `render_multiline_values_collapse_to_one_row`: setup = a declaration with `default` containing `a\nb` and `description` containing `c\r\nd`; assert = the row is a single line containing `default=a b` and `c d` with no CR or LF. Expected: fails before, passes after.
- `render_token_send_target_verbatim`: setup = `send_to = "${arg:target}"`; assert = the line is exactly `Sends to:  ${arg:target}`. Expected: fails before, passes after.

**Acceptance:**
- `cargo test -p camel-cli commands::job::help_tests` passes.
- `cargo test -p camel-cli commands::job::tests` still passes (the `probe_description` refactor is behavior-preserving).
- `cargo clippy -p camel-cli -- -D warnings` exits 0.

- [x] 1.2

### Task 1.3: clap surface and `run_job` wiring

**Files:**
- `crates/camel-cli/src/commands/job/mod.rs` (modified)
- `crates/camel-cli/src/commands/job/tests.rs` (modified)

**Steps:**
1. On the `JobArgs` struct in `mod.rs`, add the container attribute `#[command(disable_help_flag = true)]` so clap stops auto-handling `--help`/`-h` for the `job` subcommand only (top-level `camel --help` and all other subcommands keep clap help). Add the field `#[arg(long = "help", short = 'h', action = clap::ArgAction::SetTrue)] pub help: bool` with a doc comment stating: with a job name it renders the job's declared interface; without a name it prints `camel job` usage.
2. In `run_job`, change the signal-arming predicate from `args.document.is_some()` to `args.document.is_some() && !args.help`, and gate the `CAMEL_JOB_SIGNAL_MARKER` stderr marker on the same `signals.is_some()` expression it already uses (help installs no signal streams). Then RESTRUCTURE the dispatch binding: the current `let (Some(raw_document), Some(signals)) = (&args.document, signals) else { ... }` tuple pattern breaks when a document is present but signals are not (the help case) — it would route help-with-name into the listing branch. Replace it with `let Some(raw_document) = &args.document else { ...no-document path... };` keeping `signals` as the `Option<JobSignals>` it already is and passing it through to `execute_job` (which already accepts `Option`); update the stale invariant comment above the binding ("both arms key off the same predicate") to state the new invariant: the no-document else branch is the usage/listing path, and the document branch internally splits help vs execution on `args.help`.
3. In `run_job`'s no-document branch, check `args.help` BEFORE the `--report requires a job document` guard: when `help` is set, print `JobArgs::augment_args(clap::Command::new("camel job")).render_help().to_string()` to stdout and return 0; then keep the existing `--report` guard and the A1 `list_jobs` return unchanged.
4. Extract a shared stem helper `fn job_stem(name: &str) -> &str` in `mod.rs` that strips the `.job.yaml`/`.job.yml` suffix (falling back to the full name), and refactor the A1 listing's inline `strip_suffix` chain (mod.rs, the `let stem = name...` site) to call it. In `run_job`'s document branch, immediately after reading `text` (and before `parse_job_document_with_args`), add: when `args.help` is set, call `parse_job_document_for_help(&document_path, &text)`; on `Err(e)` print `{document_path.display()}: {e}` to stderr (the same pattern the execution path uses) and return 2; on `Ok(info)` call `job_stem` on the resolved file name, call `probe_description_str(&text)`, print `render_job_help(stem, description.flatten().as_deref(), &info)` to stdout, and return 0. The help path returns before any boot, report write, or pair validation. When `args.help` is NOT set, the existing execution path runs unchanged (including `--report` handling).
5. Extend the test harness in `tests.rs`: add an env-capable variant `run_camel_job_env(dir, args, env: &[(std::ffi::OsString, std::ffi::OsString)]) -> (i32, String, String)` that mirrors `run_camel_job` but sets `.envs()` on the child (the existing `run_camel_job` cannot inject environment); add a fixture writer that creates a `jobs/` directory with one `*.job.yaml` document (bare-name resolution resolves `<root>/jobs/<name>.job.yaml` through the default `[jobs]` config). The jobargs fixtures use explicit paths, so these two additions are new; reuse `write_job_fixture_config` and the existing tap-route writers where the fixture needs a valid route source.

**Tests:** (in `tests.rs`; command `cargo test -p camel-cli commands::job::tests`)
- `help_with_name_renders_declared_interface`: setup = fixture with a jobs root containing `daily-sync.job.yaml` that has a description (`Ingest the daily feed`), declares `args:` (one required with description, one optional with default), and has a valid `execute:`; action = `run_camel_job(dir, &["daily-sync", "--help"])`; assert = exit 0, stdout starts with `daily-sync`, contains `Ingest the daily feed`, `Mode:      one-shot`, `Sends to:`, both argument rows, and contains no clap `Usage:` line. Expected: fails before, passes after.
- `help_with_name_reports_required_args_without_pairs`: setup = same fixture shape but `to: "${arg:target}"`, `target` required without default; action = `run_camel_job(dir, &["daily-sync", "--help"])` with no `--arg`; assert = exit 0 and stdout contains `Sends to:  ${arg:target}`. Expected: fails before, passes after.
- `help_no_args_block_prints_no_arguments`: setup = fixture job without `args:`; action = `--help`; assert = exit 0 and stdout contains `Arguments:` followed by `  (no arguments)`. Expected: fails before, passes after.
- `help_writes_no_report_and_boots_nothing`: setup = fixture job (valid `execute:`), `CAMEL_JOB_SIGNAL_MARKER=1` injected through `run_camel_job_env`, and a report path argument; action = run `daily-sync --help --report out.json` with the env set; assert = exit 0, the report file does not exist, stdout is the interface, and stderr does not contain `signal streams armed`. Expected: fails before, passes after.
- `help_unknown_name_fails_loud`: setup = fixture with a jobs root that contains no matching document; action = `run_camel_job(dir, &["ghost", "--help"])`; assert = exit 2, stderr contains `ghost` and carries the existing bare-name resolution diagnostic, and stderr is not clap help output. Expected: fails before, passes after.
- `help_malformed_document_fails_loud`: setup = fixture `broken.job.yaml` with an unknown top-level field; action = `--help`; assert = exit 2 and stderr carries the parse diagnostic, not clap help. Expected: fails before, passes after.
- `help_without_name_prints_usage`: setup = any fixture dir; action = `run_camel_job(dir, &["--help"])`; assert = exit 0 and stdout contains `Usage: camel job`; then `run_camel_job(dir, &[])` (no flags) still prints the discovery listing (exit 0). Expected: fails before, passes after.
- `help_short_flag_behaves_like_long`: setup = the declared-interface fixture; action = `run_camel_job(dir, &["daily-sync", "-h"])`; assert = exit 0 and same stdout shape as the `--help` run. Expected: fails before, passes after.

**Acceptance:**
- `cargo test -p camel-cli commands::job::tests` passes.
- `cargo test -p camel-cli --lib` passes (no regression outside the job module).
- `cargo clippy -p camel-cli -- -D warnings` exits 0.
- `cargo fmt --check` on the touched files exits 0 (run `cargo fmt -p camel-cli` before checking).
- Bare `camel job <name>` (no `--help`) execution behavior is unchanged: the existing jobargs and one-shot tests pass without modification.

- [x] 1.3
