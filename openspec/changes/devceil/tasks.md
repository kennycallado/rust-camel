# Tasks: devceil

Retrospective record: every task below is verification work already
performed and evidenced in design.md. Boxes are checked; no code, no
workflow edits.

## Task 1 — Permissions union verification

- [x] 1.1 Compute the union of nested job permission requests in
  `release-matrix.yml` and confirm the `release-dev.yml` ceiling covers
  it.

**Files:** none — read-only verification; the record itself lives in
`openspec/changes/devceil/design.md` (section 1).

**Steps:**

1. Read `.github/workflows/release-matrix.yml`, `release.yml`,
   `release-dev.yml` at main dbff0a90; record every `permissions:`
   block with line refs.
2. Union the requests: build `contents:read` (L24, inherits L17-18) ∪
   release `contents:write` (L341, L346-347) ∪ docker
   `contents:read + packages:write + id-token:write` (L448, L452-455)
   ∪ closure-check `contents:read` (L733, inherits L17-18) =
   `{contents: write, packages: write, id-token: write}`.
3. Compare with `release-dev.yml` L17-20.

**Acceptance:**

- Union matches the dev wrapper trio exactly, no surplus, no gap
  (design.md section 1.3-1.5).
- Every claim carries a line ref valid at dbff0a90.
- No workflow file modified.

## Task 2 — Homing artifact location

- [x] 2.1 Locate the release-publisher-homing change artifacts and
  explain the bd "not found" symptom.

**Files:** none — read-only verification; findings recorded in
`openspec/changes/devceil/design.md` (section 2).

**Steps:**

1. Search `openspec/changes/` (active + archive) for the homing
   artifacts: found at
   `openspec/changes/archive/2026-09-18-release-publisher-homing/`.
2. Trace the landing commit: 83870932, 2026-09-18 19:16:34 +0200, with
   the preceding commits 02eec1be → b56f9681 → 9551088c.
3. Identify the root cause of the miss: the dev-perms-hotfix worktree
   was cut from a base predating the archive commit.

**Acceptance:**

- Artifacts confirmed present on main since 2026-09-18 19:16; no
  re-baseline fabrication performed.
- Timeline table with commit shas and timestamps recorded
  (design.md section 2).

## Task 3 — T1 re-baseline

- [x] 3.1 Verify the homing Task 1 "release-dev.yml untouched" claim
  against post-homing history.

**Files:** none — read-only verification; findings recorded in
`openspec/changes/devceil/design.md` (section 3).

**Steps:**

1. Read the homing tasks.md Task 1 acceptance (archive, L74-79,
   checkbox `[x] 1.1`).
2. Run `git log --follow` on `.github/workflows/release-dev.yml`:
   exactly two commits — 02eec1be (2026-09-18 18:49:30 +0200, created
   the trio ceiling) and dea5c9c7 (2026-09-19 18:36:45 +0200, three
   flavors, Bd: rc-5t5fo).
3. Diff dea5c9c7 against the file: one added line,
   `dev-profile: true` (call input).

**Acceptance:**

- T1 claim confirmed valid within the homing change's own diff.
- Post-homing drift fully accounted for: one input line; permissions
  block byte-identical since 02eec1be.
- No unexplained drift remains.

## Task 4 — Test harness disposition

- [x] 4.1 Decide the verification-evidence path for workflow YAML and
  record it.

**Files:** none — read-only verification; disposition recorded in
`openspec/changes/devceil/design.md` (section 4).

**Steps:**

1. Search `scripts/xtask` and `crates/` for any YAML/workflow
   assertion harness: none exists (no xtask command and no crate
   parses or asserts workflow YAML; the only `release-matrix.yml`
   mention in code is a version-format comment at
   `changelog.rs:136`).
2. Confirm no local tooling fallback: python3 lacks the `yaml` module;
   no `yq` configured.
3. Per mission order, do not invent CI; adopt the manual matrix
   (design.md section 1.5) plus the empirical green run as evidence.

**Acceptance:**

- Disposition documented: manual verification matrix + v0.54.0 green
  dev run (2026-09-25) = accepted evidence.
- No CI added, no harness invented.

## Task 5 — Hardening ride-along disposition

- [x] 5.1 Disposition the docker job's unconditional
  `packages: write` + `id-token: write` request in dev runs.

**Files:** none — read-only verification; rationale recorded in
`openspec/changes/devceil/design.md` (section 5).

**Steps:**

1. Verify gating: in dev (`publish: false`, `dev-profile: true`)
   publish-side steps stay off by
   `if: ${{ inputs.publish && (...) }}` (first occurrence L495);
   non-publish steps are gated by
   `if: ${{ !inputs.dev-profile || matrix.in-dev }}` (first occurrence
   L492). Docker matrix: production (L465) and slim (L472) are
   `in-dev: false` (empty-green legs); full (L479) is `in-dev: true`
   and runs non-publish steps (checkout, artifact download) only.
   Elevated scopes (`packages: write`, `id-token: write`) stay
   unconsumed in dev.
2. Weigh narrowing options against the rc-42pkx load-time validation
   constraint.

**Acceptance:**

- Disposition KEEP recorded with three-point rationale (load-time
  validation class, elevated scopes unconsumed in dev, revisit
  trigger).
- No workflow edit; revisit conditions stated.
