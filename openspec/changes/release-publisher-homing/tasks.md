# Tasks: release-publisher-homing

## workflow + docs homing

### Task 1: Home the publish job in release.yml; revert doc tuples

(setup: read `.github/workflows/release-matrix.yml` and
`.github/workflows/release.yml` in the worktree first. The publish
job in the matrix starts at the `publish:` job key — 6 steps:
checkout, toolchain, trustpub credential-provider install,
`scripts/publish-crates.sh`, plus its comment block at lines
~242-253.)

**Files:**
- `.github/workflows/release.yml` (modified)
- `.github/workflows/release-matrix.yml` (modified)
- `docs/src/operations/oidc-publish-fallback.md` (modified)
- `.opencode/instructions/bump-version.md` (modified)
- `.opencode/prompts/bump-version.md` (modified)
- `openspec/changes/archive/2026-09-18-release-restructure/evidence/pre-cutover-checklist.md` (modified — banner)

**Steps:**
1. In `release-matrix.yml`: DELETE the entire `publish` job
   (including its comment block). Update the `publish` input's
   `description` to drop the crates.io mention — new text:
   `"Execute the publish path (logins, registry pushes, GH release)"`
   (keeps the docker/gh-release semantics it still gates).
2. In `release.yml`: ADD a `publish` job after `call-release-matrix`
   with: `needs: call-release-matrix`, `runs-on: ubuntu-latest`,
   job-level `permissions: { contents: read, id-token: write }`,
   `environment: crates-io`, and the 6 steps moved VERBATIM from the
   matrix. NO `if:` on the job (the `v*`-tag-only trigger is the
   structural gate). The moved comment block is EDITED, not carried
   verbatim: `workflow release-matrix.yml` becomes
   `workflow release.yml` (the registration filename), keeping the
   rest of the comment (owner/repository/environment tuple, the
   registration-COMPLETE warning) intact.
3. In `docs/src/operations/oidc-publish-fallback.md`: revert the 3
   registration tuples (~lines 48-49, 63-65, 130-131) and the
   `git show` verify command (~line 94) from `release-matrix.yml`
   back to `release.yml`. Rephrase intro (~lines 4-7) and the
   wrapper note (~line 87) to: tag wrapper `release.yml` homes the
   publish job; reusable `release-matrix.yml` builds/artifacts/
   images/gh-release. KEEP a WHY-constraint sentence stating: the
   crates.io publish job MUST remain homed in `release.yml` because
   trusted publishing matches `job_workflow_ref` (the workflow file
   containing the publish job) against each crate's registration,
   and all 60+ crates are registered against `release.yml`.
4. In `.opencode/instructions/bump-version.md` (~step 7) and
   `.opencode/prompts/bump-version.md`: revert the
   matrix-holds-publish claims — `release-matrix.yml` still holds
   the 7-target matrix (build/docker/gh-release), the publish job
   lives in the `release.yml` tag wrapper.
5. Prepend an OBSOLETE banner to
   `openspec/changes/archive/2026-09-18-release-restructure/evidence/pre-cutover-checklist.md`:
   `> OBSOLETE (2026-09-18): superseded by change release-publisher-homing —
   > the publish job returned to release.yml; NO crates.io re-registration
   > is needed or wanted. Do not follow the steps below.`

