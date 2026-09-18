# Tasks: release-restructure

## reusable workflow

### Task 1: Extract release-matrix.yml (reusable, publish-gated)

(setup: read `.github/workflows/release.yml` in full first; every
verbatim-move claim below refers to its current content. Docker-job
facts you will need: the docker matrix has NO `flavor` key — the
flavor mapping is `production`/`alpine` variants → musl targets →
`(regular)`, `gnu` variants → `(full)`.)

**Files:**
- `.github/workflows/release-matrix.yml` (new)

**Steps:**
1. Create `.github/workflows/release-matrix.yml` with
   `on: workflow_call` declaring one input:
   `publish: { description: "Execute the publish path (logins, registry pushes, GH release, crates.io)", type: boolean, required: false, default: false }`.
2. Move the build job VERBATIM (7-leg matrix with `flavor` key,
   `use-cross`, `kafka-probe`, `install-librdkafka`, `install-musl-tools`
   gates, Build binary step with
   `FEATURES="${FLAVOR}${ALLOC_FEATURES:+,$ALLOC_FEATURES}"`, Assert
   version flavor probe, rename/upload artifact steps). No `if:` on
   this job — both modes build.
3. Move the docker job with an explicit mode split, keeping these
   steps UNGATED (both modes): QEMU setup, docker buildx setup,
   `Prepare build context` (pulls `alpine:3.21`, dev-safe). Gate these
   steps with `if: inputs.publish` (publish-only): the credential/
   registry login steps, `Docker metadata`, `Extract version`,
   `buildx` push/`imagetools create`, and the `Attest*` steps
   (`Extract version` hardcodes `${GITHUB_REF#refs/tags/v}` which
   yields garbage on non-tag runs; publish metadata steps are
   dead weight on dev). Add exactly two steps with
   `if: inputs.publish != true` (dev-only):
   `docker buildx build --load --platform linux/amd64 --build-arg TARGETARCH=amd64 --target ${{ matrix.target }} -t camel:dev-${{ matrix.variant }} context`
   (context is the directory `Prepare build context` populated —
   repo root lacks the `camel-${TARGETARCH}` binary the Dockerfile
   COPYs; single-arch amd64 is the smoke scope), then
   `docker run --rm camel:dev-${{ matrix.variant }} --version`
   asserting the flavor suffix — `(regular)` for `production`/
   `alpine` variants, `(full)` for `gnu` variants (case pattern as
   in the build job's Assert version flavor step).
4. Move the release job (softprops/action-gh-release + the
   `cargo xtask changelog` step with `fetch-depth: 0` checkout) with
   job-level `if: inputs.publish`.
5. Move the publish job (trustpub credential-provider install +
   `scripts/publish-crates.sh`, `environment: crates-io`) with
   job-level `if: inputs.publish` — dev runs must not even install
   the credential provider. Update the in-file trusted-publisher
   comment (currently naming `release.yml`) to name
   `release-matrix.yml` (see Task 4 for the registry-side migration).
6. Add a `closure-check` job: `if: ${{ !inputs.publish }}`,
   `runs-on: ubuntu-latest`, steps: checkout, `dtolnay/rust-toolchain`
   pinned as the other cargo jobs pin it, then
   `cargo test -p camel-cli --test feature_profiles`.
7. `release.yml` is NOT touched in this task (transient duplication
   until Task 2 collapses it).

**Tests:** (from workspace root; prefer python3-yaml assertions over
fragile greps where noted)
- `matrix-yaml-parses`: `python3 -c "import yaml; yaml.safe_load(open('.github/workflows/release-matrix.yml'))"` exits 0.
- `workflow-call-declared`: python3-yaml: `on.workflow_call.inputs.publish.type == 'boolean'` and default False.
- `legs-moved-verbatim`: `grep -c 'flavor: "flavor-full"'` = 5, `grep -c 'flavor: "flavor-regular"'` = 2, `grep -c 'if: matrix.kafka-probe == true'` = 1, `grep -c 'Assert version flavor'` = 1.
- `publish-gating` (python3-yaml): jobs `release` and `publish` each have `if == 'inputs.publish'` at job level; within `docker`, steps named for logins/push/attest each carry `if == 'inputs.publish'`.
- `dev-only-forms`: `grep -c 'if: inputs.publish != true'` = 2 (docker --load build + docker smoke, the only dev-only steps); `grep -c 'if: ${{ !inputs.publish }}'` = 1 (closure job only).
- `closure-job-toolchain`: closure-check job contains a `dtolnay/rust-toolchain` step and the `feature_profiles` invocation.
- `build-ungated` (python3-yaml): the build job has no `if` key.

**Acceptance:**
- All checks hold as stated
- `bash -n` on every inline `run:` block (extract, syntax-check)
- No secret/environment reference introduced that release.yml did not already have

- [x] 1.1

## tag wrapper

### Task 2: Rewrite release.yml as thin tag wrapper

**Files:**
- `.github/workflows/release.yml` (modified — gutted to wrapper)

**Steps:**
1. Replace the entire file with the wrapper: `on: push: tags:`
   preserving the exact current tag pattern; `permissions:
   { contents: write, packages: write, id-token: write }` (footgun 1:
   permissions INTERSECT with the reusable workflow's); one job
   `call-release-matrix` with `uses: ./.github/workflows/
   release-matrix.yml`, `with: publish: true`,
   `secrets: inherit`.
2. NO `concurrency` block (footgun 3: cancelled `imagetools create`
   = half-published manifests).
3. Nothing else — no matrix, no steps, no environment.

**Tests:** (static)
- `wrapper-uses-matrix`: `grep -c 'uses: ./.github/workflows/release-matrix.yml'` = 1.
- `no-inline-matrix`: `grep -c 'strategy:\|^[[:space:]]*matrix:' release.yml` = 0 (anchored — the job id `call-release-matrix` legitimately contains the substring); `grep -c 'flavor:' release.yml` = 0.
- `permissions-trio`: each of `contents: write`, `packages: write`, `id-token: write` exactly once.
- `secrets-inherit`: `grep -c 'secrets: inherit'` = 1.
- `publish-true`: `grep -c 'publish: true'` = 1.
- `tag-trigger`: tags pattern preserved verbatim from the old file (record it in the report).
- `no-cancel`: `grep -c 'cancel-in-progress'` = 0.
- `yaml-parses`: python3-yaml parse exits 0.

**Acceptance:**
- All static checks hold
- File is under ~30 lines

- [x] 2.1

## dev wrapper

### Task 3: Add release-dev.yml (side-effect-free dev harness)

(setup: tasks 1-2 landed)

**Files:**
- `.github/workflows/release-dev.yml` (new)

**Steps:**
1. Create the dev wrapper: `on: { push: { branches: [main] },
   workflow_dispatch: {} }`.
2. `concurrency: { group: release-dev, cancel-in-progress: true }`
   (footgun 3: cancel lives ONLY here).
3. One job calling `uses: ./.github/workflows/release-matrix.yml`
   with `with: publish: false`.
4. NO `secrets:` block, no permissions block, no environment
   references.

**Tests:** (static, cross-file coherence included)
- `dev-uses-matrix-false`: `grep -c 'uses: ./.github/workflows/release-matrix.yml'` = 1 and `grep -c 'publish: false'` = 1.
- `no-secrets`: `grep -c 'secrets:' release-dev.yml` = 0.
- `cancel-only-here`: `grep -c 'cancel-in-progress' release-dev.yml` = 1 AND in release.yml = 0.
- `triggers`: `grep -c 'workflow_dispatch'` = 1, `grep -c 'branches:'` = 1.
- `three-files-parse`: python3-yaml parse all three files, exit 0 each.
- `side-effects-unreachable-static`: in release-dev.yml no `environment:`, no login action, no push verb.

**Acceptance:**
- All static checks hold
- Cross-file: `imagetools`, `gh-release`, `publish-crates` appear ONLY inside release-matrix.yml behind publish gating (grep all three files, record counts in the report)

- [x] 3.1

## trusted-publisher migration + docs

### Task 4: crates.io trusted-publisher migration and doc updates

(setup: task 1 landed; MUST land in the same merge as tasks 1-2 —
the next tag run after this change publishes from
`release-matrix.yml`)

**Files:**
- `docs/src/operations/oidc-publish-fallback.md` (modified)
- `.opencode/instructions/bump-version.md` (modified)
- `.opencode/prompts/bump-version.md` (modified)

**Steps:**
1. In `oidc-publish-fallback.md`, update the trusted-publisher
   REGISTRATION tuples (lines ~49, ~64, ~130: owner/repository/
   workflow/environment) and the `git show` verify command (~91)
   from `release.yml` to `release-matrix.yml`, keeping
   `environment crates-io` unchanged. Line ~84 ("`release.yml`
   triggers ONLY on `v*` tag pushes") stays TRUE of the tag wrapper —
   rephrase it to name the wrapper as trigger and release-matrix.yml
   as the publishing workflow instead of replacing the filename
   blindly. The intro reference (~line 5) may keep `release.yml`
   where it describes the tag trigger. Add one sentence stating WHY
   the tuples move: crates.io trusted publishing matches the
   workflow file containing the publish job (`job_workflow_ref`),
   which after the restructure is the reusable workflow. This
   sentence MUST name `release-matrix.yml` literally (it is the
   fifth `release-matrix.yml` mention that clears the `>= 5`
   threshold: four migrated tuple/verify references plus this
   rationale sentence).
2. In `.opencode/instructions/bump-version.md` (~line 39) and
   `.opencode/prompts/bump-version.md` (~line 4), update the prose
   that describes `release.yml` as holding the 7-target matrix to
   name `release-matrix.yml` (invoked by both wrappers).
3. Write `.opencode/fleet/PRE-CUTOVER-CHECKLIST-release-restructure.md`
   (new, untracked is fine if the fleet dir is gitignored — check
   with `git check-ignore`; if ignored, place it in the change dir
   as `evidence/pre-cutover-checklist.md` instead) with the single
   human action: BEFORE the next tag push, re-register every crate's
   trusted-publisher entry on crates.io (owner kennycallado,
   repository rust-camel, workflow `release-matrix.yml`, environment
   `crates-io`). The conductor surfaces this in the merge report.

**Tests:**
- `docs-registration-migrated`: `grep -c 'release-matrix.yml' docs/src/operations/oidc-publish-fallback.md` >= 5 AND the total `release\.yml` mentions in that file equal the expected residual (intro + line-84-wrapper + any trigger-prose you kept — record the exact residual count and each line's role in the task report; the registration tuples and verify command MUST be in the release-matrix count, not the residual).
- `bump-instructions-updated`: `grep -c 'release-matrix.yml' .opencode/instructions/bump-version.md` >= 1 and `grep -c 'release-matrix.yml' .opencode/prompts/bump-version.md` >= 1.
- `checklist-exists`: the checklist file exists at one of the two permitted locations and contains `release-matrix.yml`.

**Acceptance:**
- All checks hold
- Merge report (conductor) carries the pre-cutover human action verbatim

- [x] 4.1
