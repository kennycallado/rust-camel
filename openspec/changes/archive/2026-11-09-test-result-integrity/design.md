# Design: test-result-integrity

## Approach

Three workflow-file edits, one intent: *test-result integrity*. No Rust
code changes.

### 1. Complete failure reporting on red (`ci.yml`, rc-9wx6u)

`Test (full workspace)` → `run: cargo test --workspace --no-fail-fast`,
`Test camel-cli (exec flavor)` → same flag appended. Comment on each step
in the rc-97uah style (weekly job precedent at `ci-weekly.yml:35-38`)
stating: without the flag the first failing binary aborts the rest
(measured red runs reached 41/37/309 of 636 targets); with it, a red run
is *expected* to take about as long as a green one (~85–115 min — an
estimate, subject to successful compilation) and cargo prints
`error: N targets failed:` with every target; `timeout-minutes: 150` is
a hard cap, not a completion guarantee.

**CI-time budget.** Green runs: unchanged in target set (all targets
already execute); `--no-fail-fast` only changes failure behaviour. Red
runs: rise from "first failure + abort" (~15–40 min early exit) to
roughly a green run's duration (~85–115 min — an estimate from green
runs, not a guarantee: a red run adds the compile time of every target
that fail-fast previously skipped). It stays bounded by the existing
150-minute job timeout; if compilation itself fails, execution stops at
the compile error regardless of the flag — the requirement is that
fail-fast no longer *aborts running targets*, not that it guarantees
execution through compile breakage. No new job, no parallelism change,
no cache change; the exec-flavor leg's incremental cost is its own
compile+run, which fail-fast previously truncated.

### 2. No silent skip (`ci.yml`, rc-1x9fl part 1)

Add `if: ${{ !cancelled() }}` to the same two steps.

**Why `!cancelled()` rather than keying on the failing step's outcome.**
The job has ~25 earlier steps (checkout, disk prep, toolchain, caches,
libclang, docker, workspace build, hexagonal gate, 3 bridge builds, 10+
integration-test steps, 2 compile gates). Keying a condition on each
setup step's outcome needs ~25 step outputs plumbed into one expression,
renames break it silently, and it cannot change the job conclusion
anyway: a step failure already makes the job red. `!cancelled()` widens
*execution* on red runs only:

- After a failing *test* step (JMS, XML, CXF, container, Kafka, Redis,
  OpenSearch, K3s, auth, kernel): the workspace suite still runs — the
  red run reports the whole workspace, which is the point of fix 1.
- After a failing *setup* step (e.g. `Build`): the workspace step reruns
  and fails at the same compile error, bounded by the job timeout. The
  job was already red; the conclusion does not change; the extra signal
  is free-ish and never wrong.
- Cancellation still skips the steps (conclusion `cancelled`); the job
  conclusion keeps its invariant: **success ⇒ both test steps ran and
  passed**. A cancelled run means the evidence is unusable (the release
  preflight treats it as untested), not that zero tests executed. There
  is no configuration in which the job is green while the tests were
  skipped: the steps have no feature/path gating, and the preflight
  below additionally checks the step conclusions, so a future `if:`
  regression on those steps fails the release closed.

### 3. Fail-closed release preflight (`release.yml`, rc-1x9fl part 2)

**Mechanisms considered.**

1. **Actions API query in the tag wrapper (chosen).** New `ci-preflight`
   job, first in `release.yml`, `runs-on: ubuntu-latest`,
   `permissions: { contents: read, actions: read }` (job-level
   permissions replace the workflow-level set — same pattern the publish
   job already documents). One step: checkout, resolve the tagged commit
   `sha="$(git rev-parse 'HEAD^{commit}')"` (handles annotated tags),
   then `gh api` (gh + jq preinstalled on ubuntu-latest,
   `GH_TOKEN: ${{ github.token }}`):
   - List runs: `repos/{owner}/{repo}/actions/workflows/ci.yml/runs?head_sha=<sha>&per_page=100`
     — read `.workflow_runs[].id`, `.conclusion`, `.event`, and
     `.head_branch`; keep only completed successful runs that are
     **push-triggered on main** (`event == "push"`,
     `head_branch == "main"`, `conclusion == "success"`). PR-triggered
     runs never qualify: they test a synthetic merge commit, not the
     tagged tree, and `github.sha` for the tagged push is the main
     commit.
   - For each qualifying run, jobs:
     `repos/{owner}/{repo}/actions/runs/<id>/jobs?per_page=100` — read
     `jobs[].name`, `jobs[].conclusion`, `jobs[].steps[].name`,
     `jobs[].steps[].conclusion`; satisfy when a job named
     `Full Tests (ubuntu-latest)` has `conclusion == "success"` and its
     steps `Test (full workspace)` and `Test camel-cli (exec flavor)`
     both have `conclusion == "success"`.
   - No qualifying run ⇒ `::error::` naming the sha + exit 1. Any API
     error propagates (gh exits nonzero; `set -euo pipefail`; no
     `|| true`, no `continue-on-error`).
   Why it fails closed: "no run found" is an explicit failure, not a
   pass; cancelled and missing runs are both untested and both rejected;
   PR-only evidence is rejected even when its conclusion reads success;
   a skipped test step (`conclusion: "skipped"`) fails the step check; a
   failed or partial run never matches `conclusion == "success"`; and the
   API call failing (403, network, rate limit) fails the job instead of
   defaulting to allow.
   Ordering: `call-release-matrix` gains `needs: [ci-preflight]`, so an
   untested SHA fails *before* the 12-leg matrix burns ~1h and before the
   `release` job creates a GitHub Release or docker images.
   `publish` keeps `needs: call-release-matrix` (now transitively gated).
   Token scope: the built-in `GITHUB_TOKEN` with `actions: read` — same
   repository, no secret setup; stated in the workflow comment and the
   checklist.
