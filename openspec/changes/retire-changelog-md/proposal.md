# Proposal: retire-changelog-md

## Why

ADR-0049 requires release notes rendered from Conventional Commit subjects. The
hand-maintained `CHANGELOG.md` duplicates that mechanism without an owner-approved process.
It contains only `## Unreleased`, and no release consumes it. The v0.66.0 GitHub Release
already carries generated notes from `cargo xtask changelog`. The manual file duplicated 94
commit subjects into 28 bullets and contributed to the root `.bless.json` incident in mission
540. Owner decision 2026-11-08 and bd `rc-hjcyq` authorize its retirement after v0.66.0.

## What Changes

- Delete `CHANGELOG.md`.
- Update ADR-0096 to name the shipped HTTP/REST fix subjects and generated release notes.
- Update ADR-0034 to name the release notes as the migration record.
- Update the JMS durable-subscriber specification to require both features in generated
  release notes.
- Keep `scripts/xtask/src/bridge_versions/scan.rs` unchanged. Its changelog guards remain
  valid.

Affected crates: none. This change modifies documentation and one canonical OpenSpec spec.
It excludes release workflows, changelog generation, archived changes, ignore rules, and
replacement documents.

## Acceptance criteria

- `CHANGELOG.md` is absent.
- No live `CHANGELOG` reference remains, except the excluded bridge-version scanner.
- All three references name the commit-derived release-notes mechanism.
- `cargo test -p xtask --lib` passes.
- `cargo xtask changelog --from v0.65.0 --to v0.66.0` renders release notes.
- `bash scripts/gates-local.sh` exits 0.

## Risk budget

Accept only documentation and process-metadata edits needed for this retirement. Do not
rewrite archived history or modify Rust source. Stop if verification requires release
workflow or `xtask changelog` changes.

### Self-grill record

**Questions generated:**
1. [glossary] Does “release notes” match the canonical vocabulary?
2. [sharpen] Does “migration record” imply a new migration guide?
3. [scenario] What happens when an operator follows each reference after file deletion?
4. [cross-ref] Does the workflow already generate notes, and do the named commits exist?

**Answers (with citations):**
1. `CONTEXT-MAP.md` has no competing changelog or release-notes entry. ADR-0049 uses
   “release notes” for `cargo xtask changelog` output (`docs/adr/0049-workspace-non-exhaustive-policy-for-v1-contract-enums.md:205-215`).
2. No new guide is needed. The order names generated release notes and forbids a replacement
   document (mission order lines 23-32, 81-85).
3. Unedited references would dangle. Retarget all three, then search outside archived changes
   and the scanner guard (mission order lines 35-40).
4. The workflow runs `cargo xtask changelog` and passes its output to the GitHub Release
   (`.github/workflows/release-matrix.yml:358-376,439-443`). Git records the actual subjects
   as `fix(http): map non-request-borne conversion failures to 500` (`ad068811b`) and
   `fix(rest): map response marshal errors to 500` (`4fa07a23c`). The proposal therefore uses
   these shipped subjects, not the order's combined scope.

**Outcome:** refine — cite the actual shipped commit subjects.
**Self-grill mode:** self-grill-proposals skill
