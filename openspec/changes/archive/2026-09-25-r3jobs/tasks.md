# Tasks: r3jobs

## Phase 1: Compile-side parity fixes

### Task 1.1: Skip configuration route patterns for job-kind plans

**Files:**
- `crates/camel-cli/src/compile/sources.rs` (modified)
- `crates/camel-cli/tests/compile_command_test.rs` (modified)

**Steps:**
1. In `resolve()` in `crates/camel-cli/src/compile/sources.rs`, locate the block commented `// Config route patterns, declared order, matches sorted.` (inside the `--config` branch, pushing `StoreEntryKind::Route` entries from `route_patterns`). Guard that block so it runs only when the entry-document `kind` is `TrailerKind::Route`; for `TrailerKind::Job` the configuration `routes` patterns add no plan entries (the `camel job` exactly-one-source rule: a job's route set comes only from its own `routeFiles`/`routeFilesFromRoot` declarations). Keep configuration/include/profile embedding for jobs unchanged.
2. Update the block's comment to state the job-kind carve-out and the parity rationale (a document set `camel job` accepts must compile).
3. Add the test below to `crates/camel-cli/tests/compile_command_test.rs`, modeled on the existing multi-document job test that asserts `store.index` contents (reuse that file's compile + store-decode helpers).

**Tests:**
- name: `compile_job_ignores_config_route_patterns`
  - setup: temp dir with `Camel.toml` containing `include = ["conf/base.toml"]` and `routes = ["routes/*.yaml"]`, an `include` fragment, a job document declaring `routeFiles: ["routes/b.yaml"]` (one-shot `execute:` block with a `direct:` send), `routes/b.yaml` and `routes/a.yaml` both holding valid `direct:` route documents.
  - action: run `camel compile ingest.job.yaml -o out.bin --config Camel.toml` (clear `CAMEL_*` env the way existing compile tests do); decode the artifact's store index; also run `camel job ingest.job.yaml --config Camel.toml --report cli-report.json` through the test file's subprocess-spawn mechanism (`CARGO_BIN_EXE_camel`), pinning the GIVEN that `camel job` accepts this document set.
  - assert: compile exits 0; `store.index.source_plan.references == ["ingest.job.yaml", "routes/b.yaml"]`; `routes/a.yaml` appears in NO store entry; the config and include entries still embed; the `camel job` run exits 0 with outcome `Completed`.
  - command: `TMPDIR=/home/shared/tmp cargo test -p camel-cli --test compile_command_test compile_job_ignores_config_route_patterns`
  - expected: fails before step 1 (compile exits 2 with `duplicate source`), passes after.

**Acceptance:**
- `TMPDIR=/home/shared/tmp cargo test -p camel-cli --test compile_command_test compile_job_ignores_config_route_patterns` exits 0.
- Route-kind behavior unchanged: full `compile_command_test` battery passes (`TMPDIR=/home/shared/tmp cargo test -p camel-cli --test compile_command_test`).
- `cargo fmt --check --all` and `cargo clippy -p camel-cli -- -D warnings` exit 0.

- [x] 1.1

### Task 1.2: Reject zero-route-entry job file sources at compile

**Files:**
- `crates/camel-cli/src/compile/sources.rs` (modified)
- `crates/camel-cli/tests/compile_command_test.rs` (modified)

**Steps:**
1. Add a `SourceError` variant `JobRouteSourceEmpty { document: String, patterns: Vec<String> }` to `crates/camel-cli/src/compile/sources.rs` with a `Display` implementation using the CLI's exact rule string `job route source resolved zero route definitions` (the `camel job` job-safety class wording) plus the document logical path and the declared patterns.
2. In `resolve()`, track whether the entry document's `route_source_fields` matched a file form (`RouteFiles` or `RouteFilesFromRoot`) and remember the declared patterns. After plan construction, when `kind` is `TrailerKind::Job`, a file form was declared, and the plan contains zero `StoreEntryKind::Route` entries, return `SourceError::JobRouteSourceEmpty`. This is a count-of-entries check only; do NOT parse route document contents at compile.
3. Add the two tests below to `crates/camel-cli/tests/compile_command_test.rs`.

**Tests:**
- name: `compile_job_rejects_zero_route_files`
  - setup: temp dir with a `Camel.toml` (include + `[default]`), a job document declaring `routeFiles: ["routes/none/*.yaml"]`, and an unrelated `routes/b.yaml` the pattern does not match.
  - action: run `camel compile ingest.job.yaml -o out.bin --config Camel.toml`.
  - assert: exit 2; stderr contains `job route source resolved zero route definitions` and the document name; no `out.bin` exists.
  - command: `TMPDIR=/home/shared/tmp cargo test -p camel-cli --test compile_command_test compile_job_rejects_zero_route_files`
  - expected: fails before steps 1–2 (compile exits 0), passes after.
- name: `compile_job_rejects_zero_route_files_from_root`
  - setup: same shape but the job document declares `routeFilesFromRoot: ["routes/missing/*.yaml"]` with `--config` supplied.
  - action: run the same compile.
  - assert: exit 2 with the same rule wording; no artifact.
  - command: `TMPDIR=/home/shared/tmp cargo test -p camel-cli --test compile_command_test compile_job_rejects_zero_route_files_from_root`
  - expected: fails before, passes after.

**Acceptance:**
- Both new tests exit 0; full `compile_command_test` battery passes.
- A job document with inline `routes:` only still compiles (existing tests cover this — they must stay green).
- `cargo fmt --check --all` and `cargo clippy -p camel-cli -- -D warnings` exit 0.

- [x] 1.2

### Task 1.3: Pin multi-entry job plan order and byte determinism

**Files:**
- `crates/camel-cli/tests/compile_command_test.rs` (modified)

**Steps:**
1. Add fixtures (module constants) for a multi-entry job: job document declaring `routeFiles: ["routes/b.yaml", "routes/c*.yaml", "routes/a.yaml"]` where the glob matches `routes/c1.yaml` and `routes/c2.yaml` (four route files total, each a valid `direct:` route document), plus a `Camel.toml` containing `include = ["conf/base.toml"]` and a `[default] log_level = "info"` section, and a `conf/base.toml` fragment containing `[default]\ndrain_timeout_ms = 5000`.
2. Add the two tests below, reusing the file's compile and store-decode helpers.

**Tests:**
- name: `compile_job_multi_entry_plan_order`
  - setup: the fixtures above on disk.
  - action: compile the job document with `--config Camel.toml`; decode the store index.
  - assert: exit 0; `source_plan.references == ["ingest.job.yaml", "routes/b.yaml", "routes/c1.yaml", "routes/c2.yaml", "routes/a.yaml"]` (declared pattern order, each pattern's matches sorted); store entries include all four route entries with kind `route`.
  - command: `TMPDIR=/home/shared/tmp cargo test -p camel-cli --test compile_command_test compile_job_multi_entry_plan_order`
  - expected: passes on current main (behavior-pinning test; if it fails, report the observed plan — do not change ordering code without escalating).
- name: `compile_job_multi_entry_deterministic_bytes`
  - setup: the same fixtures.
  - action: compile twice to two artifact paths; read both files' bytes.
  - assert: exit 0 both times and the two artifacts are byte-identical.
  - command: `TMPDIR=/home/shared/tmp cargo test -p camel-cli --test compile_command_test compile_job_multi_entry_deterministic_bytes`
  - expected: passes on current main (pinning).

**Acceptance:**
- Both tests exit 0; full `compile_command_test` battery passes.

- [x] 1.3

## Phase 2: Boot battery

### Task 2.1: Multi-entry chain boot fixture and outcome test

**Files:**
- `crates/camel-cli/tests/compiled_artifact_test.rs` (modified)

**Steps:**
1. Add fixture constants for a multi-entry chain job (job-legal consumer schemes only — `direct:`): `MULTI_N_JOB_DOC` declaring `routeFiles: ["routes/b.yaml", "routes/c1.yaml", "routes/c2.yaml", "routes/a.yaml"]` with a one-shot `execute:` block sending `ping` to `direct:transform` with `capture-reply: true`; route files building a chain — `routes/b.yaml` consumes `direct:transform`, sets body `"B"`, forwards to `direct:step-c1`; `routes/c1.yaml` consumes `direct:step-c1`, sets body `"B.C1"`, forwards to `direct:step-c2`; `routes/c2.yaml` consumes `direct:step-c2`, sets body `"B.C1.C2"`, forwards to `direct:final`; `routes/a.yaml` consumes `direct:final`, sets body `"B.C1.C2.A"`. Reuse `MULTI_JOB_CONFIG`/`MULTI_INCLUDE` for the configuration pair.
2. Extend the `Fixture` struct and its initializer with a `multi_job_n: PathBuf` field compiled from those inputs (model on the existing `multi_job` compilation).
3. Add the test below.

**Tests:**
- name: `compiled_job_multi_entry_boots_every_entry`
  - setup: the compiled `multi_job_n` artifact, deployed via `deploy_artifact` into a directory without the source tree, config, or routes; the fixture's original source directory still exists for the CLI parity leg.
  - action: first run `camel job ingest-n.job.yaml --config Camel.toml --report cli-report.json` in the fixture source directory through the battery's existing `CARGO_BIN_EXE_camel` spawn pattern (precedent: the arg-default tests); then run the deployed artifact with `--report report.json`.
  - assert: the CLI run exits 0 with outcome `Completed` and reply body `B.C1.C2.A`; the artifact run exits 0; the artifact report JSON records `outcome == "Completed"`, `reply.body == "B.C1.C2.A"` (job-level parity: same outcome and reply as `camel job` on the same document set, and the chain traversed all four files' routes), and `document == "compiled://ingest-n.job.yaml"`; none of the source paths exist in the deploy directory.
  - command: `TMPDIR=/home/shared/tmp cargo test -p camel-cli --test compiled_artifact_test compiled_job_multi_entry_boots_every_entry`
  - expected: passes on current main (behavior-pinning; a failure means a latent single-entry assumption — report it, do not widen the boot seam without escalating).

**Acceptance:**
- The new test exits 0; full `compiled_artifact_test` battery passes.
- `cargo fmt --check --all` and `cargo clippy -p camel-cli -- -D warnings` exit 0.

- [x] 2.1

### Task 2.2: Per-entry failure names the entry with no partial boot

**Files:**
- `crates/camel-cli/tests/compiled_artifact_test.rs` (modified)

**Steps:**
1. Add fixture constants for the failing-entry variant: same job document shape as Task 2.1 but with three route files — two valid chain links and one (`routes/bad.yaml`) containing an unknown step key (`totally_not_a_step:`) under a `direct:` consumer, so the document is well-declared but structurally invalid (compilation must allow it; boot must reject it).
2. Extend the `Fixture` struct and initializer with a `multi_job_bad: PathBuf` field compiled from those inputs; assert at fixture-build time only that compile exits 0 (structure-invalid documents compile — the existing `compile_allows_structure_invalid_but_well_declared_job` precedent).
3. Add the test below.

**Tests:**
- name: `compiled_job_multi_entry_failure_names_entry`
  - setup: the compiled `multi_job_bad` artifact deployed without its source tree.
  - action: run the artifact with `--report bad-report.json`; capture stdout, stderr, exit code.
  - assert: exit 2; stderr contains `compiled://routes/bad.yaml`; no `bad-report.json` is produced (boot-class failure is stderr-only, no outcome report, no partial boot).
  - command: `TMPDIR=/home/shared/tmp cargo test -p camel-cli --test compiled_artifact_test compiled_job_multi_entry_failure_names_entry`
  - expected: passes on current main (behavior-pinning).

**Acceptance:**
- The new test exits 0; full `compiled_artifact_test` battery passes.

- [x] 2.2

### Task 2.3: Manifest lists every route entry without boot

**Files:**
- `crates/camel-cli/tests/compiled_artifact_test.rs` (modified)

**Steps:**
1. Add the test below against the existing `multi_job_n` fixture from Task 2.1 (no new fixture).

**Tests:**
- name: `artifact_manifest_lists_multi_entry_job_without_boot`
  - setup: the compiled `multi_job_n` artifact deployed without its source tree.
  - action: run the artifact with `--manifest`; parse the JSON.
  - assert: exit 0; `artifact_kind == "job"`; the `embedded_files` list contains the `job` entry (`ingest-n.job.yaml`), all four `route` entries (`routes/a.yaml`, `routes/b.yaml`, `routes/c1.yaml`, `routes/c2.yaml`) each with non-null `length` and digest fields, and the config/include entries; stdout carries no boot markers.
  - command: `TMPDIR=/home/shared/tmp cargo test -p camel-cli --test compiled_artifact_test artifact_manifest_lists_multi_entry_job_without_boot`
  - expected: passes on current main (behavior-pinning).

**Acceptance:**
- The new test exits 0; full `compiled_artifact_test` battery passes.
- `cargo fmt --check --all` and `cargo clippy -p camel-cli -- -D warnings` exit 0.

- [x] 2.3

## Phase 3: Documentation alignment

### Task 3.1: Align camel-cli CONTEXT.md job-plan semantics

**Files:**
- `crates/camel-cli/CONTEXT.md` (modified)

**Steps:**
1. In the `## Compiled artifacts (camel compile)` section, directly after the sentence that ends `identical inputs produce identical artifact bytes.`, add a short passage stating the job-kind plan rule: for job entry documents the source plan holds exactly the job document and its own file-form route expansions; configuration `routes` patterns do not seed the job plan (the `camel job` exactly-one-source parity — an overlapping pattern is not a duplicate-source rejection for jobs); a file-form job route source resolving zero route files fails compilation naming the document; structurally invalid route entries still compile and fail at boot with the entry named.
2. Keep the passage consistent with the spec delta requirement `Compile and boot multi-entry job artifacts` in `openspec/changes/r3jobs/specs/cli-compile/spec.md` — same terms, no new promises.

**Tests:**
- name: `context-citations-and-doc-grep`
  - setup: edited `crates/camel-cli/CONTEXT.md`.
  - action: `cargo xtask lint-context-citations` and a grep confirming the new passage names both rules (config patterns do not seed the job plan; zero route files fails compile).
  - assert: lint exits 0; grep finds both phrasings.
  - command: `cargo xtask lint-context-citations`
  - expected: passes after the edit.

**Acceptance:**
- `cargo xtask lint-context-citations` exits 0.
- English prose, STE-compatible wording, no new non-English text.

- [x] 3.1
