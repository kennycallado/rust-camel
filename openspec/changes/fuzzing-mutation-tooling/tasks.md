# Tasks: fuzzing-mutation-tooling

## fuzz/ crate

### Task 1.1: Excluded camel-fuzz crate with dsl_yaml harness and target

**Files:**
- `Cargo.toml` (modified: add `"fuzz"` to `workspace.exclude`)
- `.gitignore` (modified: add `/target-fuzz/`)
- `fuzz/Cargo.toml` (new)
- `fuzz/.gitignore` (new: single line `target` — the excluded crate is
  its own workspace and builds into `fuzz/target/`, which the root-anchored
  `/target` entry does not cover)
- `fuzz/src/lib.rs` (new)
- `fuzz/fuzz_targets/dsl_yaml.rs` (new)
- `fuzz/Cargo.lock` (new, generated then committed)

**Steps:**
1. In root `Cargo.toml`, append `"fuzz"` to the existing `workspace.exclude`
   array, with a one-line comment `# cargo-fuzz crate: excluded to keep the
   production lockfile frozen`.
2. In `.gitignore`, add the line `/target-fuzz/` directly under the
   existing `/target` line. Create `fuzz/.gitignore` containing the single
   line `target`.
3. Create `fuzz/Cargo.toml`: package name `camel-fuzz`, version `0.1.0`,
   edition `2024` and `rust-version = "1.89"` (matching
   `[workspace.package]` in the root `Cargo.toml`, which is ground truth;
   no workspace inheritance — the crate is excluded);
   `[package.metadata]` with `cargo-fuzz = true`;
   `[dependencies]` `camel-dsl` and `camel-api` as path dependencies
   (`../crates/camel-dsl`, `../crates/camel-api`) and `libfuzzer-sys = "0.4"`;
   plus the explicit target registration (cargo does not auto-discover
   `fuzz_targets/`):
   `[[bin]] name = "dsl_yaml", path = "fuzz_targets/dsl_yaml.rs", test = false, doc = false`.
4. Create `fuzz/src/lib.rs` with
   `pub fn dsl_yaml_harness(data: &[u8])`:
   convert with `str::from_utf8`; on `Ok`, call
   `camel_dsl::yaml::parse_yaml_with_threshold_and_security(s,
   camel_api::stream_cache::DEFAULT_STREAM_CACHE_THRESHOLD,
   SecurityCompileContext::default())` and discard the result. Import
   `SecurityCompileContext` via
   `use camel_dsl::SecurityCompileContext;` (re-exported at
   `crates/camel-dsl/src/lib.rs:43`).
5. Create `fuzz/fuzz_targets/dsl_yaml.rs`: `#![no_main]`, then
   `libfuzzer_sys::fuzz_target!(|data: &[u8]| { camel_fuzz::dsl_yaml_harness(data); });`.
6. Run `cargo generate-lockfile --manifest-path fuzz/Cargo.toml` to
   produce `fuzz/Cargo.lock`. If the registry fetch fails (offline
   environment), STOP and report `blocker: registry-fetch libfuzzer-sys`
   instead of committing a partial lock.
7. Verify the fuzz target compiles without linking or running libFuzzer:
   `cargo check --manifest-path fuzz/Cargo.toml --lib --bins` exits 0.
8. Verify the root lockfile is untouched: `git diff --exit-code Cargo.lock`.

**Tests:** (skeleton task; behavior tests are Task 1.2 — here the check
command is the compile proof)
- Command: `cargo check --manifest-path fuzz/Cargo.toml --lib --bins` —
  expected FAIL before the files exist, PASS after.

**Acceptance:**
- `cargo check --manifest-path fuzz/Cargo.toml --lib --bins` exits 0
  (proves `fuzz_targets/dsl_yaml.rs` has valid syntax, not just the lib).
- `git diff --exit-code Cargo.lock` exits 0 (root lockfile byte-identical).
- `cargo metadata --format-version 1 | grep -c camel-fuzz` prints stdout
  `0` (the grep process itself exits 1 on zero matches — the count is the
  criterion, not the exit code).
