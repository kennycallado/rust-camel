# Design: archengine — scenario rename/drop via xtask archive wrapper

Reviewed: e_glm pre-flight APPROVE-WITH-FINDINGS (ses_f20710e69ffeaBG1t3N1AwgVYA);
all six findings folded below.

## 1. Engine facts this design depends on (verified on 1.7.0 dist)

- Guard: `findMissingCurrentScenarios` (specs-apply.js) — every scenario name
  in the current canonical block must appear in the delta MODIFIED block
  (multiplicity-aware); missing → hard abort. The MODIFIED block then
  replaces the canon block WHOLESALE (raw verbatim).
- Early-sync idempotency is first-class: identical normalized block raws →
  op count 0; all-zero counts → the canon file write is skipped entirely.
- Archive (JSON mode) prepares ALL spec updates with zero writes, aborts
  cleanly on any error, validates every rebuilt spec before writing, then
  moves the change dir to `openspec/changes/archive/YYYY-MM-DD-<name>`.
- Delta parser: unknown `##` H2 sections are silently UNREAD — directives
  must never be H2 headings. Inside a requirement block, non-H2/H3 lines are
  inert body text. HTML comments are inert to the delta validator (fences
  are masked, comments are not, but a comment without `#### Scenario:` or
  SHALL/MUST content is invisible to every check that matters).
- Canonical specs already carry HTML comments today
  (`openspec/specs/http-test-harness/spec.md:51`) — proven compatible with
  validate + archive + rendering.
- EMPIRICAL PROBE (2026-09-26, scratch root, upstream 1.7.0): a delta whose
  MODIFIED block carries the directive comment (rename + justified drop)
  passes `openspec validate <change>`; a canon pre-synced to that block
  verbatim passes `openspec validate <cap> --type specs`; `openspec archive
  <chg> --json --yes` exits 0 with `specsUpdated: false`, all totals 0, the
  change dir moved to archive/, and canon byte-identical to the pre-sync.
  The all-zero-archive path (r_glm finding 2) is confirmed by execution.
- 120 canonical specs.

## 2. Directive syntax

Inside a delta's MODIFIED requirement block only:

    ### Requirement: Existing Feature
    Body text with SHALL ...

    <!-- openspec-scenario-ops
    renamed: Old scenario header -> New scenario header
    dropped: Some scenario | one-line justification (required, non-empty)
    -->

    #### Scenario: New scenario header
    - **WHEN** ...

Rules:
- At most ONE directive comment per requirement block; a directive comment
  outside any requirement block in a delta file is a hard error for the
  wrapper (only delta files are scanned — canon is never checked for stray
  directives).
- `renamed:` old and new names must not contain `->`; `dropped:` name must
  not contain `|`. Directive lines must not contain `#### Scenario:` or the
  words SHALL/MUST (keeps the comment inert to the delta validator).
- Drop without a non-empty justification → reject. No silent drops, ever.
- Scenario-name comparison is exact match on trimmed header text — mirrors
  upstream (`headerMatch[1].trim()`, byte comparison after trim). No
  case-folding, no whitespace normalization.

## 3. Wrapper flow — `cargo xtask archive <change> [--check]`

1. Resolve the openspec root (walk up from workspace root; the wrapper
   always operates on `<root>/openspec`).
2. Run `openspec validate <change> --json` FIRST — any validation error
   aborts before a single byte is written (atomicity window shrink,
   e_glm finding 1).
3. Discover the change's delta specs (recursive `specs/**/spec.md` walk,
   port of upstream discovery: skip dot-dirs, root-level spec.md ignored).
