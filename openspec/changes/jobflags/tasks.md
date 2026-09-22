# Tasks: jobflags

## camel-cli job module

### Task 1.1: reserved-argument-name guard in document.rs

**Files:**
- `crates/camel-cli/src/commands/job/document.rs` (modified)
- `crates/camel-cli/src/commands/job/document_tests.rs` (modified)

**Steps:**
1. Add to `JobDocError` (document.rs, after `UnknownArgumentField`):
   `ReservedArgumentName { name: String }` with doc comment "A
   top-level `args:` name collides with a static job-subcommand flag
   (`help`, `config`, `report`, `arg`)."
2. Add private const in document.rs:
   `const RESERVED_ARGUMENT_NAMES: [&str; 4] = ["help", "config", "report", "arg"];`
3. In `normalize_job_args`, immediately after the
   `is_argument_identifier` check passes and before
   `job_argument_declaration`, reject reserved names:
   `if RESERVED_ARGUMENT_NAMES.contains(&name.as_str()) { return
   Err(JobDocError::ReservedArgumentName { name }); }` — this runs in
   execution parsing, `--help` parsing, and
   `validate_job_declarations_for_compile` because all three share
   `normalize_job_args`.
4. Add the `thiserror` Display arm (match the enum's existing style):
   `argument name '{name}' is reserved by the job command's --{name} flag`.

**Tests:** (executable spec — TDD: write every test in this block FIRST, verify it FAILS red against the unmodified code, then implement to green)
- `reserved_argument_name_rejected_all_four`: for each of `help`,
  `config`, `report`, `arg`, build a document text declaring
  `args: {<name>: {required: true}}` around `VALID_ONE_SHOT`-style
  execute section → call `document::parse_job_document_with_args(&doc_path(), &text, &[])` →
  assert `Err(JobDocError::ReservedArgumentName { name })` and the
  Display contains `reserved` and the name. Command:
  `cargo test -p camel-cli --lib reserved_argument_name_rejected_all_four`. Expected: fails before steps 1-3, passes after.
- `reserved_argument_name_rejected_on_help_parse`: same document for
  `report` → `document::parse_job_document_for_help(&doc_path(), &text)` →
  assert the same error variant (both parse paths reject identically).
  Command: `cargo test -p camel-cli --lib reserved_argument_name_rejected_on_help_parse`.
- `non_reserved_flag_like_name_accepted`: document declaring
  `args: {helpers: {required: true}}` (a name that merely CONTAINS a
  reserved word) → parse succeeds; proves the guard is exact-match
  only. Command:
  `cargo test -p camel-cli --lib non_reserved_flag_like_name_accepted`.
- `reserved_argument_name_rejected_at_compile_validation`: call
  `document::validate_job_declarations_for_compile` on a text with
  `args: {config: {}}` → assert `Err` matching
  `JobDocError::ReservedArgumentName`. Command:
  `cargo test -p camel-cli --lib reserved_argument_name_rejected_at_compile_validation`.

**Acceptance:**
- `cargo test -p camel-cli --lib document_tests` exits 0.
- `cargo clippy -p camel-cli -- -D warnings` exits 0.
- `cargo fmt --check` exits 0.

- [x] 1.1

### Task 1.2: phase-1 trailing capture + tail pre-scans (`--config`, help)

**Files:**
- `crates/camel-cli/src/commands/job/mod.rs` (modified)
- `crates/camel-cli/src/commands/job/tests.rs` (modified)

**Steps:**
1. Add the LAST field to `JobArgs` (after `args`), with doc comment
   "Raw trailing tokens after the document reference (phase-1 capture
   for document-derived dynamic flags; empty for invocations without
   them). Pinned clap 4.x semantics: static flags after the document
   parse statically until the first unknown token; from there
   everything is captured raw (later static flags included) and
   recovered by the tail re-parse; a flag before the document is a
   phase-1 unknown-argument error":
   ```rust
   #[arg(
       trailing_var_arg = true,
       allow_hyphen_values = true,
       num_args = 0..,
       value_name = "DYNAMIC"
   )]
   pub dynamic: Vec<std::ffi::OsString>,
   ```