2. **`workflow_run` trigger (rejected).** `workflow_run` fires only for
   workflow files and runs on the default branch; `release.yml` would
   stop being tag-triggered, the tag↔sha mapping becomes indirect, and
   multiple CI runs per sha introduce races. Fails the "simplest that
   cannot silently no-op" bar.
3. **Release invokes CI as a reusable workflow (rejected).** `ci.yml` is
   push/PR-shaped (9 jobs), not `workflow_call`-shaped; converting it is
   invasive and re-runs the entire ~2h suite on every tag. Also closer to
   the hard limit on touching the matrix.

**Post-merge human verification** (CI cannot run locally — no proof is
claimed here). After merge, cut a throwaway tag on an untested SHA and
watch the release run's preflight job:

```bash
gh run list --workflow release.yml --limit 1   # note the release run id
gh run view <release-run-id> --json status,conclusion,jobs \
  --jq '.jobs[] | {name, conclusion}'
```

Expected conclusions per scenario (all are `ci-preflight` verdicts read
from the release run):

| Scenario (tagged commit's CI evidence) | Preflight job | Matrix/publish |
|---|---|---|
| Successful push-on-main run, Full Tests + both test steps `success` | `success` | run |
| No CI run for the sha | `failure`, error names the sha | skipped |
| Latest runs failed or cancelled (untested) | `failure` | skipped |
| Successful run but workspace test step `skipped` | `failure` | skipped |
| Only PR-triggered runs match the sha | `failure` | skipped |

Read-only spot-check of the fields the preflight reads, for any commit:

```bash
gh api "repos/<owner>/<repo>/actions/workflows/ci.yml/runs?head_sha=<sha>&per_page=100" \
  --jq '.workflow_runs[] | {id, conclusion, event, head_branch}'
gh api "repos/<owner>/<repo>/actions/runs/<run-id>/jobs?per_page=100" \
  --jq '.jobs[] | {name, conclusion, steps: [.steps[] | {name, conclusion}]}'
```

**Post-merge verification of the no-silent-skip guard** (rc-1x9fl part
1; cannot be exercised locally): on the next red `main` push whose
`full-tests-linux` job ran to completion (not cancelled, not killed by
the 150-minute timeout — a cancelled or timed-out job legitimately
skips its remaining steps), read the
Full Tests job's step conclusions —
`gh run view <ci-run-id> --json jobs --jq '.jobs[] | select(.name == "Full Tests (ubuntu-latest)") | [.steps[] | select(.name == "Test (full workspace)" or .name == "Test camel-cli (exec flavor)") | .conclusion]'`
must be `["success","success"]` or a failure-shaped pair (`"failure"`
per step) — **never** `"skipped"`.

### 4. Checklist (`bump-version.md`)

New step between the release commit and the local tag: a skipped or
cancelled Full Tests run is *not tested*; check with
`gh run list --workflow ci.yml --commit <SHA>` (need a completed
**successful push run on main** — PR-triggered rows do not qualify,
they test a synthetic merge commit) and
`gh run view <run-id> --json jobs --jq ...` for the Full Tests job and
its two `Test` steps concluding `success`. Note the preflight enforces
this server-side with the built-in token (`actions: read`); Rule 0b in
the `Cargo.toml` comment stays as is.

## Affected crates

None — workflow YAML + one agent prompt file only:
- `.github/workflows/ci.yml`: two test steps (flags, `if:`, comments).
- `.github/workflows/release.yml`: `ci-preflight` job; `needs` on
  `call-release-matrix`.
- `.opencode/prompts/bump-version.md`: one checklist step + token note.

## Architecture boundaries

No Runtime / DSL / Components / Services / Languages / Functions surface
is touched. The change only tightens existing CI/release gates (a red or
untested path must fail, never pass) and does not alter build artefacts,
flavors, or publish behaviour. Spec deltas: new capability
`test-result-integrity` (execution completeness + checklist) and a
MODIFIED + ADDED requirement in `release-pipeline` (the canon pins the
tag-wrapper job structure, so the preflight job must be recorded there).

## Phases

Omitted — single-phase change (one coherent slice; three small files,
one intent, no independent subsystems).