- `cargo build --workspace` at the worktree root still exits 0.

- [x] 1.1

### Task 1.2: Harness semantics tests

**Files:**
- `fuzz/tests/harness_semantics.rs` (new)

**Steps:**
1. Create `fuzz/tests/harness_semantics.rs` with the five tests below.
   Reuse the minimal valid route fixture from
   `crates/camel-dsl/tests/sampling_dsl_tests.rs:13-20` (the
   `sampling_yaml_short_form_compiles` document: route `r1` from
   `direct:start` with one `sampling: 7` step) — verified to return `Ok`
   under `parse_yaml` by that test. Do NOT use the
   `schema_validation.rs:255` fixture: it is the
   `cache_invalidate_both` case, which parses declaratively but is
   rejected at compile, so `parse_yaml` returns `Err` for it.
2. Run the test command; all five must pass.

**Tests:** (in `fuzz/tests/harness_semantics.rs`; all call the lib, none
require libFuzzer)
- `valid_minimal_route_parses`: the sampling-fixture route document → call
  `camel_dsl::yaml::parse_yaml` on it directly → assert `Ok`, then call
  `dsl_yaml_harness` on its bytes → no panic.
- `malformed_input_no_panic`: for each of truncated YAML (a `from:` line
  with no value), a document with a wrong field type (string where a map is
  required), and a random garbage prefix — call `dsl_yaml_harness` →
  no panic (the parser returns `Err` internally).
- `invalid_utf8_skipped`: `dsl_yaml_harness(&[0xff, 0xfe, 0x00, 0x80])` →
  returns without panic (UTF-8 conversion fails, parser not called).
- `alias_bomb_no_panic`: build a YAML string with 200 anchored sequence
  nodes (`&a0` … `&a199`) followed by 2,000 alias references spread over
  those anchors. The geometry is load-bearing: with 200 anchors the
  `alias_anchor_ratio = 10.0` heuristic (noyalib-0.0.18
  `src/parser/loader.rs:71,383-393`) does not trip (2,000 ≤ 10 × 200),
  so the run deterministically hits `max_alias_expansions = 1024`
  (`src/de/config.rs:261`, checked first at `loader.rs:380-382`).
  Call `dsl_yaml_harness` → no panic; call
  `parse_yaml_with_threshold_and_security` directly with the same
  arguments as the harness → assert `Err` whose message contains
  `alias expansion limit exceeded` (the Display text of noyalib's
  `RepetitionLimitExceeded`, `src/error.rs:826`, propagated through the
  camel-dsl error chain) — proving the expansion budget, not incidental
  schema failure, is the cause.
- `deep_nesting_no_panic`: build a 1,000-level nested block sequence
  string (`"- " * 1000` + terminator); call `dsl_yaml_harness` → no panic;
  direct parse call → assert `Err`.
- Command: `cargo test --manifest-path fuzz/Cargo.toml` — expected FAIL
  before the test file exists, PASS after.

**Acceptance:**
- `cargo test --manifest-path fuzz/Cargo.toml` exits 0. This compiles the
  lib and `tests/` only, NOT the `[[bin]]` — guaranteed by
  `test = false, doc = false` on the `[[bin]]` entry from Task 1.1 — so
  it needs neither nightly nor the sanitizer runtime.
- `git diff --exit-code Cargo.lock` still exits 0.

- [x] 1.2

### Task 1.3: Committed seed corpus for dsl_yaml

**Files:**
- `fuzz/seeds/dsl_yaml/valid_minimal.yaml` (new)
- `fuzz/seeds/dsl_yaml/alias_bomb.yaml` (new)
- `fuzz/seeds/dsl_yaml/deep_nesting.yaml` (new)
- `fuzz/seeds/dsl_yaml/malformed_unknown_step.yaml` (new)
- `fuzz/seeds/dsl_yaml/malformed_flow_seq.yaml` (new)
- `fuzz/seeds/dsl_yaml/malformed_empty_id.yaml` (new)
- `fuzz/tests/seeds.rs` (new)

