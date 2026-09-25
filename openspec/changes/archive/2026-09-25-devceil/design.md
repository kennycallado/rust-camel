# Design: devceil — verification record

All line refs and shas verified in the devceil worktree at main
dbff0a90 on 2026-09-25. This record states what the evidence shows; no
history is fabricated.

## 1. Permissions union (scope 1)

### 1.1 Requested permissions in the reusable workflow

`.github/workflows/release-matrix.yml` (reusable, `workflow_call` at
L4, top-level `permissions: contents: read` at L17-18) declares four
jobs:

| Job | Line | Permission request | Ref |
| --- | --- | --- | --- |
| build | L24 | inherits top-level `contents: read` | L17-18 |
| release | L341 | `contents: write` | L346-347 |
| docker | L448 | `contents: read`, `packages: write`, `id-token: write` | L452-455 |
| closure-check | L733 | inherits top-level `contents: read` | L17-18 |

### 1.2 Callers

- `release.yml` (tag wrapper): top-level trio `contents: write`,
  `packages: write`, `id-token: write` at L8-11. Jobs:
  `call-release-matrix` (L14-15, passes `publish: true` at L17) and
  `publish` (L20) with job-level `contents: read` + `id-token: write`
  at L30-32, `environment: crates-io` at L39 — crates.io trusted
  publishing.
- `release-dev.yml` (dev wrapper): top-level trio `contents: write`,
  `packages: write`, `id-token: write` at L17-20. The comment block at
  L13-16 states the ceiling rationale. Single job `call-release-matrix`
  (L23-27) calls `release-matrix.yml` with `publish: false`,
  `dev-profile: true`.

### 1.3 Union computation

The dev wrapper must cover the union of nested job permission requests
at load time. GitHub validates these requests at workflow load, even
for jobs whose `if:` condition never fires in dev mode (rc-42pkx
lesson):

```
build         contents:read
  ∪ release   contents:write
  ∪ docker    contents:read + packages:write + id-token:write
  ∪ closure-check contents:read
= { contents: write, packages: write, id-token: write }
```

### 1.4 Verdict

`release-dev.yml` L17-20 carries exactly this trio. The ceiling HOLDS.
No workflow change required.

### 1.5 Manual verification matrix

| File | Scope | Permission | Line ref |
| --- | --- | --- | --- |
| release-matrix.yml | workflow (top) | contents: read | L17-18 |
| release-matrix.yml | job build | contents: read (inherited) | L24, L17-18 |
| release-matrix.yml | job release | contents: write | L341, L346-347 |
| release-matrix.yml | job docker | contents: read, packages: write, id-token: write | L448, L452-455 |
| release-matrix.yml | job closure-check | contents: read (inherited) | L733, L17-18 |
| release.yml | workflow (top) | contents: write, packages: write, id-token: write | L8-11 |
| release.yml | job call-release-matrix | publish: true (uses matrix) | L14-17 |
| release.yml | job publish | contents: read, id-token: write; environment crates-io | L20, L30-32, L39 |
| release-dev.yml | workflow (top) | contents: write, packages: write, id-token: write | L17-20 |
| release-dev.yml | job call-release-matrix | publish: false, dev-profile: true | L23-27 |

Required ceiling: `{contents: write, packages: write, id-token: write}`.
Present in release-dev.yml: yes, exactly (L17-20). Surplus: none.

## 2. Homing artifacts located (scope 2)

The artifacts are NOT lost. They live at
`openspec/changes/archive/2026-09-18-release-publisher-homing/`
(proposal.md, design.md, tasks.md,
specs/release-pipeline/spec.md), landed by commit 83870932
"chore(openspec): archive release-publisher-homing",
2026-09-18 19:16:34 +0200.

Timeline of 2026-09-18 (all +0200):

| Commit | Time | Subject |
| --- | --- | --- |
| 02eec1be | 18:49:30 | fix(release): grant dev wrapper permission request ceiling |
| b56f9681 | 19:15:32 | fix(release): home crates publish job in tag wrapper (the homing implementation) |
| 9551088c | 19:16:12 | fix(spec): keep canonical scenario title for archive |
| 83870932 | 19:16:34 | chore(openspec): archive release-publisher-homing |

Root cause of the bd "not found" symptom: the dev-perms-hotfix worktree
was cut from a base predating 83870932, so its checkout of
`openspec/changes/` lacked the archive entry. On main, the artifacts
have been present since 2026-09-18 19:16. No re-baseline fabrication is
needed — history is intact.

