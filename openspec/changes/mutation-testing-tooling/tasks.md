# Tasks: mutation-testing-tooling

## Task 1: mutants config + git isolation

**Files**:
- `.cargo/mutants.toml` (new)
- `.gitignore` (modified)
- `flake.nix` (modified)

**Steps**:
1. Create `.cargo/mutants.toml` with EXACTLY:
   ```toml
   # Scoped, informational mutation testing (bd rc-eba8, ADR context: audit R1
   # adoption decision). Never a gate, never a threshold — see
   # openspec/changes/mutation-testing-tooling.
   examine_globs = [
     "crates/camel-api/src/ssrf.rs",
     "crates/components/camel-mqtt/src/config.rs",
     "crates/components/camel-jms/src/config.rs",
   ]
   ```
   No other keys in v1 (no timeout_multiplier — the 15-min figure is a
   measured criterion, not an enforced timeout).
2. Append `/target-mutants/` to `.gitignore` in the build-artifacts section
   next to `/target-fuzz/`.
3. Add `pkgs.cargo-mutants` to the flake devShell `packages` list, next to
   `pkgs.cargo-fuzz`, with the same comment style. The project's locked
   nixpkgs ships cargo-mutants **27.1.0** — exactly the pinned version the
   wrapper enforces (schema + exit codes are pinned to it). cargo-mutants
   runs on the stable toolchain — NO nightly, NO cargo shim (unlike
   cargo-fuzz).

**Tests**:
- name: config-verbatim (command verification — the `#[test]` version lands
  in Task 2 with the module)
  setup: repo has `.cargo/mutants.toml` from step 1
  command: `python3 -c "import tomllib; g=tomllib.load(open('.cargo/mutants.toml','rb'))['examine_globs']; assert g == ['crates/camel-api/src/ssrf.rs','crates/components/camel-mqtt/src/config.rs','crates/components/camel-jms/src/config.rs'], g" && git check-ignore target-mutants/`
  expected: exit 0 (config verbatim; target-mutants ignored)

**Acceptance**:
- Command above exits 0.
- `git diff .gitignore` shows exactly the one added line.
- Root `Cargo.lock` byte-identical (`git status --porcelain Cargo.lock` empty).

- [x] 1. mutants-config-isolation

---

## Task 2: xtask mutants wrapper

**Files**:
- `scripts/xtask/src/mutants.rs` (new)
- `scripts/xtask/src/main.rs` (modified — dispatch entry)

**Steps**:
1. Create `scripts/xtask/src/mutants.rs` following `fuzz.rs` structure:
   a thin `run` + pure helpers, tests on the helpers (fuzz.rs has NO
   injection machinery — its tests exercise pure fns with synthetic paths;
   mirror that idiom). Public surface:
   - `pub fn run(root: &Path, args: &MutantsArgs) -> Result<(), String>`
     (same `root: &Path` shape as `fuzz.rs::run`), where
     `#[derive(Default)] pub(crate) struct MutantsArgs { pub(crate)
     file: Option<String>, pub(crate) diff: bool, pub(crate) json: bool }`.
   - Pure helper `fn mutants_argv(root: &Path, args: &MutantsArgs) ->
     Result<Vec<String>, String>`: flag exclusivity check
     (`file.is_some() && diff` → Err usage); argv assembly — always
     `--output <root>/target-mutants`; no flags → argv with NO
     `--file`/`--no-config`/`--in-diff`; `file=P` → `--no-config --file P`;
     `diff` → `--in-diff`.
   - Pure helper `fn target_env(root: &Path) -> Vec<(String, String)>`:
     yields exactly `("CARGO_TARGET_DIR", "<root>/target-mutants")`.
   - Pure helper `fn survivor_lines(outcomes_json: &[u8]) ->
     Result<Vec<String>, String>`: parse outcomes (see schema note in
     Tests) and render survivor JSON lines
     `{"file","function","mutation","status"}`, survivors only; `Err` on
     malformed JSON, unknown schema shape, or entries missing required
     fields (schema drift must fail loudly).
   - Pure helper `fn classify_exit(code: Option<i32>) ->
     Result<bool, String>` with cargo-mutants exit codes PINNED (e_gpt
     ruling, plan-bless round 2): `Some(0)` = all caught → `Ok(false)`;
     `Some(2)` = missed mutants
     found → `Ok(true)` (still success — informational);
     `Some(1|3|4|5|6|70)` = operational errors → `Err` naming the class;
     `None` (signal termination, `ExitStatus::code() == None`) → `Err`.
     The caller passes `status.code()` directly.
     `run` maps `Ok(_)` to success and `Err` to its error path; survivors
     NEVER produce `Err`. Task 3's smoke cross-checks the pinned codes
     against the real binary — if reality differs, classify_exit + its
     test + this pin are corrected together in the measurement commit.
   - Pure guard `fn guard_error(git_dir: &Path, git_common_dir: &Path) ->
     Option<String>` and `fn missing_tool_error() -> String`: reuse
     fuzz.rs's `is_main_checkout` logic (extract to a shared location ONLY
     if fuzz.rs exports/permits; otherwise replicate the path logic
     locally — either way fuzz.rs behavior and its tests stay untouched).
   - `run` composes: guard → presence check (`cargo mutants --version`
     probe, PINNED to cargo-mutants **27.1.0** so schema and exit codes
     cannot drift silently; absent → `Err` containing
     `cargo install --locked cargo-mutants --version 27.1.0`) → env from
     `target_env` + argv from `mutants_argv` → spawn `cargo mutants` →
     `classify_exit` on its status; on `Ok(_)` and `json: true`, print
     `survivor_lines(<root>/target-mutants/mutants.out/outcomes.json)`
     (an `Err` from survivor_lines on a successful run is itself an
     operational failure — schema drift). Exit mapping: survivors never
     affect success; `Err` only for guard, missing tool, spawn failure,
     operational-failure exit classes. STDOUT OWNERSHIP: in JSON mode the
     child's stdout is captured (not inherited), its human-readable output
     forwarded to stderr — stdout carries ONLY the wrapper's JSONL, so
     `tee` captures a clean parseable stream.
