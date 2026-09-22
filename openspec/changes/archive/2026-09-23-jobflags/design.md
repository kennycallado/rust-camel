# Design: jobflags

## Approach

Two-phase in-process clap re-parse (research Option A, owner-approved;
clap 4.6.6, no new dependency):

- **Phase 1 (capture).** `JobArgs` gains a trailing catch-all field:
  `#[arg(trailing_var_arg = true, allow_hyphen_values = true,
  num_args = 0..)] dynamic: Vec<OsString>`. The top-level
  `Cli::parse()` stays intact; tokens after the positional document
  path land in `dynamic`. Existing flagless invocations are unchanged
  (`num_args = 0..` allows an empty tail).
- **Phase 2 (authoritative).** After the document is read and its
  declarations are known, build `JobArgs::augment_args(Command::new(
  "camel job"))`, re-assert `disable_help_flag`/`disable_version_flag`,
  keep `infer_long_args` off, then add one runtime `Arg` per declared
  argument under COLON-NAMESPACED IDs (`dyn:<name>`, `dyn:no:<name>`
  for bool negation — `:` cannot appear in an argument identifier, so
  a declared arg named `document`, `dynamic`, or `no_<bool>` can
  never collide with an ID or another runtime arg; the `--` longs
  stay verbatim): `ArgAction::Set` + `allow_hyphen_values(true)` for
  string/int/enum; `ArgAction::SetTrue` plus a sibling `--no-<name>`
  `ArgAction::SetFalse` for bool. Bool presence is detected by
  command-line `value_source`, never `get_flag` truth (`SetFalse`
  implies a `true` default). The rebuilt command sets
  `no_binary_name(true)` (the tail iterator carries no argv[0] —
  without it clap swallows the first tail token as the binary name)
  and `args_override_self(true)` (repeated flags are last-wins;
  without it clap rejects repeats with `ArgumentConflict` — both
  probed). Re-parse the dynamic tail with
  `try_get_matches_from_mut` (never exits; `DisplayHelp` returned as
  `Err`).
- **Lowering.** Map phase-2 matches to `Vec<(String, String)>` pairs
  (`--name world` → `("name","world")`; bare bool → `("flag","true")`;
  `--no-flag` → `("flag","false")`), merge with `--arg` pairs, then
  call the UNCHANGED `parse_job_document_with_args`. Coercion,
  defaults, required checks, interpolation, reports: untouched.
  `lower_dynamic_flags` in `mod.rs` owns: cross-form conflict
  detection (same key via flag and `--arg` → error naming the key),
  `--flag --no-flag` contradiction error, `--flag=false` rejection
  (clap error detected and re-worded to name `--flag`, `--no-flag`,
  `--arg flag=false`), stray positional rejection, and no-`args:`
  hard error (nothing declares dynamic flags → any tail token that is
  neither static nor empty = error pointing at `args:` / `--arg`).
- **Reserved names.** `normalize_job_args` rejects `help`, `config`,
  `report`, `arg` with new `JobDocError::ReservedArgumentName` at load
  time (execution AND `--help` AND compile validation — all share
  `normalize_job_args`). Prevents the clap duplicate-long panic before
  phase 2 can ever see it.
- **Help.** `render_job_help` gains one note line under `Arguments:`
  (only when arguments exist) documenting `--<name> <VALUE>`,
  `--<name>` / `--no-<name>` for bool, and `--arg <name>=<value>`.
- **Ordering invariant.** Lowering runs AFTER `normalize_job_args`
  (declaration errors win) and BEFORE `resolve_job_args`/interpolation
  (`${arg:}` sees final values). Exit-2 precedence ladder intact.

**Pinned clap tail semantics** (probed in this worktree against the
workspace-resolved clap 4.x — 4.6.6 at authoring time; the manifest
floats 4.x minors, so the byte-pinned tests in the plan are the real
drift guard; scratch project under ignored `target/probe/`):
1. Static flags after the document path parse STATICALLY — `--arg`,
   `--config`, `--report`, `--help` after the path keep their meaning
   with zero reconciliation (back-compat is native).