**Steps:**
1. Write `valid_minimal.yaml` using the same sampling-fixture route
   document chosen in Task 1.2 (`sampling_dsl_tests.rs:13-20`).
2. Write `alias_bomb.yaml`: 200 anchored sequence nodes plus 2,000 alias
   references spread over them (same geometry and counts as the Task 1.2
   alias-bomb test — ratio-safe, above the 1024 expansion budget).
3. Write `deep_nesting.yaml`: 1,000-level nested sequences (same shape as
   the Task 1.2 deep-nesting test, above the 128 depth budget).
4. Write the three malformed seeds with an explicit mapping: the empty-id
   malformed document from `crates/camel-dsl/tests/format_aware_errors.rs`
   (`const YAML_MALFORMED` at line 10) becomes `malformed_empty_id.yaml`;
   the malformed flow-sequence document `routes:\n  - id: [invalid\n    from: timer:tick\n`
   from the same file (inline `let yaml_bad` at line 69 — a broken flow
   sequence, not a truncation) becomes `malformed_flow_seq.yaml` — cite
   the source file and line in a one-line comment at the top of each.
   Synthesize `malformed_unknown_step.yaml` by hand (a valid route whose
   single step name is not a known step — the unknown-field cases in
   `schema_validation.rs` are `serde_json::json!` values, not YAML, so
   this seed is derived, not copied).
5. Create `fuzz/tests/seeds.rs` per the tests below.

**Tests:** (in `fuzz/tests/seeds.rs`)
- `required_seed_shapes_present`: read_dir `fuzz/seeds/dsl_yaml` → collect
  file names → assert the exact six names listed in Files exist
  (including `malformed_flow_seq.yaml`).
- `all_seeds_no_panic`: for every file in `fuzz/seeds/dsl_yaml`, read
  bytes and call `camel_fuzz::dsl_yaml_harness` → no panic for any seed.
- `valid_seed_parses_ok`: parse `valid_minimal.yaml` content with
  `camel_dsl::yaml::parse_yaml` → assert `Ok`.
- Command: `cargo test --manifest-path fuzz/Cargo.toml --test seeds` —
  expected FAIL before seeds exist, PASS after.

**Acceptance:**
- The seeds test command exits 0 (same `test = false` isolation note as
  Task 1.2: the lib and `tests/` compile, not the `[[bin]]`).
- The seed directory contains exactly the six named files (no extras).

- [x] 1.3

## scripts/xtask

### Task 2.1: `cargo xtask fuzz` subcommand with guards and isolation

One vertical slice: the helpers, their unit tests, and the dispatch wiring
live in one compile unit — splitting them would leave either unwired
helpers (dead_code under `-D warnings`) or a stub `run` (a forbidden
placeholder).

**Files:**
- `scripts/xtask/src/fuzz.rs` (new)
- `scripts/xtask/src/main.rs` (modified: `mod fuzz;`, `Commands::Fuzz`
  variant, dispatch arm)

**Steps:**
1. In `main.rs` add `mod fuzz;` beside the existing module declarations.
2. Add `Commands::Fuzz { target: String, #[arg(long, default_value_t = 60)] time: u64 }`
   with doc comment `Run a cargo-fuzz target in this worktree only`.
   The dispatch arm does exactly:
   `let root = workspace_root_or_exit();`
   then `if let Err(msg) = fuzz::run(&root, &target, time) { eprintln!("error: {msg}"); std::process::exit(1); }`
   — matching the house pattern at `main.rs:1077-1081` (note the
   `error: ` prefix). Both `workspace_root_or_exit` and the dispatch arm
   live in `main.rs`, so no visibility change to existing helpers is
   needed.
