# Tasks: archengine

## Task 1 — Directive comment parser

**Files:** `scripts/xtask/src/archive.rs` (new), `scripts/xtask/src/main.rs` (mod line)

**Steps:**
- Create `archive.rs` with a data model: `ScenarioOp { Renamed { from, to }, Dropped { name, justification } }`,
  `DirectiveComment { ops: Vec<ScenarioOp> }`.
- Implement `parse_directive_comment(lines: &[String]) -> Result<Option<DirectiveComment>, ArchiveError>`:
  scans a requirement block's raw lines for exactly one
  `<!-- openspec-scenario-ops` ... `-->` span; parses `renamed: <from> -> <to>`
  and `dropped: <name> | <justification>` lines; rejects unknown directive
  lines, missing justification, names containing `->` / `|`, directive lines
  containing `#### Scenario:` or SHALL/MUST.
- Error type `ArchiveError` with requirement/scenario/rule context, `Display`
  impl fit for conductor consumption.

**Tests (unit, in `archive.rs` `#[cfg(test)]`):**
- `parse_renamed_and_dropped` — arrange: block lines with one renamed + one
  dropped op; act: parse; assert: ops match, order preserved.
- `parse_rejects_unknown_directive_line` — unknown `foo:` line → Err naming
  the line.
- `parse_rejects_drop_without_justification` — empty text after `|` → Err.
- `parse_rejects_delimiter_collision` — from-name containing `->` → Err.
- `parse_rejects_scenario_header_in_directive` — `#### Scenario:` inside the
  comment → Err.
- `parse_none_when_no_comment` — plain block → Ok(None).
- `parse_rejects_second_comment` — two directive comments in one block → Err.

**Acceptance:** parser unit tests pass; `cargo clippy -p xtask -- -D warnings`
clean; `cargo fmt` clean.
- [x] 1

## Task 2 — Guard matrix

**Files:** `scripts/xtask/src/archive.rs`

**Steps:**
- Implement fence-aware block/scenario extraction:
  `split_requirement_blocks(spec_md) -> Vec<RequirementBlock>` (port of
  upstream `extractRequirementsSection` boundaries: `## Requirements`
  section, `### Requirement:` headers, code-fence masking) and
  `scenario_names(block_raw) -> Vec<String>` (`#### Scenario:` headers,
  fence-masked).
- Implement `check_guard(canon_block, delta_block, directive, ctx) ->
  Result<GuardOutcome, Vec<ArchiveError>>` enforcing design rules R1–R10
  (idempotent already-applied branches included; `GuardOutcome::{Apply,
  AlreadyApplied}`).

**Tests (unit):**
- `guard_accepts_rename_and_drop` — canon has A,B,C; delta renames A→A2,
  drops B with justification, carries C → Ok(Apply).
- `guard_rejects_missing_from` (R1), `guard_rejects_to_in_canon` (R2),
  `guard_rejects_missing_justification` (R3), `guard_rejects_duplicate_target`
  (R4 — canon A×2), `guard_rejects_missing_carry` (R5 — C absent from delta),
  `guard_rejects_stale_carry` (R6 — delta still lists A), `guard_rejects_rename_eq`
  (R2 — A→A), `guard_rejects_two_renames_same_target`,
  `guard_rejects_from_and_to_both_in_canon` (R1 — ambiguous, never silent skip).
- `guard_already_applied` — canon has A2, no A → Ok(AlreadyApplied); R2 not
  consulted for that op.
- `guard_ignores_fenced_headers` — `### Requirement:` / `#### Scenario:`
  inside fenced code are not boundaries/names.

**Acceptance:** all guard unit tests pass; clippy/fmt clean.
- [x] 2

## Task 3a — Canon splice + discovery

**Files:** `scripts/xtask/src/archive.rs`

**Steps:**
- `splice_block(canon_md, requirement_name, replacement_raw) -> String`:
  replace the named requirement block verbatim; recompose the Requirements
  section port-of-upstream (blank-line joins, `\n{3,}`→`\n\n`); byte-stable
  for untouched blocks.
- Root + discovery: resolve `openspec/` from workspace root; recursive delta
  spec discovery mirroring upstream (dot-dirs skipped, root spec.md ignored).