2. Wire dispatch in `main.rs` (clap derive, mirroring the `Fuzz` variant
   at main.rs:169-176 and its match arm at main.rs:468-474): add
   `Commands::Mutants { file: Option<String>, diff: bool, json: bool }`
   with doc comments, a match arm that constructs
   `let args = MutantsArgs { file, diff, json };` from the clap fields
   and calls `mutants::run(&root, &args)`, and `mod mutants;`.
3. No test invokes real cargo-mutants — helpers + synthetic paths only.

**Tests** (all in `scripts/xtask/src/mutants.rs` `#[cfg(test)] mod tests`,
RED first; pure helpers + synthetic paths, fuzz.rs idiom):
- name: `mutants_guard_error_main_checkout`
  setup: git-dir path pairs of the same shape fuzz.rs's guard tests use —
  the MAIN-checkout shape is the equal pair `("/r/.git", "/r/.git")`; a
  linked worktree has differing `git_dir`/`git_common_dir` paths
  action: `guard_error(&git_dir, &git_common_dir)`
  assert: `Some(msg)` only for the main-checkout pair; msg names the main
  checkout and mentions worktree; `None` for the worktree pair
- name: `mutants_missing_tool_error_contains_hint`
  action: `missing_tool_error()`
  assert: the returned/const string contains
  `cargo install --locked cargo-mutants`
  (fuzz.rs precedent: `run` itself carries zero direct unit tests; the
  guard and hint decisions are pure fns — those are what these tests pin)
- name: `mutants_argv_default_run`
  action: `mutants_argv(&root, &MutantsArgs::default())`
  assert: argv contains `--output <root>/target-mutants`; does NOT contain
  `--file`, `--no-config`, or `--in-diff`
- name: `mutants_argv_file_maps_verbatim`
  action: `mutants_argv` with `file = Some("crates/components/camel-http/src/lib.rs")`
  assert: argv contains `--no-config` AND `--file
  crates/components/camel-http/src/lib.rs` verbatim (no path rewriting);
  `--in-diff` absent; `--output` unchanged
- name: `mutants_argv_diff_maps`
  action: `mutants_argv` with `diff = true`
  assert: argv contains `--in-diff`; `--file`/`--no-config` absent
- name: `mutants_argv_rejects_combined_flags`
  action: `mutants_argv` with `file = Some("p")`, `diff = true`
  assert: `Err` usage message
- name: `mutants_target_dir_derivation`
  action: `target_env(&root)` with synthetic root
  assert: `CARGO_TARGET_DIR == <root>/target-mutants` and nothing else added
  or removed
- name: `survivor_lines_renders_missed_only`
  setup: `outcomes.json` fixture in the PINNED schema (cargo-mutants
  **27.1.0**, e_gpt ruling — see schema note below): root JSON OBJECT;
  outcomes at `.outcomes[]`; three entries: one with
  `.summary == "CaughtMutant"`, one with `.summary == "MissedMutant"`
  (carrying `.scenario.Mutant` with `.file`, `.name`,
  `.function.function_name`), one MissedMutant with
  `.function.function_name` ABSENT (nullable case)
  action: `survivor_lines(&fixture_bytes)`
  assert: `Ok` with exactly two JSON lines, one per MissedMutant entry;
  keys file/function/mutation/status where file ←
  `.scenario.Mutant.file`, function ←
  `.scenario.Mutant.function.function_name` (null-safe), mutation ←
  `.scenario.Mutant.name`, status ← `.summary`; the nullable entry
  renders `"function":null`