3. In `fuzz.rs` define pure helpers (unit-testable, no I/O):
   - `const KNOWN_TARGETS: &[&str] = &["dsl_yaml"];`
   - `fn is_main_checkout(git_dir: &Path, git_common_dir: &Path) -> bool` —
     true when the two paths are equal after best-effort canonicalization
     (`fs::canonicalize` on each; fall back to the raw path when
     canonicalization fails, so nonexistent test paths still compare).
   - `fn corpus_dir(worktree: &Path, target: &str) -> PathBuf` —
     `<worktree>/target-fuzz/corpus/<target>`.
   - `fn artifacts_dir(worktree: &Path, target: &str) -> PathBuf` —
     `<worktree>/target-fuzz/artifacts/<target>`.
   - `fn artifact_prefix(worktree: &Path, target: &str) -> String` —
     `<artifacts_dir>` rendered as a string with trailing slash.
   - `fn libfuzzer_args(time: u64, prefix: &str) -> Vec<String>` —
     `["-max_total_time=<time>".into(), format!("-artifact_prefix={prefix}")]`.
   - `fn copy_seeds(seeds_dir: &Path, corpus_dir: &Path) -> std::io::Result<usize>` —
     copies every regular file when the corpus dir is empty or missing,
     returns the count written; no-op returning 0 otherwise.
   - `fn new_artifacts(before: &[PathBuf], after: &[PathBuf]) -> Vec<PathBuf>` —
     returns the entries of `after` absent from `before`.
4. Implement `pub(crate) fn run(root: &Path, target: &str, time: u64) -> Result<(), String>`
   (public to the crate so the `main.rs` dispatch arm can call it):
   a. Run `git rev-parse --git-dir` and `git rev-parse --git-common-dir`
      from `root`; if `is_main_checkout` → return `Err` with one line
      `refusing: cargo xtask fuzz must run in a linked worktree, not the main checkout`.
   b. If `target` is not in `KNOWN_TARGETS` → return `Err` naming the
      unknown target plus the known list.
   c. Probe `cargo +nightly fuzz --version`; on failure return `Err` with
      `cargo-fuzz or nightly toolchain missing — install with: rustup toolchain install nightly && cargo install cargo-fuzz`
      before any build starts.
   d. Create `corpus_dir(root, target)`; call
      `copy_seeds(<root>/fuzz/seeds/<target>, corpus)`.
   e. Create `artifacts_dir(root, target)` BEFORE the run and snapshot its
      file list (`before`).
   f. Spawn
      `cargo +nightly fuzz run <target> <corpus> -- <libfuzzer_args>` with
      env `CARGO_TARGET_DIR=<root>/target-fuzz`. Inherit stdout/stderr.
   g. Exit semantics: a zero fuzz-run exit returns `Ok(())`. On non-zero
      exit: list `artifacts_dir` again, diff with
      `new_artifacts(before, after)`; if exactly one new artifact exists,
      spawn `cargo +nightly fuzz tmin <target> <artifact>` with the SAME
      env `CARGO_TARGET_DIR=<root>/target-fuzz`, capture its stdout, and
      locate the minimized file by globbing `artifacts_dir` for entries
      created after `tmin` started (cargo-fuzz writes the minimized file
      into the artifacts directory; its exact naming — historically
      `minimized-from-*` — is verified by the Phase 2 tmin drill, so do
      NOT hardcode the pattern); print the
      minimized artifact path and the promotion instruction:
      `promote this input into a #[test] regression case; do not commit the raw artifact`.
      If multiple new artifacts exist, print all their paths and run
      `tmin` on the newest one. If none, report the non-zero exit verbatim.
      The wrapper returns `Err` in all three non-zero cases, so the fuzz
      crash surfaces as a non-zero xtask exit; a failed `tmin` invocation
      returns an `Err` that names the minimization failure separately from
      the original crash.

**Tests:** (in `scripts/xtask/src/fuzz.rs` under `#[cfg(test)]`)
- `main_checkout_detected`: `is_main_checkout(Path::new("/r/.git"), Path::new("/r/.git"))`
  → true.
- `linked_worktree_detected`: `is_main_checkout(Path::new("/r/.git/worktrees/w"), Path::new("/r/.git"))`
  → false.
- `artifact_prefix_format`: `artifact_prefix(Path::new("/wt"), "dsl_yaml")`
  → `/wt/target-fuzz/artifacts/dsl_yaml/` (trailing slash present).