2. The raw dynamic tail starts at the FIRST unknown token: everything
   from there on (including later static flags) is captured raw into
   `dynamic`. Phase 2 (which declares the static set via
   `augment_args`) recovers tail `--arg` pairs (merged into the
   channel, argv order, last-wins), tail `--help`/`-h` (OR with
   phase-1 help, manual help path), and tail `--report` (last
   occurrence wins).
3. A tail `--config` is honored by a targeted exact-token pre-scan of
   `args.dynamic` (`--config X` / `--config=X`) BEFORE config load,
   with the last occurrence on argv winning — and the resolved path
   threads through ALL `args.config` call sites (config load, the
   `jobs_roots` canonical-root anchor, and `JobRun.project_root`), so
   bare-name discovery follows the effective config.
4. Flags BEFORE the document path: phase-1 unknown-argument error
   with clap's standard diagnostic (owner decision: path first,
   flags after — the natural clap shape). A tail `--help`/`-h` is
   exact-token pre-scanned BEFORE `JobSignals::arm`'s gate (document
   && !help && !tail-help), so the recovered help path never runs
   with signal streams armed.
5. Terminator: the FIRST bare `--` after the `job` token ends flag
   parsing for everything after it; post-`--` tail tokens are
   literals and exit 2 as unexpected positionals. The boundary is
   computed from RAW argv (`tail_terminator_boundary`): the `--`
   itself is clap-stripped when it precedes tail capture
   (`doc -- --name x` captures `["--name","x"]`) but survives inside
   an already-started tail (`doc --name w -- --literal`), where the
   surviving terminator belongs to neither side of the split. Known
   ambiguity: a bare `--` in VALUE position (`--name --`) resolves
   as the terminator by this rule, so the flag errors
   "requires a value"-class — deterministic and loud. Phase 2 still
   rejects any stray positional token that fills the rebuilt
   command's `document` slot ("unexpected positional", exit 2) —
   this is also what rejects `--flag false`'s stray `false`. A
   legacy (no-`args:`) document maps phase-2 unknown-`--flag` errors
   to the no-declaration diagnostic.
6. Help-steal: an exact `--help`/`-h` token anywhere in the dynamic
   tail always wins (rendered help, exit 0) even in value position
   after a value-taking flag (`--name --help`) — a deliberate
   divergence from `--report`/`--config`, which bind as values there
   (help is the likely user intent; pinned by `tail_has_help` /
   phase-2 pre-scan agreement).

## Affected crates

- `camel-cli` only: `commands/job/mod.rs` (JobArgs field,
  `lower_dynamic_flags`, `run_job` wiring, argv-merge), 
  `commands/job/document.rs` (`ReservedArgumentName` variant),
  `commands/job/help.rs` (note line), tests in
  `tests.rs`/`document_tests.rs`/`help_tests.rs` (+ binary-level tests
  through the existing subprocess harness; e2e in
  `crates/camel-cli/tests/job_one_shot_test.rs` where the report is
  asserted). Embedded/compiled artifact paths pass empty pairs and are
  untouched.

## Architecture boundaries

Pure CLI front-end (control-plane argv seam); converges on the
existing pair channel, so Runtime/DSL/Components/Services are
untouched. Coercion stays in `document.rs` (clap does shape-only
capture) per research Q7 — keeps canonical-string production and
`ArgumentCoercion` wording byte-stable. Precedent for reserved names:
ADR-0062's reserved-suffix contract (camel-dsl + camel-cli). Exit
taxonomy: cli-jobs spec `exit-code taxonomy` (all new errors exit 2).

Single phase: one coherent slice, one crate, ~600 lines with tests.

## Alternatives considered

Rejected per research: external-subcommand tail hand-parsing (no
byte-exact clap diagnostics), manual argv pre-scan (bypasses clap
rules), external crate (none mature; owner bans improvisation),
`--arg`-only sugar (misses the feature). Bool `require_equals`
three-state form rejected (owner: set-true/set-false spellings only).
