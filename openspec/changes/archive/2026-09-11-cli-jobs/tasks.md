# Tasks: cli-jobs

## Implementation

### Task 1: `execute:` document model

**Files:**
- `crates/camel-cli/src/commands/job/document.rs` (new)
- `crates/camel-cli/src/commands/job/document_tests.rs` (new)

**Steps:**
1. Job-local `JobDocument` / `ExecuteSection` / `JobSendAction` types
   with `deny_unknown_fields`; `routeFiles` / `routeFilesFromRoot` /
   `routes` keys with the family exactly-one conflict rule (reused
   `TestDocError::RouteSourceConflict`).
2. Validation: reserved suffix, `execute:` presence, exclusivity vs
   `scenario:` and the unit-tier vocabulary, `mode: one-shot` only
   (`batch` reserved with a distinct error), mandatory positive
   humantime `timeout`, `direct:`/`seda:` send target, body scalar
   rejection.
3. Route-source resolution: `routeFiles` against the doc dir,
   `routeFilesFromRoot` against the strict nearest-ancestor
   `Camel.toml` walk (reused `find_camel_toml_root`), inline wrap for
   `parse_routes_with_env`.

**Tests:** 14 unit tests in `document_tests.rs` covering the parse
matrix, exclusivity, conflict rule, consumer allowlist, and URI base
matching.

- [x] 1

### Task 2: `camel job` runner

**Files:**
- `crates/camel-cli/src/commands/job/mod.rs` (new)
- `crates/camel-cli/src/commands/mod.rs` (modified)
- `crates/camel-cli/src/main.rs` (modified)
- `crates/camel-cli/src/commands/run.rs` (modified:
  `load_config_or_default` / `canonical_project_root` to `pub(crate)`)

**Steps:**
1. `JobArgs` (document, `--report`, `--config`) and the `Commands::Job`
   arm; `run_job` returns the exit code, `main.rs` applies the process
   exit (single site).
2. Boot composition mirroring `camel run` steps 1-5 (config, context,
   security, bind acks, bundles boot) + discovery/inline route loading;
   conditional `exec:` bundle registration; SQL startup checks.
3. Consumer gate (fail-closed allowlist), auto_startup flip (all false,
   target true), registration, `ctx.start()`.
4. Send with the bounded startup-race retry window under the mandatory
   overall `timeout_at`; `PipelineOutcome` mapping through the producer
   reply seam; teardown via `boot_handle.shutdown_with_deadline` with a
   5 s floor; JSON report to stdout or `--report`.

**Tests:** integration tests (Task 4); exit taxonomy assertions inline.

- [x] 2

### Task 3: `camel test` dispatch guard

**Files:**
- `crates/camel-cli/src/commands/test/document_parse.rs` (modified)

**Steps:**
1. `declares_execute` sniff ahead of the scenario sniff in
   `parse_document`; an `execute:` document fails with a stderr error
   naming `camel job` (exit-2 parse-error class).

**Tests:** covered by the family exclusion scenarios in the spec;
unit-tier parse behavior unchanged.

- [x] 3

### Task 4: integration tests

**Files:**
- `crates/camel-cli/tests/job_one_shot_test.rs` (new)
- `crates/camel-cli/tests/common/mod.rs` (modified: `spawn_camel_job`)

**Steps:**
1. Happy path: `direct:` transform route, `capture-reply`, exit 0,
   `Completed` report with the reply body.
2. `log:` sink route: exit 0, no reply field.
3. Failure path: `to: direct:missing-consumer` inside the route exits 1
   with a `Failed` report.
4. Load-time rejection: `mode: batch` exits 2 with the reserved-mode
   stderr message.
5. Seda verdict fidelity: a failing `seda:` route exits 1 with a
   `Failed` report (the forced `waitForTaskToComplete=Always` makes the
   send synchronous).
6. Overall timeout: a 30 s `delay` route with `timeout: 1s` exits 2
   with a `Timeout` report, bounded under 15 s wall clock.

**Acceptance:** all six green via
`cargo test -p camel-cli --test job_one_shot_test`; the camel-cli
integration-test binary count moves 24 → 25.

- [x] 4

### Task 5: docs

**Files:**
- `crates/camel-cli/CONTEXT.md` (modified)
- `openspec/changes/cli-jobs/**` (this change)

**Steps:**
1. CONTEXT.md: `camel job` failure-mode table (exit taxonomy) and the
   new ADR-0012 `error!` site rows.
2. openspec change: proposal, design, tasks, and the `cli-jobs` spec
   delta; `openspec validate cli-jobs --type change --json` passes.

- [x] 5