**Tests:** (static; run all, record counts. pyyaml notes: `on:` may
parse as key `True` in python3-yaml (use `d.get('on') or d[True]`);
accept `needs` as either list `['call-release-matrix']` or scalar.)
- `job-homed` (python3-yaml on `release.yml`): jobs contain `call-release-matrix` AND `publish`; `publish.needs` in `(['call-release-matrix'], 'call-release-matrix')` forms; `publish.environment == 'crates-io'`; `publish.permissions == {'contents': 'read', 'id-token': 'write'}`; `publish` job has NO `if` key. Thinness invariants (same parse): trigger `on.push.tags == ['v*']`; top-level permissions contain exactly `contents: write`, `packages: write`, `id-token: write`; call job `with.publish is True` and `secrets: inherit`; no `strategy` key in any job.
- `matrix-publish-job-gone`: python3-yaml on `release-matrix.yml`: no `publish` job; `grep -c 'publish-crates' release-matrix.yml` = 0.
- `steps-verbatim`: `grep -c 'publish-crates.sh' release.yml` >= 1 and the trustpub credential-provider install step name matches the one deleted from the matrix (record both step names in the report).
- `comment-edited`: `grep -c 'workflow release-matrix' release.yml` = 0 AND `grep -c 'workflow release.yml' release.yml` >= 1 (scoped to the comment tuple text — the `uses:` and `needs:` lines legitimately contain `release-matrix`).
- `input-description-shrunk` (python3-yaml on `release-matrix.yml`): `on.workflow_call.inputs.publish.description` == `"Execute the publish path (logins, registry pushes, GH release)"`.
- `docs-tuples-reverted`: `grep -c 'workflow \`release.yml\`' docs/src/operations/oidc-publish-fallback.md` >= 2 (tuples are backticked; one tuple line-wraps so `workflow` and the filename land on different lines — record the matching line numbers; the exact-2 count below catches any missed tuple revert) AND `grep -c 'release-matrix.yml' docs/src/operations/oidc-publish-fallback.md` = 2 (exactly the intro + wrapper-note prose rephrase; tuples and verify command must NOT count here) AND `grep -c 'job_workflow_ref' docs/src/operations/oidc-publish-fallback.md` >= 1.
- `bump-docs-reverted`: in `.opencode/instructions/bump-version.md` and `.opencode/prompts/bump-version.md`, NO line contains both `release-matrix` and (`crates.io` or `publish-crates`) — crates.io publishing is attributed to release.yml or unattributed, never to the matrix.
- `checklist-banner`: `head -3 openspec/changes/archive/2026-09-18-release-restructure/evidence/pre-cutover-checklist.md` contains `OBSOLETE`.
- `yaml-parses`: python3-yaml parse both workflow files, exit 0.
- mdbook: `mdbook build docs` exits 0 (same command used in change release-restructure).

**Acceptance:**
- All checks hold with recorded counts
- `release-dev.yml` untouched (`git diff --name-only` shows no release-dev.yml)
- No Rust files modified by this task

- [x] 1.1

## decision record

### Task 2: ADR-0082 + CONTEXT-MAP line

(setup: task 1 landed)

**Files:**
- `docs/adr/0082-release-publisher-homing.md` (new)
- `CONTEXT-MAP.md` (modified)

**Steps:**
1. Write `docs/adr/0082-release-publisher-homing.md` following the
   repo's ADR format (status Accepted, date 2026-09-18; see
   `docs/adr/0081-*.md` for structure). Content:
   - Context: release-restructure moved the publish job into
     release-matrix.yml; crates.io trusted publishing matches
     `job_workflow_ref` (the workflow FILE containing the publish
     job) against per-crate registrations; 60+ crates registered
     against `release.yml`; no crates.io API exists for
     trusted-publisher config; zero crates had been re-registered
     (owner verified 2026-09-18).
   - Decision: the crates.io publish job is permanently homed in
     `release.yml` (the tag wrapper). `job_workflow_ref` therefore
     stays `.github/workflows/release.yml`. The matrix `publish`
     input gates image/registry effects only.
   - Accepted cost: wrapper grows to ~55 lines; publish waits ALL
     matrix jobs (`needs: call-release-matrix`) — a docker failure
     blocks crates publishing (safer: least-reversible side effect
     fails closed); no partial-dependency mechanism exists in
     GitHub Actions.
   - Consequences: any future refactor that moves the publish job
     to another file re-opens the 60+ re-registration problem —
     this ADR forbids that without an explicit superseding ADR.
2. In `CONTEXT-MAP.md`: add one line to the architecture decisions
   section referencing ADR-0082 (match the section's existing line
   format exactly).

**Tests:**
- `adr-exists-and-complete`: file exists; `grep -c 'job_workflow_ref' docs/adr/0082-release-publisher-homing.md` >= 1; contains Status, Context, Decision, Consequences headings.
- `contextmap-line`: `grep -c '0082' CONTEXT-MAP.md` >= 1.
- `lint-context-citations`: `cargo xtask lint-context-citations` exits 0.

**Acceptance:**
- All checks hold
- ADR in English; mdbook/docs gates still pass

- [x] 2.1