4. Parse MODIFIED blocks for directive comments.
   - None anywhere → passthrough: exec `openspec archive <change> --json
     --yes` and propagate exit code + stdout (today's behavior, unchanged).
5. Guard matrix (all checks before any write; violations exit 1 with
   diagnostics naming requirement, scenario, and rule):
   - R1 `renamed` from-name must exist in the canon block's scenarios with
     count == 1. Already-applied iff from ∉ canon ∧ to ∈ canon → no-op;
     that idempotent branch short-circuits R1's count check and R6, in
     addition to R2. from ∈ canon ∧ to ∈ canon (both present) → reject as
     ambiguous — never a silent skip (r_glm finding 1).
   - R2 `renamed` to-name must exist among the delta block's scenarios,
     must differ from the from-name, and must NOT exist in the canon
     block's scenarios — this rule is checked only for ops NOT already
     applied (an `AlreadyApplied` op short-circuits R2; idempotent re-run
     support).
   - R3 `dropped` name must exist in the canon block's scenarios with
     count == 1 (absent → already-applied no-op); justification non-empty.
   - R4 Multiplicity guard (e_glm finding 2): a from/dropped name appearing
     more than once in canon is rejected — rename-one-of-many is
     unarchivable under adjusted-superset semantics.
   - R5 Adjusted superset (port of upstream algorithm, multiplicity-aware):
     every canon scenario not in {renamed-from ∪ dropped} — counting ops
     already applied, whose to-names are simply expected present — must
     appear in the delta block's scenarios.
   - R6 No stale carries: renamed-from and dropped names must NOT appear
     among the delta block's scenarios (skipped for already-applied ops —
     see R1's idempotent branch).
   - R7 Combination guard (e_glm finding 3): a requirement-level RENAMED
     section in the same delta spec file → reject (v1 does not reason about
     rename-then-modify keying).
   - R8 Structure: at most one directive comment per block; none outside
     requirement blocks; unknown lines inside the directive comment are
     rejected (no silently ignored directives).
   - R9 The MODIFIED requirement must exist in canon — clear wrapper error,
     not a passthrough to the upstream "not found".
   - R10 Directive content invariants (delimiter collisions, scenario-header
     and SHALL/MUST absence) as in section 2.
   - Two renames targeting the same to-name → reject.
6. Pre-sync canon: for each directive-bearing requirement, replace the canon
   block with the delta MODIFIED block VERBATIM (directive comment included
   — permanent audit trail; inert to the engine and to markdown rendering).
   The splice ports upstream `extractRequirementsSection` semantics
   (code-fence-aware block boundaries; blocks joined with blank lines;
   `\n{3,}` → `\n\n`) so the file is normalization-equivalent to what the
   engine itself would write. Byte-exactness is NOT required beyond that
   (e_glm finding 6): worst case the engine rewrites the file with correct
   content.
7. Exec `openspec archive <change> --json --yes`, propagate exit code and
   stdout. On engine failure AFTER pre-sync, print a loud note: canon was
   pre-synced; the fix is idempotent — correct the delta and re-run
   `cargo xtask archive <change>` (e_glm finding 1).
8. `--check` mode: run steps 2–5 only (guard verification without writes) —
  usable at spec-review time to prove directives parse and hold.

## 4. Idempotency

Re-running the wrapper after a partial failure, or on an already-pre-synced
canon, must be a no-op for directive ops (R1/R3 already-applied branches)
and must not double-write. The engine's own early-sync handling covers the
remainder (identical blocks → zero counts → write skip).

## 5. Tests

Unit (always run, in `scripts/xtask`):
- Directive parser: well-formed comment → ops; malformed lines, unknown
  lines, missing justification, delimiter collisions → errors.
- Guard matrix: one accept case per rule path, one reject case per R1–R10,
  idempotent already-applied branches, multiplicity cases.
- Splice: normalization-equivalent block replacement; fence-masked
  `### Requirement:` lines inside code examples are NOT block boundaries.
- Passthrough: no directives → no canon read-modify-write at all.

Integration (`#[ignore]`, need the real `openspec` on PATH; run explicitly
at mission gate — CI wiring is a recorded deferral):
- Scratch openspec root in a tempdir: canon spec with 3 scenarios; delta
  MODIFIED renaming one + dropping one (with justification) → wrapper
  archive → assert exit 0, canon carries new names, old/dropped names
  absent, change dir moved under archive/.
- Strict-negative: delta silently dropping a scenario (no directives) →
  upstream refusal message preserved.
- `openspec validate`: MODIFIED + REMOVED same requirement name still
  errors (AC#2).
- Idempotent: second wrapper run on already-synced tree → no-op success.

## 6. Docs

- Usage note in `.opencode/skills/openspec-archive-change/SKILL.md` at the
  delta-sync assessment step: scenario renames/drops go through
  `cargo xtask archive <change>` with the directive comment; syntax block
  included.
- The subcommand's clap `--help` text is the other discoverable surface.
- No CONTEXT-MAP entry (tooling-only, e_glm concurred).

## 7. Non-goals / deferrals

- Upstream (Fission-AI/OpenSpec) contribution of native scenario-level ops —
  follow-up bd at park.
- CI wiring for the ignored integration tests — follow-up bd at park
  (mission gate runs them manually).
- One-line cross-ref in the `openspec-apply-change` skill (authoring-time
  discoverability of the directive syntax; r_glm minor) — follow-up bd at
  park.
- Rename-one-of-duplicate-scenario-names (R4 rejects; requires upstream
  positional semantics).
- Requirement-level RENAMED + scenario-ops in one delta file (R7 rejects v1).

## 8. Task breakdown (worker-dispatchable, ≤30 min each)

1. `scripts/xtask/src/archive.rs` — directive comment parser + data model
   + parser unit tests.
2. Guard matrix functions + scenario/block extraction (fence-aware) +
   guard unit tests (reject cases for R1–R10 incl. both-present ambiguity).
3a. Canon splice + root/discovery resolution + splice unit tests.
3b. CLI wiring (`Archive` subcommand in main.rs, `--check` flag), exec
    passthrough, recovery messaging, R7/R8/R9 rejects + tests.
4. `#[ignore]` integration tests on scratch openspec root (happy, negative,
   validate-conflict, idempotent).
5. Docs: SKILL.md usage note + skip_specs marker verification + final gate
   run (fmt, clippy legs, unit tests, ignored tests, scratch-dir demo,
   specs validate 120/120).