- name: `survivor_lines_rejects_malformed`
  setup: four bad fixtures: invalid JSON bytes; valid JSON of an unknown
  shape (no `.outcomes` array); an outcome entry with
  `.scenario.Mutant` missing `.file`; a `MissedMutant` entry with no
  `.scenario.Mutant` at all
  action: `survivor_lines` on each
  assert: `Err` for all four, each message naming the failure class
  (the last names schema drift)
  schema note (PINNED, cargo-mutants 27.1.0, e_gpt ruling): root object
  with outcomes at `.outcomes[]`; survivor ⇔ `.summary ==
  "MissedMutant"` AND `.scenario.Mutant` present — a `MissedMutant`
  entry WITHOUT `.scenario.Mutant` is schema drift → `Err` naming it
  (never silently skipped); file ←
  `.scenario.Mutant.file`; function ←
  `.scenario.Mutant.function.function_name` (NULLABLE when the mutated
  construct has no owning function — render as null, not an error);
  mutation ← `.scenario.Mutant.name`; status ← `.summary`. The install
  and version probe are pinned to 27.1.0 so this schema cannot drift
  silently; if a future bump changes it, fixture + parser + these tests
  change together in one commit.
- name: `classify_exit_maps_classes`
  setup: pinned codes (e_gpt ruling): Some(0), Some(2), each of
  Some(1)/Some(3)/Some(4)/Some(5)/Some(6)/Some(70), None, and
  Some(101) (catch-all arm)
  action: `classify_exit` on each
  assert: Some(0) → `Ok(false)`; Some(2) → `Ok(true)`; each error code,
  None, and Some(101) → `Err` naming the class (unknown exit =
  operational failure). Task 3's smoke cross-checks against the
  real binary (a missed-mutant run must classify `Ok(true)`, not `Err`).
- name: `mutants_version_probe_accepts_only_27_1_0`
  setup: three probe outputs: `cargo-mutants 27.1.0 (...)`,
  `cargo-mutants 26.0.0 (...)`, and malformed `oops`
  action: presence-check decision fn fed by `parse_version_output`
  on each
  assert: 27.1.0 → accepted; 26.0.0 → `Err` naming found version + the
  pinned install command; malformed → `Err` naming the malformation
- name: `mutants_baseline_globs_pinned`
  setup: `.cargo/mutants.toml` from Task 1, resolved via
  `concat!(env!("CARGO_MANIFEST_DIR"), "/../../.cargo/mutants.toml")`
  (cwd under `cargo test` is the xtask package dir — resolve explicitly)
  action: parse with the `toml` crate (workspace dep, confirmed in
  `scripts/xtask/Cargo.toml`)
  assert: `examine_globs` equals the three-entry list verbatim
- command: `cargo test -p xtask mutants` — all pass
- expected: RED before implementation (module absent), GREEN after

**Acceptance**:
- `cargo test -p xtask` all pass (pre-existing tests unmodified; only
  additions).
- `cargo clippy -p xtask --all-targets -- -D warnings` exit 0.
- `cargo fmt --check --all` exit 0.
- `fuzz.rs` behavior unchanged: `cargo test -p xtask fuzz` passes without
  edits to its assertions (guard-helper extraction allowed if
  behavior-preserving).

- [x] 2. xtask-mutants-wrapper

---

## Task 3: end-to-end smoke validation + preliminary baseline

**Files**:
- bd rc-eba8 (updated with measurement notes — no repo files)

**Steps**:
1. Tool provisioning, PINNED to 27.1.0: inside the nix devShell
   (`nix develop`) cargo-mutants 27.1.0 is PROVIDED (Task 1 added it to
   flake.nix; locked nixpkgs ships exactly 27.1.0). Outside nix, install
   once: `cargo install --locked cargo-mutants --version 27.1.0`. The
   wrapper's presence+version check accepts either source identically
   (probe is by PATH).
2. PREBUILD the wrapper first — `.cargo/config.toml` aliases
   `xtask = "run --package xtask --"`, so `cargo xtask ...` may itself
   build xtask and touch `target/` BEFORE the guard runs:
   `cargo build -p xtask`, THEN fingerprint, THEN invoke the built binary
   DIRECTLY (`"$WT/target/debug/xtask" mutants ...`, with `WT="$(pwd)"`
   from step 3) for the isolation-verified run.
3. Record mtime fingerprints for BOTH target trees explicitly (relative
   `target` is ambiguous):
   `WT="$(pwd)"; touch /tmp/mt-stamp; find "$WT/target" /home/kenny/dev/rust-camel/target -newer /tmp/mt-stamp -print | head -20`
   — expect the SAME (empty) result post-run: the stamp predates the run,
   so any instrumented write to either tree shows up as newer.
