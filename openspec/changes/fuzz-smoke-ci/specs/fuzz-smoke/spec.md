# fuzz-smoke Delta Spec

## ADDED Requirements

### Requirement: path-filtered PR smoke trigger

The `fuzz-smoke.yml` workflow SHALL run on `pull_request` events whose
changed files match at least one of: `crates/camel-dsl/**`, `fuzz/**`,
`scripts/xtask/**`, `.github/workflows/fuzz-smoke.yml`; and on manual
`workflow_dispatch`. It SHALL NOT run on pushes to `main`.

#### Scenario: workflow triggers on a PR touching the wrapper

- **WHEN** a PR modifies `scripts/xtask/src/fuzz.rs`
- **THEN** the `fuzz-smoke` workflow runs for that PR

#### Scenario: workflow skips unrelated PRs

- **WHEN** a PR modifies only `crates/camel-core/**`
- **THEN** the `fuzz-smoke` workflow does not run

#### Scenario: manual drill run

- **WHEN** a maintainer dispatches the workflow from the Actions tab
- **THEN** the drills execute on the selected ref

### Requirement: never blocks PR merge

The fuzz-smoke job SHALL declare job-level `continue-on-error: true`.
No step in the workflow SHALL be a required status check.

#### Scenario: crash does not red-X the merge

- **WHEN** the smoke run finds a crash artifact
- **THEN** the job is marked as failed-but-passing (annotation, not a blocking red X)

### Requirement: main-checkout refusal drill

The workflow SHALL run `cargo xtask fuzz dsl_yaml` directly in the
GitHub checkout and SHALL assert that it exits non-zero with the guard
message `refusing: cargo xtask fuzz must run in a linked worktree`.

#### Scenario: refusal observed end-to-end

- **WHEN** the wrapper runs in the plain checkout
- **THEN** the step asserts exit code 1 and the refusal substring in output

### Requirement: worktree-isolated 60-second smoke

The workflow SHALL create a linked worktree (`git worktree add`) and run
`cargo xtask fuzz dsl_yaml --time 60` from it on PRs (a
`workflow_dispatch` input MAY raise the budget — the deferred 300 s
full-run criterion closes on such a dispatch). On a clean run the step
SHOULD exit 0 with the corpus populated and no crash artifact.

#### Scenario: clean smoke run

- **WHEN** the target runs 60 s without finding a crash
- **THEN** the wrapper exits 0 and the worktree `target-fuzz/corpus/dsl_yaml/` contains the staged seeds

#### Scenario: build and run outputs stay worktree-local

- **WHEN** the smoke run completes
- **THEN** all fuzz build, corpus, and artifact outputs land under the worktree's `target-fuzz/` directory (worktree metadata, the temporary drill patch, and the worktree's own `./target` xtask build dir excluded — the wrapper redirects only cargo-fuzz target dirs)

### Requirement: cold-main assertion

The workflow SHALL assert that the checkout's `./target` directory is
absent before and after every drill and smoke step (the refusal drill's
xtask build SHALL redirect its target dir outside the checkout).

#### Scenario: main target stays cold

- **WHEN** the smoke run completes in the linked worktree
- **THEN** the checkout contains no `./target` directory at any point in the job

### Requirement: tmin drill with injected panic

The workflow SHALL, inside the linked worktree only, inject a temporary
panic into the `dsl_yaml` harness, run the wrapper with a short budget,
and SHALL assert: non-zero exit, `new artifact(s):` in output, a
`minimized artifact:` path in output whose file exists under
`target-fuzz/artifacts/dsl_yaml/`, that no `fuzz/artifacts/` directory
is created, and the promotion instruction line. The injection SHALL NOT
touch the checkout working tree.

#### Scenario: injected panic is caught and minimized

- **WHEN** the harness panics on every input and the wrapper runs with a 20 s budget
- **THEN** a crash artifact is detected, tmin produces a minimized artifact under `target-fuzz/artifacts/`, no `fuzz/artifacts/` directory exists, and the promotion instruction is printed

#### Scenario: no leakage to the checkout

- **WHEN** the drill completes
- **THEN** `git status --porcelain` in the checkout is empty (the panic existed only in the discarded worktree)

### Requirement: fuzz lockfile audit

The workflow SHALL run `cargo audit --file fuzz/Cargo.lock` under the
same non-blocking contract.

#### Scenario: advisory in the fuzz lockfile

- **WHEN** the fuzz lockfile picks up a vulnerable dependency
- **THEN** the audit step annotates the PR without blocking the merge

### Requirement: findings summary with triage instructions

A final workflow step SHALL write a step summary that, on any finding,
lists artifact paths and includes the triage command
(`bd create -t bug -p 1`) and the promotion rule (minimized input becomes
a committed `#[test]`, never a raw corpus blob). The workflow SHALL NOT
create issues or bd entries automatically.

#### Scenario: summary renders on drill failure

- **WHEN** any drill step fails
- **THEN** the PR shows a summary with artifact paths and triage instructions, and no issue automation ran
