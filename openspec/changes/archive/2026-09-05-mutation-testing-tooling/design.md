# Design: mutation-testing-tooling

Implements the mutants half of the adjudicated adoption decision (e_opus
2026-08-31, bd rc-4g9j; operative sub-task bd rc-eba8). The fuzz half is out of
scope: phase 1 shipped (rc-a456) and canonical-path fuzzing is owned by
rc-fvah.

## Context

- xtask already has the pattern for guarded tool wrappers: `scripts/xtask/src/fuzz.rs`
  (clap-derive dispatch in main.rs, worktree detection, per-tool target dir,
  `main_checkout_detected` test). The mutants wrapper mirrors that shape.
- `.cargo/config.toml` sets `incremental = false` and no `CARGO_TARGET_DIR` —
  instrumented builds would land in the default `./target` without explicit
  redirection (the cold 75 GB main target must never be touched).
- cargo-mutants has NO function-level filter — only file globs (`examine_globs`)
  and whole-file runs. Measured module sizes drive the two-tier scope:
  ssrf.rs 366 LoC, camel-mqtt config.rs 371, camel-jms config.rs 1122,
  aggregator.rs 2618, camel-file lib.rs 4348, camel-http lib.rs 9103.
- rc-eba8's module set (supersedes the memo's draft list): `camel-api/src/ssrf.rs`,
  `redact_*` helpers in mqtt/jms/http, camel-file `validate_relative_filename`,
  aggregator/resequencer limit enforcement.

## Goals / Non-Goals

Goals: bounded, informational mutation probe with worktree-local disk
isolation, mirroring the shipped `xtask fuzz` wrapper patterns; a measured
baseline (kill rate, < 15 min) recorded in bd.

Non-Goals: gates/thresholds; workspace-scope mutation; fuzz work (rc-fvah);
fixing surviving mutants (follow-up bds); CI jobs (this change ships none —
the probe runs on developer request).

## Real-world behavior decisions (with alternatives)