4. `set -o pipefail; mkdir -p target-mutants && "$WT/target/debug/xtask"
   mutants --json | tee target-mutants/run-baseline.json`
   (pipefail so a masked wrapper failure cannot read as success through
   tee's exit 0; direct binary per step 2; tee INSIDE the already-ignored
   `target-mutants/`; mkdir because tee cannot create parents).
5. Verify isolation: both target-tree fingerprints unchanged; no
   `mutants.out*` at repo root; all artifacts under `target-mutants/`.
6. Record in bd rc-eba8 (append to description or a comment): wall time
   (target < 15 min), kill rate over the three-file baseline, survivor list
   — marked **PRELIMINARY (pre-rc-fvah)**. The formal > 90% baseline and
   rc-eba8 close happen after rc-fvah lands (epic ordering).
7. For each actionable survivor: `bd create "<module>: mutant survived
   <mutation>" -t task -p 3 --deps discovered-from:rc-eba8`.

**Tests**:
- name: end-to-end smoke (manual, command-driven)
  setup: cargo-mutants installed; worktree clean; `target-mutants/` absent
  action: steps 3–4 above
  assert: run completes; exit 0 regardless of survivor count; isolation
  checks pass; JSON output parses as JSON lines
  command: `set -o pipefail; "$WT/target/debug/xtask" mutants --json`
  expected: completes within budget on the dev machine (informational if
  slower — record the actual number). ADDITIONAL cross-check: survivor-line
  count on stdout MUST equal the `"MissedMutant"` count in raw
  `target-mutants/mutants.out/outcomes.json` (catches schema drift between
  the unit fixture and reality). If > 15 min, apply the design fallback as a
  COHERENT cascade in one commit: drop `crates/components/camel-jms/src/
  config.rs` from `.cargo/mutants.toml` baseline globs to the `--file` tier,
  update `mutants_baseline_globs_pinned` + the Task 1 command to the new
  list, amend the spec delta (scenario text) and note it for re-bless, and
  record the change in rc-eba8. No partial application.

**Acceptance**:
- Steps 3–7 executed; rc-eba8 holds the preliminary numbers + survivor bds.
- `git status --porcelain` in the worktree shows NO new tracked files from
  this task (measurement produces no repo changes).
- Anti-gate check (scoped to the gates block, not the whole file):
  `! sed -n '/## QUALITY GATES/,/^## [^Q]/p' AGENTS.md | grep -q
  "xtask mutants"` (documentation mentions elsewhere are fine; the
  subcommand must not be a gate).

- [x] 3. e2e-smoke-preliminary — deviation note: warm 909s > 900s budget,
  cascade NOT applied (human-approved 2026-09-05: coverage + security
  outweigh seconds; threshold effectively ~20min warm). Recorded in
  rc-eba8; note for re-bless at archive time.

---

## Task 4: formal baseline after rc-fvah (BLOCKED until rc-fvah lands)

**Files**:
- bd rc-eba8 (final numbers + close)

**Precondition** (epic ordering, design §Phases): rc-fvah (canonical-path
fuzzing, another agent) has LANDED on main. This task is not started before
that; the change is NOT archived while this checkbox is open.

**Steps**:
1. Rebase/merge the change onto post-rc-fvah main (the kill-rate baseline
   must include rc-fvah's added adversarial tests — that is the point of
   the ordering).
2. Repeat Task 3's MEASUREMENT steps only (1–5: prebuild, fingerprints,
   pipefail run under tee, isolation checks, MissedMutant-count cross-check) on
   the rebased tree.
3. Record FORMAL numbers in rc-eba8: wall time (measured against the
   15-min engineering guidance on the RECORDED local environment — see
   design §budgets), kill rate over the three-file baseline
   (informational target > 90%, enforced nowhere), survivor list.
4. Triage with DEDUP: list existing open issues with
   `--deps discovered-from:rc-eba8` first; create
   `bd create "<module>: mutant survived <mutation>" -t task -p 3 --deps
   discovered-from:rc-eba8` ONLY for survivors not already tracked
   (no duplicate bd issues from the preliminary round).
5. Close rc-eba8 with the formal numbers + scope-decision reference.

**Tests**:
- name: formal-baseline (manual, command-driven — same shape as Task 3)
  command: `set -o pipefail; "$WT/target/debug/xtask" mutants --json | tee target-mutants/run-formal.json`
  assert: completes; stdout survivor lines == raw outcomes.json MissedMutant
  count; both target-tree fingerprints unchanged; numbers recorded in bd

**Acceptance**:
- rc-eba8 closed with formal post-rc-fvah numbers + survivor bds filed.
- Worktree shows no tracked-file changes from this task.
- Gates-block check (Task 3 phrasing) still holds on the rebased tree.

- [ ] 4. formal-baseline-post-rc-fvah