- `libfuzzer_args_shape`: `libfuzzer_args(90, "/wt/target-fuzz/artifacts/dsl_yaml/")`
  → contains `-max_total_time=90` and
  `-artifact_prefix=/wt/target-fuzz/artifacts/dsl_yaml/`.
- `seeds_copied_into_empty_corpus`: temp dir with one seed file and an
  empty corpus dir → `copy_seeds` → Ok(1) and the file exists in corpus.
- `seeds_skipped_when_corpus_has_files`: corpus dir pre-populated with one
  file → `copy_seeds` → Ok(0), seed file absent from corpus.
- `new_artifacts_diff`: `before = [/a/crash-1]`, `after = [/a/crash-1, /a/oom-2]`
  → returns exactly `[/a/oom-2]`.
- Command: `cargo test -p xtask fuzz` — the trailing `fuzz` is a name
  filter that matches the fully-qualified module path
  (`fuzz::tests::<name>`), so the module's tests run; expected FAIL
  before implementation, PASS after.

**Acceptance:**
- `cargo test -p xtask fuzz` exits 0.
- `cargo clippy -p xtask -- -D warnings` exits 0.
- `cargo run --package xtask -- fuzz dsl_yaml` executed in this worktree
  (cargo-fuzz not installed here) exits non-zero and its stderr contains
  `cargo install cargo-fuzz` — the toolchain-guard path, no build started.

- [x] 2.1

## Verification

### Task 3.1: Isolation verification and toolchain-deferral record

**Files:**
- `openspec/changes/fuzzing-mutation-tooling/verification.md` (new)

**Steps:**
1. Run the guard-path check:
   `cargo run --package xtask -- fuzz dsl_yaml; echo "exit=$?"` in the
   worktree. Record the exit code and the guard line printed.
2. Run the isolation static checks and record each result:
   `git diff --exit-code Cargo.lock` (expect exit 0);
   `cargo metadata --format-version 1 | grep -c camel-fuzz` (expect stdout
   `0`; grep itself exits 1 on zero matches — record the count);
   `test ! -d target-fuzz || ls target-fuzz` (guard fired before build, so
   the directory must be absent or empty of build output);
   `grep -A40 'QUALITY GATES' AGENTS.md | grep -c 'fuzz'` (expect stdout
   `0`, same grep caveat);
   `git check-ignore -q fuzz/target && echo fuzz-target-ignored` (the
   manifest-path cargo commands of Tasks 1.1-1.3 create `fuzz/target/`
   inside the excluded crate's own workspace — confirm it is git-ignored
   and note it is distinct from both `target-fuzz/` and the shared main
   `./target`).
3. Run `cargo test --manifest-path fuzz/Cargo.toml` and
   `cargo test -p xtask fuzz` once more; record both green.
4. Write `verification.md` with: the recorded outputs of steps 1-3, plus a
   `## Deferred to CI` section stating that the full
   `cargo xtask fuzz dsl_yaml --time 300` run, the `tmin` drill on an
   injected panic, the main-`./target` mtime check, and the end-to-end
   main-checkout refusal drill (AGENTS.md forbids building xtask in the
   main checkout, so only the pure predicate is unit-tested locally)
   require nightly + cargo-fuzz or main-checkout execution, which this
   environment lacks; they execute in the Phase 2 CI smoke (bd rc-7rw2).
   Mark them `integration-verification-deferred-to-CI`. The deferral
   RECORDS that the runtime drills are unexecuted — it does not claim the
   proposal's runtime acceptance criteria passed; those criteria close
   when the Phase 2 CI smoke runs them.

**Tests:** (verification-only task; checks are the commands above, no new
test functions)

**Acceptance:**
- `verification.md` exists with the guard-path exit code, five isolation
  check results, two green test commands, and the deferral section
  explicitly stating the runtime drills are unexecuted, not passed.
- All recorded isolation checks pass (lockfile diff clean, zero
  camel-fuzz members, no fuzz entry in QUALITY GATES, `fuzz/target/`
  confirmed git-ignored).

- [x] 3.1