| # | Decision | Alternative rejected | Why |
|---|----------|---------------------|-----|
| D1 | Wrapper mirrors `scripts/xtask/src/fuzz.rs` shape (guard + forced `CARGO_TARGET_DIR` + tests) | standalone script or bare `cargo mutants` docs | guards must be mechanical; consistency with the shipped, reviewed wrapper |
| D2 | Two-tier scope: baseline `examine_globs` = 3 narrow files; broad files via `--file` | all 6 families in globs | whole-file mutation of 2600–9100 LoC files blows the 15-min budget; the tier split IS the function-family restriction strategy given cargo-mutants has no function filter |
| D3 | Baseline = `camel-api/src/ssrf.rs` + `camel-mqtt/src/config.rs` + `camel-jms/src/config.rs` | larger sets including the camel-http / camel-file / jms-component broad families | ssrf (366) and mqtt (371) are the narrowest; jms config (1122) is the largest acceptable single file — measured criterion validates the choice; every broader family is `--file` tier |
| D4 | Kill rate recorded in bd, consumed by nothing | score ratchet or CI badge | binding anti-obstruction policy: informational only |
| D5 | `target-mutants/` worktree-local, git-ignored, purgeable | shared dir or default `./target` | cold-main rule; same class as `target-fuzz/` |
| D7 | cargo-mutants provisioned by the nix devShell (`pkgs.cargo-mutants`, locked nixpkgs = **27.1.0**, matching the pin) + `cargo install --locked --version 27.1.0` fallback outside nix | cargo-install only (ignore nix) or nix-only without version check | fuzz convention is env-provided tooling; here the wrapper KEEPS presence+version enforcement because the outcomes schema and exit codes are pinned to 27.1.0 — presence-only (fuzz's approach) would let a version drift silently break survivor parsing. No nightly/shim: cargo-mutants runs on the stable toolchain. CI: no mutants job in this change |

## Interfaces

**`xtask mutants [--file P | --diff] [--json]`** — new module
`scripts/xtask/src/mutants.rs`, clap `Commands::Mutants { file, diff, json }`
variant + match arm in `main.rs` mirroring `Fuzz`; the arm constructs
`MutantsArgs { file, diff, json }` and calls `mutants::run(&root, &args)`.
Thin `run` + pure helpers (fuzz.rs idiom — `run` itself carries no direct
unit tests):

1. `run(root: &Path, args: &MutantsArgs) -> Result<(), String>` composes:
   - Pure guard `guard_error(git_dir, git_common_dir) -> Option<String>`:
     refuse the main checkout with the `fuzz` message shape (equal dirs =
     main-checkout shape; linked worktrees differ).
   - Presence check: `cargo mutants --version` probe PINNED to
     cargo-mutants **27.1.0** (schema + exit codes are pinned to that
     version) with EXECUTABLE enforcement — pure
     `parse_version_output(&str) -> Result<String, String>` extracts the
     semver; absent, non-27.1.0, or malformed output → `Err` naming the
     cause + `cargo install --locked cargo-mutants --version 27.1.0`.
   - `#[derive(Default)] pub(crate) struct MutantsArgs { pub(crate) file:
     Option<String>, pub(crate) diff: bool, pub(crate) json: bool }`.
   - Pure `target_env(root) -> Vec<(String, String)>`: exactly
     `("CARGO_TARGET_DIR", "<root>/target-mutants")`.
   - Pure `mutants_argv(root, args) -> Result<Vec<String>, String>`:
     rejects `file`+`diff` combined (Err usage); always passes
     `--output <root>/target-mutants` (cargo-mutants' repo-local
     `mutants.out/` is NOT relocated by CARGO_TARGET_DIR); no flags →
     config defaults; `file=P` → `--no-config --file P` (REQUIRED:
     CLI `--file` merges with `examine_globs` without it); `diff` →
     `--in-diff`.
   - Pure `classify_exit(code: Option<i32>) -> Result<bool, String>`
     (27.1.0 codes, e_gpt ruling): `Some(0)` → Ok(false) all caught;
     `Some(2)` → Ok(true) missed found (still success — informational);
     `Some(1|3|4|5|6|70)` → Err naming the class; `None` (signal death)
     → Err. Survivors NEVER produce Err.
   - Pure `survivor_lines(outcomes_json: &[u8]) -> Result<Vec<String>,
     String>`: parse the PINNED 27.1.0 outcomes schema — root object,
     outcomes at `.outcomes[]`; survivor ⇔ `.summary == "MissedMutant"`
     with `.scenario.Mutant` present; emitted keys: file ←
     `.scenario.Mutant.file`, function ←
     `.scenario.Mutant.function.function_name` (nullable → null, not an
     error), mutation ← `.scenario.Mutant.name`, status ← `.summary`.
     Err on malformed JSON, unknown shape (no `.outcomes`), or missing
     required fields — drift fails loudly.
   - Stdout ownership: in JSON mode the child's stdout is captured (not
     inherited), human diagnostics forwarded to stderr; stdout carries
     ONLY wrapper JSONL so `tee` output stays parseable.
   - Exit mapping: survivors never affect success; `Err` only for guard,
     missing tool, spawn failure, operational-failure exit classes, or
     schema drift on a successful run.

**`.cargo/mutants.toml`**:
```toml
examine_globs = [
  "crates/camel-api/src/ssrf.rs",
  "crates/components/camel-mqtt/src/config.rs",
  "crates/components/camel-jms/src/config.rs",
]
```
No `timeout_multiplier` in v1 (defaults; the 15-min figure is a measured
criterion, not an enforced timeout — add one only if measurement demands).

**`.gitignore`**: add `/target-mutants/`.

## Verification test matrix

