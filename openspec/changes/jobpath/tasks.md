# Tasks: jobpath

## camel-cli / commands/job

### Task 1.1: Resolution ladder in `resolve_job_path` (probe relative paths, keep bare names byte-identical)

**Files:**
- `crates/camel-cli/src/commands/job/mod.rs` (modified)
- `crates/camel-cli/tests/job_one_shot_test.rs` (modified)
- `CONTEXT-MAP.md` (modified — one clause in the "Jobs discovery
  set" entry, step 3)

**Steps:**
1. Write the nine new integration tests listed under Tests into
   `crates/camel-cli/tests/job_one_shot_test.rs` (model each on the
   existing `explicit_path_wins_over_jobs_resolution` fixture shape:
   `write_config` or a `dirs = ["jobs"]`-style Camel.toml, a routes
   file, job documents, then `run_job_args(dir, &[arg])` or
   `run_job(dir, arg)`). Run them and verify the five new-behavior
   tests fail against the current resolver while the four
   back-compat pin tests pass.
2. Rewrite `resolve_job_path` in `mod.rs` to the blessed ordered
   ladder, keeping the existing error message shapes verbatim
   (`no job \`{arg}\` in any configured root (looked for {probes})`
   and `job \`{arg}\` is ambiguous: matches {n} configured roots:
   {paths}`):
   a. classify `explicit_class = raw.components().count() > 1 ||
      lower.ends_with(".yaml") || lower.ends_with(".yml") ||
      lower.ends_with(".json")` (the existing classification,
      unchanged);
   b. if `raw` is absolute (`raw.is_absolute()`) → return
      `Ok(raw.to_path_buf())` — no probing;
   c. if `explicit_class && raw.exists()` → return
      `Ok(raw.to_path_buf())` — CWD-relative existence wins; bare
      names NEVER take this branch (byte-identical bare behavior);
   d. otherwise build one probe per root by plain `Path::join` of the
      argument as spelled — no normalization, no confinement:
      `probe_name = if lower.ends_with(".yaml") ||
      lower.ends_with(".yml") || lower.ends_with(".json") {
      raw.to_path_buf() } else { PathBuf::from(format!("{name}.job.yaml")) }`
      then `root.join(&probe_name)` per root (append `.job.yaml` for
      bare names and separator-bearing stem paths; verbatim for
      suffixed args so the displayed spelling
      `daily/ingest.job.yaml` resolves);
   e. collect ALL probes that `exists()`; zero → miss error naming
      every probed path; one → return it; many → ambiguity error
      naming every matching path.
3. Update the `resolve_job_path` doc comment, the `JobArgs.document`
   arg doc, and the module header (lines 1-4) to the new ladder:
   absolute → as-is; explicit-class CWD existence wins; bare names
   probe root-level `<name>.job.yaml` only and never consult the CWD;
   relative stem paths probe `<root>/<arg>.job.yaml`; suffixed args
   probe verbatim; probes joined as spelled (no normalization, no
   confinement). Also extend the "Jobs discovery set" entry in
   `CONTEXT-MAP.md` (~line 190) with one clause: after the bare-name
   sentence, document that a document argument existing relative to
   the CWD is used as-is, and on a CWD miss relative paths probe the
   configured roots (stem paths append `.job.yaml`; suffixed
   arguments probe verbatim — the listing's spelling).
4. Run the new tests plus the whole `job_one_shot_test` binary and
   the `job_signal_test` binary; every test (new and pre-existing)
   must pass — pre-existing resolution tests are the back-compat
   proof (`explicit_path_wins_over_jobs_resolution`,
   `explicit_job_path_bypasses_roots_and_bare_miss_is_named`,
   `named_job_uses_later_matching_configured_root`,
   `named_job_collision_reports_all_matching_paths`,
   `listing_formats_descriptions_and_yml_is_display_only`).

**Tests:** (all in `crates/camel-cli/tests/job_one_shot_test.rs`; run
with `cargo test -p camel-cli --test job_one_shot_test <name>` from
the worktree root)
- `nested_stem_path_resolves_across_root`: `Camel.toml` with
  `dirs = ["jobs"]`, valid `jobs/daily/ingest.job.yaml` (direct route,
  output marker) → `run_job(dir, "daily/ingest")` → exit 0, report
  `outcome == "Completed"`, report `document` field ends with
  `jobs/daily/ingest.job.yaml`, stderr empty. Expected: FAIL before
  step 2 (resolver treats `daily/ingest` as explicit, canonicalize
  error), PASS after.
- `nested_document_path_resolves_verbatim`: same fixture →
  `run_job(dir, "daily/ingest.job.yaml")` → exit 0, `Completed`.
  Expected: FAIL before (path does not exist CWD-relative), PASS
  after.
- `cwd_relative_existence_wins_over_root_probe`: CWD-relative
  `local/echo.job.yaml` with marker `cwd-marker` AND
  `jobs/local/echo.job.yaml` with marker `root-marker`, distinct
  routes → `run_job(dir, "local/echo.job.yaml")` → exit 0 and the
  report proves the CWD copy ran (`cwd-marker` reaches the output,
  `root-marker` does not). Expected: PASS before AND after
  (back-compat pin; must not regress).
- `nested_relative_path_collision_names_every_match`:
  `dirs = ["first", "second"]`, valid `first/daily/ingest.job.yaml`
  and `second/daily/ingest.job.yaml` → `run_job(dir, "daily/ingest")`
  → exit 2, stderr contains `ambiguous` plus both exact paths
  `first/daily/ingest.job.yaml` and `second/daily/ingest.job.yaml`.
  Expected: FAIL before, PASS after.
- `nested_relative_path_miss_names_probes`: `dirs = ["first",
  "second"]`, no nested docs → `run_job(dir, "daily/missing")` →
  exit 2, stderr contains `no job \`daily/missing\`` plus
  `first/daily/missing.job.yaml` and
  `second/daily/missing.job.yaml`. Expected: FAIL before, PASS
  after.
- `bare_name_does_not_descend_into_subdirectories`: `dirs = ["jobs"]`,
  only `jobs/daily/ingest.job.yaml` → `run_job(dir, "ingest")` →
  exit 2, stderr names `jobs/ingest.job.yaml` and does NOT contain
  `daily/ingest.job.yaml`. Expected: PASS before AND after
  (back-compat pin).
- `bare_name_ignores_cwd_entries`: `dirs = ["jobs"]`, valid
  `jobs/report.job.yaml`, decoy CWD file named exactly `report`
  (no extension, content `decoy`) → `run_job(dir, "report")` →
  exit 0, `Completed`, report proves the root document ran (decoy
  never parsed). Expected: PASS before AND after (back-compat pin).
- `probe_is_joined_as_spelled_without_normalization`: `dirs =
  ["first"]`, no `first/../outside/ingest.job.yaml` →
  `run_job(dir, "../outside/ingest")` → exit 2, stderr contains the
  verbatim joined probe `first/../outside/ingest.job.yaml`. Expected:
  FAIL before (old resolver uses the arg as-is → different error
  text), PASS after.
- `absolute_argument_is_used_as_is_without_probing`: job doc at a
  tempdir absolute path outside roots → `run_job` with that absolute
  arg → exit 0, `Completed`; then `run_job` with a nonexistent
  absolute path → exit 2, stderr contains that path and does NOT
  contain `in any configured root`. Expected: PASS before AND after
  for the existing case; the nonexistent case may change error text
  only if probing were reached — it must not be.

**Acceptance:**
- `cargo test -p camel-cli --test job_one_shot_test` — all tests
  pass (new nine + every pre-existing).
- `cargo test -p camel-cli --test job_signal_test` and
  `cargo test -p camel-cli --lib` — all pass (the lib suites in
  `src/commands/job/tests.rs` spawn the binary on the changed
  resolver path).
- `cargo test -p camel-config --lib
  legacy_jobs_dir_alias_is_supported` — passes (legacy `[jobs].dir`
  alias guard; the resolution change must not disturb config
  parsing).
- `cargo fmt --check --all` clean; `cargo clippy -p camel-cli --
  -D warnings` clean.
- Bare-name behavior byte-identical: the four back-compat pin tests
  above pass both before and after (verified in step 1's run that
  they were passing before).

- [x] 1.1

### Task 1.2: Listing display drops `: stem`; help header shows the shared invocable display name

**Files:**
- `crates/camel-cli/src/commands/job/mod.rs` (modified)
- `crates/camel-cli/src/commands/job/help.rs` (modified)
- `crates/camel-cli/src/commands/job/help_tests.rs` (modified)
- `crates/camel-cli/tests/job_signal_test.rs` (modified)
- `crates/camel-cli/tests/job_one_shot_test.rs` (modified)

**Steps:**
1. Write/amend the tests under Tests first; verify the amended
   display pins and the new integration tests fail against current
   code (the unit pin `render_nested_display_name_verbatim` is a
   renderer contract pin — it passes once written; its wiring is
   proven by `nested_job_help_header_shows_invocable_path`, which
   fails before).
2. Add `fn root_relative_display(path: &Path, root: &Path) ->
   Option<String>` in mod.rs — the ONE shared display rule: return
   `Some(relative.display().to_string())` when
   `path.strip_prefix(root)` yields a relative path with more than
   one component, `None` otherwise. In `walk_level`, replace the
   inline strip/match with
   `let display = root_relative_display(&path, root).unwrap_or(stem);`
   — nested rows become exactly the configured-root-relative path
   (the invocable spelling, `: stem` dropped); root-level rows keep
   the bare stem. Update the `ListedJob` doc comment ("the bare stem
   for files directly under the configured root, the
   configured-root-relative path for nested ones — the exact
   spelling that resolves as a document argument").
3. Add `fn job_display_name(resolved: &Path, roots: &[(String,
   PathBuf)]) -> String` in mod.rs, built ON the shared core: for
   each root, `root_relative_display(resolved, root)`; on the first
   `Some`, return it; otherwise return `job_stem` of the resolved
   file name (falling back to the full display string when the path
   has no file name). Lexical only, computed from the
   pre-canonicalize resolved path — never canonicalize inside the
   helper; document that symlink aliasing is not identity-resolved
   and that both the listing and this helper go through
   `root_relative_display`, so the two surfaces cannot drift.
4. At the `--help` call site in `run_job`, pass
   `&job_display_name(&resolved, &jobs_roots)` as the first
   argument of `help::render_job_help` (replacing
   `job_stem(&file_name)` around line 625) and remove the now-unused
   `file_name` binding (mod.rs ~617-620, dead code otherwise —
   clippy `-D warnings` fails on it). `resolved` (mod.rs ~582) and
   `jobs_roots` (~556) are both alive and only borrowed at the call
   site; keep reading/parsing from the canonicalized
   `document_path` as today.
5. In `help.rs`, update the `render_job_help` doc comment: the first
   parameter is the job's display name (the invocable spelling —
   configured-root-relative path for nested documents, file stem
   otherwise), rendered verbatim as the header line.
6. Update the pinned display assertions in the integration tests to
   the new line shape: `job_signal_test.rs`
   `job_listing_recurses_and_skips_test_documents` →
   `domain/report.job.yaml — nested job`;
   `job_listing_stops_at_depth_eight` → the depth-8 line becomes
   `d1/d2/d3/d4/d5/d6/d7/d8/eight.job.yaml — depth eight`;
   `job_listing_walks_directory_with_many_subdirs` →
   `d0000/early.job.yaml — early job` and
   `d1029/late.job.yaml — late job` (the root-level `top — top job`
   row is unchanged); `job_one_shot_test.rs` recursive-listing test
   → `a/aa.job.yaml — a dir job` and `b/bb.job.yaml — b dir job`.
7. Add the help-header integration test and the unit pin (below).
   Verify ALL job test binaries pass.

**Tests:**
- `render_nested_display_name_verbatim` (unit, `help_tests.rs`):
  `render_job_help("daily/ingest.job.yaml", Some("Ingest"), &info)`
  → first line of the rendered string is exactly
  `daily/ingest.job.yaml`. Expected: PASS before and after — the
  renderer is pure and accepts any header string, so this pin locks
  the renderer contract; the WIRING (call site passes the display
  name) is proven by `nested_job_help_header_shows_invocable_path`,
  which fails before Task 1.2 lands.
- `nested_job_help_header_shows_invocable_path` (integration,
  `job_one_shot_test.rs`): `dirs = ["jobs"]`, valid
  `jobs/daily/ingest.job.yaml` with `description: Daily ingest` →
  `run_job_args(dir, &["daily/ingest", "--help"])` → exit 0, first
  stdout line is exactly `daily/ingest.job.yaml`, stdout contains
  `Daily ingest`. Expected: FAIL before (nested stem path not
  resolvable pre-Task-1.1; header would also render bare `ingest`).
- `job_listing_recurses_and_skips_test_documents` (amended,
  `job_signal_test.rs`): exact stdout line
  `domain/report.job.yaml — nested job`; the `: report` infix is
  gone. Expected: FAIL before, PASS after.
- `listing_display_is_invocable_verbatim` (integration,
  `job_one_shot_test.rs`): `dirs = ["jobs"]`,
  `jobs/daily/ingest.job.yaml` → run `camel job` (no args) and
  capture stdout: a line starting `daily/ingest.job.yaml —` exists;
  then `run_job(dir, "daily/ingest.job.yaml")` (the exact spelling
  before the ` —` descriptor, minus the descriptor) → exit 0,
  `Completed`. This pins display == invocable spelling in one test.
  Expected: FAIL before, PASS after.
- Root-level display unchanged: `explicit_job_dirs_override_legacy_
  dir` keeps asserting the exact line `first — (no description)` —
  no edit; must keep passing (regression guard).

**Acceptance:**
- `cargo test -p camel-cli --test job_one_shot_test` and `--test
  job_signal_test` — all pass.
- `cargo test -p camel-cli --lib help_tests` — all pass.
- `cargo fmt --check --all` clean; `cargo clippy -p camel-cli --
  -D warnings` clean.
- `grep -rn '\.job\.yaml: \|\.job\.yml: ' crates/camel-cli/tests/
  job_signal_test.rs crates/camel-cli/tests/job_one_shot_test.rs`
  returns no hits (no old-shape `relative: stem` display pins remain;
  root-level `stem — desc` lines are unaffected by the pattern).

- [x] 1.2