**Tests (unit):**
- `splice_replaces_block_verbatim` — replacement raw lands byte-for-byte;
  sibling blocks untouched.
- `splice_normalizes_blank_runs` — triple newlines collapse; result is
  normalization-equivalent (trimmed-block comparison).
- `discovery_finds_nested_specs` — `specs/<a>/<b>/spec.md` discovered with
  id `<a>/<b>`; dot-dirs and root-level spec.md skipped.

**Acceptance:** splice/discovery unit tests pass; clippy/fmt clean.
- [x] 3a

## Task 3b — CLI wiring, passthrough exec, recovery

**Files:** `scripts/xtask/src/archive.rs`, `scripts/xtask/src/main.rs`
(`Archive` subcommand)

**Steps:**
- CLI: `cargo xtask archive <change> [--check]`; flow per design section 3
  (validate gate → parse → guard → splice → exec `openspec archive <change>
  --json --yes`); no-directive passthrough; post-pre-sync failure recovery
  note; `--check` stops after guards (no writes).
- Rejects with named errors + tests: R7 (requirement-level RENAMED section
  in the same delta file), R8 (directive comment outside any requirement
  block; second comment in one block), R9 (MODIFIED requirement absent
  from canon).
- main.rs delta stays minimal: enum variant + dispatch arm (main.rs is
  pre-existing 8k+ lines — do not grow it).

**Tests (unit):**
- `rejects_renamed_section_combo` (R7) — delta file with RENAMED section +
  scenario-ops directive → Err before any write.
- `rejects_directive_outside_block` (R8) — directive comment between
  requirement blocks → Err.
- `rejects_missing_requirement` (R9) — MODIFIED requirement not in canon →
  clear wrapper error, no upstream exec.
- `passthrough_when_no_directives` — exec receives `--json --yes`; canon
  file not opened for write.
- `check_mode_no_writes` — `--check` returns guard verdict, canon mtime
  unchanged.

**Acceptance:** unit tests pass; `cargo xtask archive --help` renders usage;
clippy/fmt clean.
- [x] 3b

## Task 4 — Integration tests on scratch root

**Files:** `scripts/xtask/tests/archive_e2e.rs` (new)

**Steps:**
- `#[ignore]` tests creating a tempdir scratch openspec root (`openspec/
  specs/<cap>/spec.md` canon + `openspec/changes/<chg>/specs/<cap>/spec.md`
  delta + minimal tasks/proposal files so upstream validate passes).
- Drive the wrapper's library functions + exec the real `openspec` binary
  with cwd = scratch root.

**Tests:**
- `e2e_rename_and_drop_archives` — 3-scenario canon; delta renames one,
  drops one (justified); run wrapper; assert exit 0, canon shows new names,
  old/dropped absent, change dir moved to archive/.
- `e2e_strict_refusal_preserved` — delta silently omits a scenario (no
  directives); wrapper exits non-zero carrying the upstream "not present in
  the modified block" refusal.
- `e2e_validate_modified_removed_conflict` — same requirement in MODIFIED
  and REMOVED → `openspec validate` still errors (AC#2).
- `e2e_idempotent_rerun` — run wrapper twice; second run no-ops successfully.

**Acceptance:** `cargo test -p xtask -- --ignored` green (mission gate);
unit suite still green.
- [x] 4

## Task 5 — Docs + marker + gates

**Files:** `.opencode/skills/openspec-archive-change/SKILL.md`,
`openspec/changes/archengine/.openspec.yaml`

**Steps:**
- Add the usage note at the delta-sync assessment step of the SKILL: when a
  MODIFIED block must rename or drop a scenario, author the
  `openspec-scenario-ops` directive comment and archive through
  `cargo xtask archive <change>`; include the syntax block and the
  justification requirement.
- Set `skip_specs: true` in the archengine change's `.openspec.yaml`.
- Run the mission gates: `cargo fmt --check --all`; clippy legs covering
  xtask; `cargo test -p xtask`; `cargo test -p xtask -- --ignored`;
  scratch-dir demo recorded; `openspec validate --json` — 120/120 specs
  untouched/valid.

**Acceptance:** SKILL diff reviewed; gates green and recorded in the park
report.
- [x] 5