| Test name | Arrange | Act | Assert |
|---|---|---|---|
| `mutants_guard_error_main_checkout` | git-dir path pairs: main-checkout shape (`git_dir == git_common_dir`) vs linked-worktree shape (differing dirs) | call pure `guard_error(&git_dir, &git_common_dir)` | `Some(msg)` only for the main pair, msg names main checkout + worktree; `None` for the worktree pair |
| `mutants_missing_tool_error_contains_hint` | none (pure const/fn) | call `missing_tool_error()` | string contains `cargo install --locked cargo-mutants` |
| `mutants_target_dir_derivation` | synthetic root path | call pure `target_env(&root)` | exactly one pair: `("CARGO_TARGET_DIR", "<root>/target-mutants")` |
| `survivor_lines_rejects_malformed` | three bad fixtures: invalid JSON; unknown shape (no `.outcomes`); entry missing `.scenario.Mutant.file` | call `survivor_lines` on each | `Err` per fixture, message names the failure class |
| `classify_exit_maps_classes` | pinned codes Some(0)/Some(2)/Some(1\|3\|4\|5\|6\|70)/None | call `classify_exit` | Some(0)→Ok(false), Some(2)→Ok(true), rest and None→Err naming the class |
| `mutants_version_probe_accepts_only_27_1_0` | probe outputs: 27.1.0, 26.0.0, malformed | parse + presence decision | 27.1.0 accepted; others → Err naming cause + pinned install command |
| `mutants_argv_default_run` | synthetic root + default args | call pure `mutants_argv(&root, &args)` | contains `--output <root>/target-mutants`; no `--file`/`--no-config`/`--in-diff` |
| `mutants_argv_file_maps_verbatim` | `file = Some("crates/components/camel-http/src/lib.rs")` | call `mutants_argv` | `--no-config` AND verbatim `--file` present; `--in-diff` absent |
| `mutants_argv_diff_maps` | `diff = true` | call `mutants_argv` | `--in-diff` present; `--file`/`--no-config` absent |
| `mutants_argv_rejects_combined_flags` | `file = Some("p")`, `diff = true` | call `mutants_argv` | `Err` usage message |
| `survivor_lines_renders_missed_only` | outcomes.json fixture (documented cargo-mutants shape): one Caught + one Missed | call pure `survivor_lines(&bytes)` | exactly one JSON line (the Missed entry) with keys file/function/mutation/status |
| `mutants_baseline_globs_pinned` | `.cargo/mutants.toml` resolved via `CARGO_MANIFEST_DIR/../../` | parse with toml crate | `examine_globs` equals the three-entry list verbatim |
| `target_mutants_git_ignored` | repo root | `git check-ignore target-mutants` | exit 0 |

Exit mapping (survivors → exit 0, operational failure → non-zero) lives in
`run`'s composition and is verified by Task 3's end-to-end smoke — fuzz.rs
precedent: `run` itself carries no direct unit tests.

Every row except `target_mutants_git_ignored` (command-verification) is an
`#[test]` fn in `scripts/xtask/src/mutants.rs` exercising a pure helper with
synthetic inputs — no stubbed runners, no real cargo-mutants. tasks.md
carries their full executable specs.

## Cross-task interactions / emergent risks

- **Kill-rate scope (authoritative decision)**: rc-eba8's "> 90% killed"
  criterion applies to the THREE-FILE BASELINE only. Whole-file mutation of
  the broad tier dilutes the rate (most mutants land in non-security
  functions the audit's adversarial tests never targeted), making a
  broad-file percentage meaningless. Broad families are measured
  opportunistically per `--file` run; their survivor LISTS (not rates) are
  the actionable output. This scoping is recorded in bd rc-eba8's
  description.
- **cargo-mutants test-selection**: mutants runs the CRATE's tests per mutant
  (camel-api, camel-mqtt, camel-jms suites) — not the whole workspace; budget
  risk is camel-jms (largest suite of the three). Measurement task validates;
  if > 15 min, drop jms from baseline globs to `--file` tier — as a coherent
  cascade per tasks.md Task 3 (toml + pinned-globs test + Task-1 command +
  spec-delta amendment for re-bless + bd record, all in one commit; no
  partial application).
- **Lockfile**: cargo-mutants is dev-installed; no dependency enters any
  manifest. `.cargo/mutants.toml` is config, not manifest.
- **Gates**: `mutants.rs` is xtask code — runs under `cargo clippy -p xtask`
  and xtask's own tests like every other subcommand; the SUBCOMMAND itself is
  absent from `QUALITY GATES`.

## Phases

Single-phase deliverable: one coherent tool (wrapper + config + ignore +
measured baseline). No phase split — the whole change lands as one reviewable
unit. Ordering constraint (rc-4g9j epic notes, e_opus 2026-09-03): rc-eba8
runs AFTER rc-fvah. The TOOLING in this change may land on main
independently; the baseline MEASUREMENT and rc-eba8's close happen only after
rc-fvah (canonical-path fuzzing) completes, so the kill-rate baseline
includes the adversarial tests rc-fvah adds.

## Open questions

- None blocking. (Whether a periodic CI probe is ever wanted is a future
  decision; this change ships none.)