2. Add private fn in mod.rs `fn tail_config_override(dynamic: &[std::ffi::OsString]) -> Option<String>`:
   exact-token scan (NOT a tokenizer — exact matches only) over the
   tail returning the LAST match, where a match is a token equal to
   `--config` (value = next token, if any) or starting with
   `--config=` (value = remainder after `=`). Non-UTF-8 tokens are
   skipped (no panic; config paths must be UTF-8 or the config loader
   fails loudly on its own).
3. Add private fn `fn tail_has_help(dynamic: &[std::ffi::OsString]) -> bool`:
   exact-token scan for a token equal to `--help` or `-h`.
4. In `run_job`, compute
   `let config_path = tail_config_override(&args.dynamic).unwrap_or_else(|| args.config.clone());`
   BEFORE `JobSignals` usage decisions and before
   `load_config_or_default`, and use `config_path` at ALL THREE
   `args.config` sites: the config load, the
   `try_canonical_project_root(&args.config)` call inside
   `jobs_roots` (thread the resolved path into `jobs_roots` — the
   jobs-root anchor must follow the effective config, not the
   phase-1 spelling), and `canonical_project_root` building
   `JobRun.project_root` (argv last-wins: a tail `--config` overrides
   the phase-1 value, matching clap's own last-wins for repeats).
5. Extend the signal-arm gate to
   `(args.document.is_some() && !args.help && !tail_has_help(&args.dynamic)).then(JobSignals::arm)`
   — a tail `--help` must NOT leave signal streams armed on the help
   path (the spec's help contract: no signal handlers installed).

**Tests:** (executable spec — pin the probe-frozen clap semantics against the REAL `Cli` type; place next to the existing jobargs family in tests.rs. TDD: red first, then green)
- `phase1_flags_after_path_land_in_dynamic_tail`: build
  `crate::Cli::try_parse_from(["camel", "job", "doc.job.yaml", "--name", "world", "--name=flat"])`
  → assert `dynamic == ["--name", "world", "--name=flat"]` and
  `document == Some("doc.job.yaml")`. Command:
  `cargo test -p camel-cli --lib phase1_flags_after_path_land_in_dynamic_tail`.
- `phase1_static_flags_after_path_parse_statically`:
  `["camel", "job", "doc.job.yaml", "--arg", "name=x", "--config", "c.toml", "--report", "r.json", "--help"]`
  → assert `args == [("name","x")]`, `config == "c.toml"`,
  `report == Some("r.json")`, `help == true`, `dynamic` empty.
  Command: `cargo test -p camel-cli --lib phase1_static_flags_after_path_parse_statically`.
- `phase1_tail_starts_at_first_unknown_token`:
  `["camel", "job", "doc.job.yaml", "--arg", "a=1", "--name", "w", "--report", "r.json"]`
  → assert `args == [("a","1")]` and
  `dynamic == ["--name", "w", "--report", "r.json"]` (later static
  flags are captured raw). Command:
  `cargo test -p camel-cli --lib phase1_tail_starts_at_first_unknown_token`.
- `phase1_flag_before_document_is_unknown_argument`:
  `["camel", "job", "--name", "world", "doc.job.yaml"]` → assert Err
  with `clap::error::ErrorKind::UnknownArgument`. Command:
  `cargo test -p camel-cli --lib phase1_flag_before_document_is_unknown_argument`.
- `phase1_bare_and_doc_only_unchanged`: `["camel", "job"]` parses
  with `document == None`, `dynamic` empty; `["camel", "job", "doc.job.yaml"]`
  parses with empty `dynamic`. Command:
  `cargo test -p camel-cli --lib phase1_bare_and_doc_only_unchanged`.
- `tail_config_override_last_wins`: `tail_config_override` on
  `["--name", "w", "--config", "b.toml", "--config=a.toml"]` →
  `Some("a.toml")`; with no config tokens → `None`. Command:
  `cargo test -p camel-cli --lib tail_config_override_last_wins`.
- `tail_has_help_exact_token`: `tail_has_help` on `["--name", "x", "--help"]`
  → true; on `["--name", "--helpp"]` → false; on `["-h"]` → true. Command:
  `cargo test -p camel-cli --lib tail_has_help_exact_token`.

**Acceptance:**
- `cargo test -p camel-cli --lib -- phase1 tail_config tail_has` exits 0 (all seven).
- Existing tests still pass: `cargo test -p camel-cli --lib job::` exits 0.
- `cargo clippy -p camel-cli -- -D warnings` and `cargo fmt --check` exit 0.

- [x] 1.2

### Task 1.3: `lower_dynamic_flags` core engine (dynamic args + bool semantics)

**Files:**
- `crates/camel-cli/src/commands/job/mod.rs` (modified)
- `crates/camel-cli/src/commands/job/flags_tests.rs` (new; wire with
  `#[cfg(test)] mod flags_tests;` next to the existing test mods)

**Steps:**
1. Define in mod.rs (private, above `run_job`):
   ```rust
   struct LoweredDynamic {
       pairs: Vec<(String, String)>,   // merged arg-pairs + dynamic pairs (last-wins per key at resolution)
       help: bool,                     // tail --help/-h recovered
       report: Option<PathBuf>,        // tail --report recovered (last-wins vs phase-1 at the caller)
   }
   enum DynamicFlagError {
       BoolFlagValue { name: String },        // --flag=... / --no-flag=... value form on a declared bool
       ContradictoryBool { name: String },    // --flag AND --no-flag in one invocation
       CrossFormConflict { name: String },    // key given via --name AND --arg
       UndeclaredDocument { flag: String },   // dynamic flag but the document declares no args: block
       UnexpectedPositional { token: String },// stray positional token in the tail
       Clap(String),                          // clap-rendered diagnostic (unknown flag, malformed --arg, ...)
   }
   ```
   with `impl Display` producing exactly:
   - BoolFlagValue: `boolean flag '--{name}' takes no value; use '--{name}' for true, '--no-{name}' for false, or '--arg {name}=false'`
   - ContradictoryBool: `argument '{name}' given as both '--{name}' and '--no-{name}'`
   - CrossFormConflict: `argument '{name}' given through both '--{name}' and '--arg {name}=VALUE`
   - UndeclaredDocument: `dynamic flag '{flag}' requires the document to declare an 'args:' block; declare the argument or use '--arg {flag}=VALUE'`
   - UnexpectedPositional: `unexpected positional '{token}' after the job document`
   - Clap(s): `{s}` verbatim (clap's own rendered error)
   (CrossFormConflict and UndeclaredDocument branches are exercised by
   Task 1.4's tests; implement them in this task so the enum is
   complete and Display-pinned here.)
2. Implement `fn lower_dynamic_flags(declarations: Option<&document::JobArgumentDeclarations>, dynamic: &[std::ffi::OsString], arg_pairs: &[(String, String)]) -> Result<LoweredDynamic, DynamicFlagError>`:
   a. Empty tail: return `LoweredDynamic { pairs: arg_pairs.to_vec(), help: false, report: None }`.
   b. Help precedence pre-scan (mirror of `tail_has_help`, inlined or
      reused): if any tail token equals `--help` or `-h`, return
      `LoweredDynamic { pairs: arg_pairs.to_vec(), help: true, report: None }`
      WITHOUT further validation (help path ignores pair/flag
      validation, matching today's `--arg` behavior on the help branch).
   c. Bool value-form pre-scan: for each declared bool argument `b`,
      if any tail token starts with `--{b}=` or `--no-{b}=` (exact
      prefix, name spelled verbatim), return
      `Err(BoolFlagValue { name: b })`.
   d. Build the phase-2 command:
      `let mut cmd = JobArgs::augment_args(clap::Command::new("camel job")).disable_help_flag(true).disable_version_flag(true).no_binary_name(true).args_override_self(true);`
      (infer_long_args stays off by default; do NOT enable it).
      `no_binary_name(true)` is REQUIRED: the tail iterator has no
      argv[0], so clap would otherwise swallow the first tail token as
      the binary name (probed: `--name first --name second` loses
      `--name`, `first` fills the `document` positional). 
      `args_override_self(true)` is REQUIRED for last-wins: without
      it a repeated `--name` is an `ArgumentConflict` "cannot be used
      multiple times" error (probed; same for SetTrue/SetFalse).
   e. For each declared `(name, decl)` in lexical order add runtime
      args with COLON-NAMESPACED IDs (a `:` cannot appear in an
      argument identifier, so a declared arg named `document`,
      `dynamic`, or `no_<bool>` can never collide with another ID —
      the longs stay verbatim):
      - bool: `Arg::new(format!("dyn:{name}")).long(name.clone()).action(ArgAction::SetTrue)`
        and `Arg::new(format!("dyn:no:{name}")).long(format!("no-{name}")).action(ArgAction::SetFalse)`
      - otherwise: `Arg::new(format!("dyn:{name}")).long(name.clone()).action(ArgAction::Set).allow_hyphen_values(true).value_name(name.to_uppercase())`
   f. `match cmd.try_get_matches_from_mut(dynamic.iter())` — on
      `Err(e)`, return `Err(DynamicFlagError::Clap(e.render().to_string()))`
      (clap's unknown-argument hint for undeclared/dash-alias/prefix
      spellings comes free; `--arg` value_parser errors in the tail
      render clap-style too).
   g. From matches: if the rebuilt command's `document` positional
      got a command-line value (check
      `matches.value_source("document") == Some(clap::value_source::ValueSource::CommandLine)`),
      return `Err(UnexpectedPositional { token })` with that value
      (this is the `--flag false` stray-token rejection).
   h. Bool extraction per declared bool `name` — presence MUST be
      detected by command-line value source, NOT `get_flag` truth
      (`ArgAction::SetFalse` implies a `true` default, so `get_flag`
      is true when the negation is ABSENT):
      `let pos = matches.value_source(&format!("dyn:{name}")) == Some(ValueSource::CommandLine);`
      `let neg = matches.value_source(&format!("dyn:no:{name}")) == Some(ValueSource::CommandLine);`
      both → `Err(ContradictoryBool { name })`; exactly `pos` → pair
      `(name, "true")`; exactly `neg` → pair `(name, "false")`; neither → no pair.
      Non-bool: `if matches.value_source(&format!("dyn:{name}")) == Some(ValueSource::CommandLine)`
      → pair `(name, matches.get_one::<String>(&format!("dyn:{name}")).cloned().unwrap())` —
      NOTE: `get_one` addresses the arg by its namespaced ID
      (`dyn:{name}`), while the pair KEY stays the bare `name`.
      (`args_override_self(true)` on the command provides last-wins
      for repeated flags; SetTrue/SetFalse repeats are idempotent.)
   i. Tail statics: `help = value_source("help") == CommandLine`;
      `report = matches.get_one::<PathBuf>("report")` when its value
      source is `CommandLine`; tail `--arg` pairs from
      `matches.get_many::<(String, String)>("args")`. Tail `--config`
      is intentionally IGNORED here (honored by
      `tail_config_override` before config load).
   j. Cross-form conflict: if any key appears in BOTH the dynamic
      pairs and the arg pairs (phase-1 + tail combined), return
      `Err(CrossFormConflict { name })`.
   k. Merge: `pairs = arg_pairs ++ tail_arg_pairs ++ dynamic_pairs`.

**Tests:** (new flags_tests.rs; declarations obtained via
`document::parse_job_document_for_help(&doc_path(), &text).unwrap().args`
on real document texts. TDD: red first, then green)
- `lower_string_flag_to_pair`: document declaring `name: {required: true}`; tail `["--name", "world"]` → `pairs` ends with `("name","world")`; equals-form tail `["--name=world"]` → byte-identical pair. Command: `cargo test -p camel-cli --lib lower_string_flag_to_pair`.
- `lower_last_wins_within_form`: document declaring `name`; tail `["--name", "first", "--name", "second"]` → pair `("name","second")`. Command: `cargo test -p camel-cli --lib lower_last_wins_within_form`.
- `lower_int_flag_flows_to_coercion`: document declaring `count: {type: int}`; lower tail `["--count", "notanint"]` → pairs built, then `document::parse_job_document_with_args(&doc_path(), &text, &lowered.pairs)` → `Err(JobDocError::ArgumentCoercion { .. })` whose Display names `count`, `int`, `notanint` (downstream unchanged). Command: `cargo test -p camel-cli --lib lower_int_flag_flows_to_coercion`.
- `lower_bool_bare_and_negated`: document declaring `verbose: {type: bool, default: "true"}`; tail `["--verbose"]` → `("verbose","true")`; tail `["--no-verbose"]` → `("verbose","false")`; empty tail → no `verbose` pair (default applies downstream). Command: `cargo test -p camel-cli --lib lower_bool_bare_and_negated`.
- `lower_bool_value_form_rejected_naming_spellings`: document declaring `verbose: {type: bool}`; tails `["--verbose=false"]` and `["--no-verbose=false"]` → each `Err(BoolFlagValue { name: "verbose" })`, Display contains `--verbose`, `--no-verbose`, `--arg verbose=false`. Command: `cargo test -p camel-cli --lib lower_bool_value_form_rejected_naming_spellings`.
- `lower_stray_positional_after_bool_rejected`: document declaring `verbose: {type: bool}`; tail `["--verbose", "false"]` → `Err(UnexpectedPositional { token: "false" })`. Command: `cargo test -p camel-cli --lib lower_stray_positional_after_bool_rejected`.
- `lower_contradictory_bool_rejected`: document declaring `verbose: {type: bool}`; tail `["--verbose", "--no-verbose"]` → `Err(ContradictoryBool { name: "verbose" })`. Command: `cargo test -p camel-cli --lib lower_contradictory_bool_rejected`.
- `lower_negation_of_non_bool_is_unknown`: document declaring `count: {type: int}` (no bools); tail `["--no-count"]` → `Err(Clap(s))` naming `--no-count`. Command: `cargo test -p camel-cli --lib lower_negation_of_non_bool_is_unknown`.
- `lower_alias_spellings_rejected`: document declaring `user_name`; tails `["--user-name", "x"]`, `["--user", "x"]`, and `["-u", "x"]` (short spelling) → each `Err(Clap(s))` naming the token. Command: `cargo test -p camel-cli --lib lower_alias_spellings_rejected`.
- `lower_undeclared_flag_renders_clap_error_with_hint`: document declaring only `name`; tail `["--nmae", "x"]` → `Err(Clap(s))` where `s` contains `--nmae` and a suggestion containing `--name`. Pin the exact rendered string in the test with a `// clap 4.x byte-pin` comment (byte-pinned; a clap bump must surface here loudly). Command: `cargo test -p camel-cli --lib lower_undeclared_flag_renders_clap_error_with_hint`.
- `lower_empty_tail_is_identity`: empty tail + arg_pairs → pairs == arg_pairs, `help == false`, `report == None`. Command: `cargo test -p camel-cli --lib lower_empty_tail_is_identity`.

**Acceptance:**
- `cargo test -p camel-cli --lib flags_tests` exits 0 (all 11).
- `cargo clippy -p camel-cli -- -D warnings` and `cargo fmt --check` exit 0.

- [x] 1.3

### Task 1.4: statics recovery, cross-form conflict, undeclared-document mapping

**Files:**
- `crates/camel-cli/src/commands/job/mod.rs` (modified)
- `crates/camel-cli/src/commands/job/flags_tests.rs` (modified)

**Steps:**
1. In `lower_dynamic_flags`, when `declarations` is `None` (legacy
   document): build the phase-2 command from `augment_args` — with
   the SAME `no_binary_name(true)` and `args_override_self(true)`
   settings as Task 1.3 step 2d — and NO
   dynamic args; if `try_get_matches_from_mut` fails with
   `ErrorKind::UnknownArgument`, extract the offending token from
   `err.get(clap::error::ContextKind::InvalidArg)` (NOT by parsing the
   rendered string); if the token starts with `--`, map it to
   `Err(UndeclaredDocument { flag })` with `flag` = the token without
   its `--` prefix and WITHOUT any `=value` suffix (split at the
   first `=`, so `--name=x` yields `name`); any other clap error or a
   non-`--` token keeps the `Clap(s)` class (malformed `--arg` values
   in the tail stay clap-rendered). A tail
   of ONLY static tokens parses cleanly (no dynamic flags exist), so
   legacy `--arg`-only tails keep working through the normal recovery
   path.
2. Verify (and adjust if needed) the Task 1.3 step 2i/2j/2k paths are
   complete: tail `--arg` pair recovery, `--report` recovery,
   `--help` recovery, cross-form conflict detection, and the merge
   order `arg_pairs ++ tail_arg_pairs ++ dynamic_pairs`.
3. No new symbols beyond Task 1.3's enum — this task completes the
   engine's statics/conflict/undeclared branches.

**Tests:** (flags_tests.rs. TDD: red first, then green)
- `lower_tail_statics_recovered`: document declaring `name`; tail `["--name", "w", "--report", "r.json", "--arg", "a=1"]` → `report == Some("r.json")`, pairs contain `("a","1")` and `("name","w")`, `help == false`. Command: `cargo test -p camel-cli --lib lower_tail_statics_recovered`.
- `lower_help_precedence_short_circuits`: document declaring `name`; tail `["--nmae", "x", "--help"]` → `Ok(LoweredDynamic { help: true, .. })` (help wins over flag errors, parity with `--arg` on the help branch). Command: `cargo test -p camel-cli --lib lower_help_precedence_short_circuits`.
- `lower_cross_form_conflict_both_orders`: document declaring `name`; `(dynamic=["--name","a"], arg_pairs=[("name","b")])` and `(dynamic=["--arg","name=b","--name","a"], arg_pairs=[])` → both `Err(CrossFormConflict { name: "name" })` with Display naming both forms. Command: `cargo test -p camel-cli --lib lower_cross_form_conflict_both_orders`.
- `lower_dynamic_on_undeclared_document_errors`: `declarations = None` (legacy document text), tail `["--name", "x"]` → `Err(UndeclaredDocument { flag: "name" })`, Display mentions `args:` and `--arg`; and tail `["--arg", "a=1"]` alone → `Ok` with pair `("a","1")` (statics-only tail on a legacy document is not an error). Command: `cargo test -p camel-cli --lib lower_dynamic_on_undeclared_document_errors`.
- `lower_ids_do_not_collide_with_declared_names`: document declaring `document: {type: string}` (the positional's name), `verbose: {type: bool}`, and `no_verbose: {type: string}` (a bool-negation-shaped name); tail `["--document", "x", "--verbose", "--no_verbose", "y"]` → `Ok` with pairs `("document","x")`, `("verbose","true")`, `("no_verbose","y")`: the colon-namespaced IDs keep the `--no-verbose` negation (dash) and the `--no_verbose` declared flag (underscore) distinct, and a declared `document` flag does not collide with the `document` positional ID. Command: `cargo test -p camel-cli --lib lower_ids_do_not_collide_with_declared_names`.

**Acceptance:**
- `cargo test -p camel-cli --lib flags_tests` exits 0 (all 16).
- `cargo clippy -p camel-cli -- -D warnings` and `cargo fmt --check` exit 0.

- [x] 1.4

### Task 1.5: run_job wiring + binary-level behavior tests

**Files:**
- `crates/camel-cli/src/commands/job/mod.rs` (modified)
- `crates/camel-cli/src/commands/job/tests.rs` (modified)

**Steps:**
1. In `run_job`'s document branch, after the text is read and BEFORE
   the existing `args.help` check: parse the document ONCE via
   `let info = document::parse_job_document_for_help(&document_path, &text)`
   (errors print `{path}: {e}`, exit 2 — same shape as the help
   branch; this also re-runs the Task 1.1 reserved-name guard on
   execution) and HOIST the existing help branch to reuse `info`
   instead of its own `parse_job_document_for_help` call (one parse,
   both paths; stem/description/probe logic moves with it).
2. Compute `let lowered = lower_dynamic_flags(info.args.as_ref(), &args.dynamic, &args.args)`;
   on `Err(e)` print by variant: `Clap(s)` → `eprint!("{s}")`,
   everything else → `eprintln!("camel job: {e}")`; return 2.
3. Help branch becomes `if args.help || lowered.help` — renders the
   declared interface exactly as today from the hoisted `info` (no
   pair resolution; signals already correctly unarmed by the Task 1.2
   gate; no second parse).
4. Call `document::parse_job_document_with_args(&document_path, &text, &lowered.pairs)`
   (replacing `&args.args`).
5. Legacy deprecation guard: `if legacy_header_args && !lowered.pairs.is_empty()`
   (covers tail-recovered `--arg` pairs too).
6. `JobRun` construction: `cli_args: if legacy_header_args { lowered.pairs.clone() } else { Vec::new() }`;
   `report_path: lowered.report.clone().or(args.report.clone())`
   (argv last-wins: a tail `--report` comes after any pre-path one).
7. Keep `execute_job` and everything downstream untouched.

**Tests:** (tests.rs, using the existing `run_camel_job_env` subprocess
harness, `write_jobs_root_document`, and the execution-fixture
helpers already used by the jobargs family in this file — reuse the
nearest existing execution fixture pattern for declared-args jobs.
TDD: red first, then green)
- `binary_dynamic_flag_runs_identical_to_arg`: one declared-args job
  whose body interpolates `${arg:name}`; run twice via harness —
  `["<doc>", "--name", "world"]` and `["<doc>", "--arg", "name=world"]`
  → both exit 0 and both reports show the interpolated `world`. Command: `cargo test -p camel-cli --lib binary_dynamic_flag_runs_identical_to_arg`.
- `binary_bare_name_with_dynamic_flag`: write the job under
  `jobs/hello.job.yaml` via `write_jobs_root_document`; run
  `["hello", "--name", "world"]` → exit 0, interpolated body (bare-name
  resolution + dynamic flags). Command: `cargo test -p camel-cli --lib binary_bare_name_with_dynamic_flag`.
- `binary_bool_spellings_end_to_end`: declared `verbose: {type: bool}`
  interpolated in the body; three runs — `--verbose` → `true`;
  `--no-verbose` → `false`; no flag → declared default applies. Command: `cargo test -p camel-cli --lib binary_bool_spellings_end_to_end`.
- `binary_bool_value_form_rejected`: run `["<doc>", "--verbose=false"]`
  → exit 2, stderr contains `--verbose`, `--no-verbose`,
  `--arg verbose=false`. Command: `cargo test -p camel-cli --lib binary_bool_value_form_rejected`.
- `binary_cross_form_conflict`: run `["<doc>", "--name", "a", "--arg", "name=b"]`
  → exit 2, stderr names `name` and both forms. Command: `cargo test -p camel-cli --lib binary_cross_form_conflict`.
- `binary_dynamic_on_legacy_document_errors`: no-`args:` document,
  run `["<doc>", "--name", "x"]` → exit 2, stderr mentions `args:`
  and `--arg`, and stderr does NOT contain the legacy deprecation
  sentence. Command: `cargo test -p camel-cli --lib binary_dynamic_on_legacy_document_errors`.
- `binary_arg_backcompat_positions`: no-`args:` document recording
  headers; two runs — `["<doc>", "--arg", "name=x"]` (after the path)
  and `["--arg", "name=x", "<doc>"]` (before the path) → both exit 0
  with header `name=x` and the deprecation note (legacy behavior
  unchanged in both positions). Command: `cargo test -p camel-cli --lib binary_arg_backcompat_positions`.
- `binary_tail_help_recovered`: declared-args job; run
  `["<doc>", "--name", "x", "--help"]` → exit 0, stdout renders the
  declared interface (tail `--help` wins over flag processing),
  nothing executes, no signal marker on stderr. Command: `cargo test -p camel-cli --lib binary_tail_help_recovered`.
- `binary_tail_report_recovered`: declared-args job; run
  `["<doc>", "--name", "x", "--report", "<tmp>/r.json"]` → exit 0 and
  the report file exists with interpolated content. Command: `cargo test -p camel-cli --lib binary_tail_report_recovered`.
- `binary_tail_config_selects_jobs_root`: two fixture projects, each
  with its own `Camel.toml` declaring a different `[jobs].dirs` root
  and a distinct `jobs/pick.job.yaml` whose body interpolates
  `${arg:tag}`; run `["pick", "--tag", "t", "--config", "<projectB>/Camel.toml"]`
  from project A's directory → exit 0 and the report shows project
  B's document content (tail `--config` re-anchors root resolution). Command: `cargo test -p camel-cli --lib binary_tail_config_selects_jobs_root`.

**Acceptance:**
- `cargo test -p camel-cli --lib binary_` exits 0 (all 10).
- Full lib suite: `cargo test -p camel-cli --lib` exits 0 (no regression in the existing 60+ document/help/job tests).
- `cargo clippy -p camel-cli -- -D warnings` and `cargo fmt --check` exit 0.

- [x] 1.5

### Task 1.6: help note line + integration e2e + reserved-name help path

**Files:**
- `crates/camel-cli/src/commands/job/help.rs` (modified)
- `crates/camel-cli/src/commands/job/help_tests.rs` (modified)
- `crates/camel-cli/src/commands/job/tests.rs` (modified)
- `crates/camel-cli/tests/job_one_shot_test.rs` (modified)

**Steps:**
1. In help.rs add
   `const FLAG_SPELLING_NOTE: &str = "  (settable as --<name> <VALUE>; bool as --<name> / --no-<name>; or --arg <name>=<value>)";`
2. In `render_job_help`, inside the `Some(declarations) if !entries.is_empty()`
   arm, emit `\n` + `FLAG_SPELLING_NOTE` immediately after
   `out.push_str("Arguments:")` and BEFORE the first argument row. The
   `(no arguments)` arm is untouched (no note).
3. Update every existing golden in help_tests.rs whose document
   declares arguments to include the note line at its exact position;
   goldens for no-args documents gain nothing. Add two NAMED tests:
   `note_line_renders_once_with_arguments` (a document with declared
   args renders `Arguments:` + the note line EXACTLY once — byte-pinned
   — followed by the argument rows) and
   `note_line_absent_without_arguments` (a no-`args:` document renders
   `(no arguments)` and the note string appears NOWHERE in the
   output).
4. Add to tests.rs (subprocess harness): `binary_reserved_name_help_rejected`
   — for EACH of the four reserved names, write
   `jobs/rsv.job.yaml` declaring `args: {<name>: {required: true}}`
   via `write_jobs_root_document`; run `["rsv", "--help"]` → exit 2,
   stderr carries the reserved-name diagnostic naming the argument
   (help path rejects identically to execution, all four names).
5. Add to `crates/camel-cli/tests/job_one_shot_test.rs` (real-binary
   e2e, fixture style of that file):
   `dynamic_flag_one_shot_matches_arg_form` — a declared-args job
   (body `${arg:name}`) run twice: `["<doc>", "--name", "world"]` and
   `["<doc>", "--arg", "name=world"]`; assert both exit 0 and the two
   JSON reports match on the recorded evidence fields exactly as the
   neighboring tests assert them (elapsed/duration fields excluded).
   `dynamic_flag_unknown_exits_two` — run `["<doc>", "--nmae", "x"]`
   → exit 2 and stderr contains clap's unknown-argument text with a
   `--name` suggestion.

**Tests:** (the additions above are the tests. TDD: red first, then green)
- `help_tests` goldens: `cargo test -p camel-cli --lib help_tests` exits 0 with the note pinned byte-exact (`note_line_renders_once_with_arguments`, `note_line_absent_without_arguments`).
- `binary_reserved_name_help_rejected`: `cargo test -p camel-cli --lib binary_reserved_name_help_rejected` exits 0.
- e2e: `cargo test -p camel-cli --test job_one_shot_test dynamic_flag` exits 0.

**Acceptance:**
- All three commands above exit 0.
- `cargo test -p camel-cli --lib` exits 0 (full job family).
- `cargo clippy -p camel-cli -- -D warnings` and `cargo fmt --check` exit 0.

- [x] 1.6