Related archive dirs: `2026-09-18-release-restructure` (parent change,
carries `evidence/pre-cutover-checklist.md`) and
`2026-09-19-three-flavor-matrix` (commit dea5c9c7, Bd: rc-5t5fo).

## 3. T1 re-baseline (scope 3)

The homing tasks.md Task 1 acceptance (archive dir, L74-79) reads:

> - `release-dev.yml` untouched (`git diff --name-only` shows no
>   release-dev.yml)

with checkbox `[x] 1.1`. This is a task-scoped claim: it asserted that
the homing change's own diff touched no workflow file. Valid at homing
time.

Full git history of `.github/workflows/release-dev.yml` (`git log
--follow`) holds exactly two commits:

| Commit | Date | Change to release-dev.yml |
| --- | --- | --- |
| 02eec1be | 2026-09-18 18:49:30 +0200 | created the file with the trio ceiling (rc-42pkx hotfix) |
| dea5c9c7 | 2026-09-19 18:36:45 +0200 | feat(dist): three-flavor release pipeline (slim/regular/full) (body trailer "Bd: rc-5t5fo.5") — ONE added line: `dev-profile: true` (call input) |

Delta verdict: T1 "untouched" held within the homing change's own diff.
Post-homing drift is dea5c9c7's single `dev-profile: true` input line —
a workflow-call input, not a permissions change. The permissions block
(L17-20) is byte-identical since 02eec1be. No unexplained drift.

## 4. Test harness disposition (scope 4)

No workflow-YAML test harness exists in the repo. No xtask command and
no crate parses or asserts workflow YAML. The only mention of
`release-matrix.yml` in code is a version-format comment at
`scripts/xtask/src/changelog.rs:136`. The `dsl_yaml` target in
`fuzz.rs` fuzzes the DSL, not workflow YAML. Local python3 lacks the
`yaml` module; no `yq` harness is configured.

Per mission order, no CI is invented. Disposition: a documented manual
verification matrix — section 1.5 above IS that matrix (line refs plus
the union computation). Empirical cross-check: the Release (dev) run on
the v0.54.0 main push (2026-09-25) went green, which means GitHub's own
load-time validation passed on the exact current files. Manual matrix
plus the empirical run is the accepted evidence.

## 5. Hardening ride-along disposition

Observation: the docker job in `release-matrix.yml` requests
`packages: write` + `id-token: write` at job level (L452-455)
unconditionally. In dev runs (`publish: false`, `dev-profile: true`)
the gating is mixed:

- publish-side steps (docker/login L569/577, build-push L653,
  attest-build-provenance L671/680/694) are gated by
  `if: ${{ inputs.publish && (...) }}` (first occurrence L495) and
  stay off in dev because `publish` is false;
- non-publish steps are gated by
  `if: ${{ !inputs.dev-profile || matrix.in-dev }}` (first occurrence
  L492). The docker matrix sets `in-dev: false` for production (L465)
  and slim (L472), but `in-dev: true` for full (L479). In dev this
  gate is TRUE for the full leg: that leg runs its non-publish steps
  (checkout, artifact download).

So in dev, 2 of the 3 docker legs (production, slim) run empty-green;
the full leg runs non-publish steps only. Publish-side steps stay off
via `inputs.publish`, so the elevated scopes (`packages: write`,
`id-token: write`) remain unconsumed in dev, and no secrets are
inherited.

Disposition: KEEP (status quo). Rationale:

1. GitHub validates nested permission requests at workflow load
   regardless of step gating (rc-42pkx class failure). Narrowing the
   docker job permissions requires splitting dev and prod docker jobs
   into separate job definitions — added complexity for zero runtime
   exposure reduction.
2. The elevated scopes are structurally unconsumed in dev: no step
   uses `packages: write` or `id-token: write` access.
3. Revisit only if the security posture tightens; then prefer
   `persist-credentials: false` or a dev docker-job split.

## 6. Spec delta

None. The canonical release-pipeline spec (archived with the homing
change) does not encode the ceiling: "Scenario: dev wrapper calls the
same matrix" (spec.md L47-54) requires only `publish: false`,
`dev-profile: true`, no secrets, and a concurrency group — it never
mentions the permission trio. The trio mandate lives in the workflow
comment (`release-dev.yml` L13-16, added by 02eec1be) and bd
rc-42pkx. The spec delta was skipped per mission order
(`skip_specs: true`): the mission expected none, and none is needed.
Current workflows conform; no spec change.

## 7. Empirical evidence

- Release (dev) ran green on the v0.54.0 main push (2026-09-25):
  GitHub load-time validation passed on the current main content of
  all three workflow files.
- Base commit for this record: dbff0a90.
