# Tasks: job-ux-reshape

<!--
  Single-phase change. No `## Phase N` heading: one coherent deliverable
  (jobs leave the test family, gain a home and a listing), matching
  design.md's single-phase declaration. Task order is deliberate:
  the dsl predicate (1.1) and config table (1.2) land first because
  every later task consumes them; the gate flip (1.3) renames fixtures
  in the same commit as the flip so the suite never has a red window;
  docs (1.7) land last with the sweep gate.
-->

## camel-dsl

### Task 1.1: Split the reserved-suffix predicate and rename the discovery error

**Files:**
- `crates/camel-dsl/src/discovery.rs` (modified)

**Steps:**
1. Add `pub fn is_job_document(path: &Path) -> bool` directly below `is_test_document` (discovery.rs ~163): true when the file name ends with `.job.yaml` or `.job.yml` (same shape as `is_test_document`).
2. Add `pub fn is_reserved_document(path: &Path) -> bool`: `is_test_document(path) || is_job_document(path)`, with a doc comment naming both reserved families and their owners (`camel test`, `camel job`).
3. Rename enum variant `DiscoveryError::ReservedTestSuffix { path: String }` → `ReservedDocumentSuffix { path: String }`. New message: `"Route file {path} uses a reserved document suffix ('.test.yaml'/'.test.yml' names a camel test document; '.job.yaml'/'.job.yml' names a camel job document). Run it with 'camel test {path}' or 'camel job {path}', or rename it if it is a route."` Update the variant's doc comment (lines ~59-64) to the two-family wording.
4. Switch the route-discovery skip-gate (line ~327: `if is_test_document(&path)`) to `if is_reserved_document(&path)`; update the gate's comment to say "reserved test/job documents".
5. Audit every doc comment in `discovery.rs` that says the reserved suffix belongs to `camel test` only (lines ~59-64, ~161, ~1658) and re-word for the two-family world. Keep `is_test_document`'s own doc accurate for the test family.
6. Update the in-file tests: rename the `ReservedTestSuffix` matcher at ~1585-1592 to `ReservedDocumentSuffix`; extend `is_test_document_predicate` (~1677) with a sibling `is_job_document_predicate` (`a.job.yaml`/`a.job.yml` true; `ajob.yaml`/`a.yaml`/`x.job.json` false) and an `is_reserved_document_predicate` (true for one suffix of each family, false for `a.yaml`); mirror the `.test.yaml` wildcard-skip and literal-error tests (~1546-1680) with `.job.yaml` twins (wildcard `routes/**/*.yaml` brushing `jobs/x.job.yaml` skips silently; literal `routes/x.job.yaml` pattern errors with `ReservedDocumentSuffix`).

**Tests:** (in `crates/camel-dsl/src/discovery.rs` test module; `cargo test -p camel-dsl discovery`)
- `is_job_document_predicate`: paths `a.job.yaml`, `a.job.yml` → true; `ajob.yaml`, `a.yaml`, `x.job.json` → false.
- `is_reserved_document_predicate`: `a.test.yaml` and `a.job.yaml` → true; `a.yaml` → false.
- `job_doc_skipped_under_wildcard`: temp dir with `routes/x.yaml` + `routes/x.job.yaml`, pattern `routes/*.yaml` → discovery returns only `x.yaml`'s routes, no error.
- `job_doc_literal_pattern_errors`: temp dir with `routes/x.job.yaml`, literal pattern `routes/x.job.yaml` → `DiscoveryError::ReservedDocumentSuffix` naming the path (mirrors the existing `.test.yaml` literal test).
- existing `test_doc_skipped_under_wildcard` / literal `.test.yaml` tests: still pass unchanged (family parity).

**Acceptance:**
- `cargo test -p camel-dsl discovery` passes including the new twins.
- `cargo clippy -p camel-dsl --all-targets -- -D warnings` exits 0.
- `grep -n 'ReservedTestSuffix' crates/camel-dsl/src/discovery.rs` → no hits.

- [x] 1.1

## camel-config

### Task 1.2: Add the `[jobs]` table with the `dir` key

**Files:**
- `crates/camel-config/src/config.rs` (modified)

**Steps:**
1. Add `pub struct JobsCamelConfig { pub dir: String }` decorated `#[serde(deny_unknown_fields)]` with a doc comment: jobs directory for `camel job` discovery, resolved against the Camel.toml root; default `"jobs"`. Follow the nested-struct naming style of the existing tables (e.g. `HealthCamelConfig`, `StreamCachingConfig`).
2. Wire `jobs` through all three merge sites: (a) resolved `CamelConfig` (~line 15 area) gains `pub jobs: JobsCamelConfig` and its `Debug` field (~111); (b) the overlay struct (~140) gains `pub jobs: Option<JobsCamelConfig>` plus a builder method `jobs(mut self, v: JobsCamelConfig)` mirroring `routes`; (c) the layer merge (~182) gains `jobs: self.jobs.unwrap_or(defaults.jobs)`; (d) `impl Default for CamelConfig` (~206) gains `jobs: JobsCamelConfig { dir: "jobs".to_string() }`.
3. Add `"jobs"` to `KNOWN_TOP_LEVEL_KEYS` (config.rs ~2805) — the const MUST mirror `CamelConfig`'s serde field names or a `[jobs]` table silently false-warns as an "unselected profile" (the `known_top_level_keys_*` tripwires in `config_ergonomics_tests` guard this drift; keep them green).
4. Add round-trip tests to the config test module: `[jobs]` absent → `dir == "jobs"`; `[jobs] dir = "ops/jobs"` → survives merge; `[jobs] unknown = 1` → TOML deserialisation error (deny_unknown_fields); overlay builder `jobs()` overrides a file value; `[jobs]` beside `[default]` with no `CAMEL_PROFILE` emits no "unselected profile" warning.
5. Do NOT add the key to `[context]`; the table is top-level (design rationale).

**Tests:** (in `crates/camel-config/src/config.rs` tests; `cargo test -p camel-config`)
- `jobs_table_defaults_when_absent`: parse a minimal `Camel.toml` without `[jobs]` → `config.jobs.dir == "jobs"`.
- `jobs_table_dir_round_trip`: `Camel.toml` with `[jobs]\ndir = "ops/jobs"` → `config.jobs.dir == "ops/jobs"`.
- `jobs_table_unknown_key_rejected`: `[jobs]\nbogus = true` → deserialisation error mentioning `unknown field`.
- `jobs_overlay_overrides_default`: overlay with `jobs` set merges over a file without `[jobs]` → overlay value wins.
- `jobs_table_no_unselected_profile_warning`: `Camel.toml` with `[jobs]` (and a `[default]` profile block) loaded without `CAMEL_PROFILE` → no "unselected profile" warning names `jobs`.

**Acceptance:**
- `cargo test -p camel-config` passes.
- `cargo clippy -p camel-config -- -D warnings` exits 0.
- `cargo xtask schema --check` exits 0 (config is not a ts-rs/schemars export — verify no drift; if it regenerates, commit the regenerated schema).

- [x] 1.2

## camel-cli job document

### Task 1.3: Flip the job-document suffix gate and add `description:`

**Files:**
- `crates/camel-cli/src/commands/job/document.rs` (modified)
- `crates/camel-cli/src/commands/job/document_tests.rs` (modified)
- `crates/camel-cli/tests/job_one_shot_test.rs` (modified)

**Steps:**
1. In `parse_job_document` (document.rs:246): replace the `is_test_document` gate (line 247) with `camel_dsl::discovery::is_job_document`.
2. Rename `JobDocError::NotTestSuffix { path: String }` (line 93) → `NotJobSuffix { path }`; update its `Display` arm (line 125) to: `job document {path} must use the reserved `.job.yaml`/`.job.yml` suffix (rename it — `*.test.yaml` names a camel test document)`.
3. Add `description: Option<String>` to `JobDocumentDoc` (line 200) with a doc comment "optional one-line description shown by `camel job` listing". `deny_unknown_fields` stays. `JobDocument` (the parsed output) does not gain the field — listing uses the probe parse, not the full grammar.
4. Keep the cross-family guards (scenario/unit-tier sections rejected beside `execute:`) exactly as they are; re-word their doc comments from shared-family defence to "reject test vocabulary in a job document" (belt-and-suspenders). Do NOT touch `classify()` or the body-scalar sentinel logic.
5. Update `document_tests.rs`: rename every inline document path/string fixture from `*.test.yaml` to `*.job.yaml`; extend the suffix-rejection test to assert the new `NotJobSuffix` variant and message for `foo.test.yaml`; add a case asserting a doc with `description: create a user` parses and one with `description: {a: 1}` (non-string) fails deserialisation.
6. Rename the fixtures in `tests/job_one_shot_test.rs`: `job.test.yaml` → `job.job.yaml` at every site (lines ~99-328) and every `run_job(dir.path(), "job.test.yaml")` argument; keep all assertions otherwise identical. The suite must be green in the same commit as the gate flip.
7. Verify (no code change expected) the `camel test` refusal: `crates/camel-cli/src/commands/test/document_parse.rs:45-50` already refuses `execute:` with a pointer to `camel job`, covered by `crates/camel-cli/src/commands/test/document_tests/parsing.rs:978-1003`. Run those tests; they must pass unchanged (a `.test.yaml` declaring `execute:` is refused by `camel test` at dispatch — suffix boundary holds).

**Tests:** (`cargo test -p camel-cli job`)
- `not_job_suffix_rejected`: `parse_job_document(Path::new("foo.test.yaml"), valid_body)` → `JobDocError::NotJobSuffix { path: "foo.test.yaml" }`, Display contains "must use the reserved `.job.yaml`".
- `description_optional_accepted`: doc with `description: create a user via direct:in` + valid `execute:` → parse succeeds.
- `description_non_string_rejected`: `description:` mapping → serde error (doc-validation class).
- existing mutual-exclusion tests (execute + scenario/expects): pass with `.job.yaml` fixtures.
- integration `job_one_shot_test.rs` suite: green after fixture rename (`cargo test -p camel-cli --test job_one_shot_test`).

**Acceptance:**
- `cargo test -p camel-cli --lib job` and `cargo test -p camel-cli --test job_one_shot_test` pass.
- `cargo test -p camel-cli --lib test::document_tests` (camel test refusal) passes unchanged.
- `cargo clippy -p camel-cli -- -D warnings` exits 0 (new public-enum variant: `lint-non-exhaustive` also green).
- `grep -rn 'NotTestSuffix' crates/camel-cli/` → no hits.

- [x] 1.3

## camel-cli callers

### Task 1.4: Switch non-route skip callers to `is_reserved_document`

**Files:**
- `crates/camel-cli/src/commands/lint.rs` (modified)
- `crates/camel-cli/tests/lint_corpus.rs` (modified)
- `crates/camel-cli/tests/lint_test_doc_skip.rs` (modified)
- `crates/camel-integration-test/src/boot_scenario.rs` (modified)
- `crates/camel-cli/src/commands/run_tests.rs` (modified)

**Steps:**
1. `lint.rs:85` — `camel lint` skips a document that is not a route; switch `is_test_document(path)` → `is_reserved_document(path)` and update the module doc: lint prints one info line for reserved documents (test or job). A literal `.job.yaml` given to `camel lint` gets the same skip as `.test.yaml`.
2. `tests/lint_corpus.rs:111` — the route-lint corpus filter must exclude job documents too: switch to `is_reserved_document`; update the comment at line 73.
3. `tests/lint_test_doc_skip.rs` — update the module doc comment (line 7) from `is_test_document` to the reserved-document predicate; add a `.job.yaml` sibling to the existing `.test.yaml` fixture and assert it is also skipped by `camel lint` (same info-line semantics).
4. `crates/camel-integration-test/src/boot_scenario.rs:270` — this filters non-route documents out of route discovery; switch to `is_reserved_document`.
5. `run_tests.rs:100` — comment references `ReservedTestSuffix`; update the name and wording to `ReservedDocumentSuffix`.
6. Do NOT touch `crates/camel-cli/src/commands/test.rs:237` or `crates/camel-integration-test/src/document.rs:484` — those select the TEST family for `camel test` and keep `is_test_document`.

**Tests:**
- `lint_corpus` suite: `cargo test -p camel-cli --test lint_corpus` passes with the reserved predicate (a stray `x.job.yaml` in the corpus dir is not linted as a route).
- `lint_test_doc_skip`: new case `job_doc_also_skipped` — `camel lint` on a dir containing `x.job.yaml` prints the reserved-document info line, exits 0, does not parse it as a route.
- `cargo test -p camel-integration-test` compiles and the touched module's tests pass.

**Acceptance:**
- `cargo test -p camel-cli --test lint_corpus --test lint_test_doc_skip` passes.
- `grep -rn 'is_test_document' crates/camel-cli/src/commands/lint.rs crates/camel-cli/tests/lint_corpus.rs crates/camel-cli/tests/lint_test_doc_skip.rs crates/camel-integration-test/src/boot_scenario.rs` → no hits.

- [x] 1.4

## camel-cli job command

### Task 1.5: Optional document argument + deterministic bare-name resolution

**Files:**
- `crates/camel-cli/src/commands/job/mod.rs` (modified)
- `crates/camel-cli/src/main.rs` (modified)

**Steps:**
1. `JobArgs.document` (`mod.rs:57`): `PathBuf` → `Option<PathBuf>`; update its doc comment: "Path to the job document (`*.job.yaml`). A bare name resolves `{jobs.dir}/<name>.job.yaml`; omitted lists the jobs directory."
2. `main.rs` job subcommand help (~line 104): "Run one job from a *.job.yaml document with an `execute:` section." Keep the trust-model paragraph.
3. In `run_job` (`mod.rs:124`): when `args.document` is `None`, print `no job document given` to stderr and return 2 — for both the `--report` and plain cases (intermediate behaviour). Task 1.6 replaces the plain no-document branch with the listing call and keeps the `--report` case at exit 2 (listing and the JSON report never co-occur).
4. Resolution of a document argument — new private fn `fn resolve_job_path(raw: &Path, jobs_root: &Path) -> Result<PathBuf, String>`: if `raw` components contain a path separator (`raw.components().count() > 1` covers `./x`, `a/b`; also treat a `raw` whose file name ends with `.yaml`, `.yml`, or `.json` as explicit) → return `raw.to_path_buf()` (explicit path always wins, including `.job.yml`); else probe exactly `jobs_root.join(format!("{}.job.yaml", name))` and on miss return `Err(format!("no job `{name}` in `{}` (looked for {name}.job.yaml)", jobs_root.display()))`.
5. In `run_job`, compute the jobs root BEFORE resolving the document via a single shared helper `fn jobs_root(args: &JobArgs, camel_config: &CamelConfig) -> PathBuf`: `crate::commands::run::canonical_project_root(Path::new(&args.config)).join(&camel_config.jobs.dir)` — anchored at the Camel.toml root, never the CWD (the existing `project_root` at line 228 stays for `routeFilesFromRoot`). Task 1.6's listing MUST call this same helper, not re-derive the path. Resolution errors print to stderr and return 2. The resolved path feeds the existing `canonicalize` + parse flow unchanged.
6. Integration tests in `tests/job_one_shot_test.rs` (fixtures create their own `Camel.toml`): bare name `run_job(dir, "job")` with `jobs/job.job.yaml` + `Camel.toml` → runs (exit as before); bare miss `run_job(dir, "nope")` → exit 2, stderr names `nope.job.yaml` and `jobs/`; explicit `run_job(dir, "ops/x.job.yml")` → uses the path as-is; document without any route-source key under `jobs/` → exit 2 route-source error (no `routes/` fallback — assert stderr does NOT mention route discovery of `routes/`); subdir anchoring — invoke with CWD set to a nested dir (tempdir chdir helper) while `--config` points at the root `Camel.toml` → bare name still resolves the root `jobs/`.

**Tests:** (`cargo test -p camel-cli --test job_one_shot_test`)
- `bare_name_resolves_from_jobs_dir`: `jobs/job.job.yaml` + bare `job` → runs, exit code as explicit-path twin.
- `bare_name_miss_single_error`: no `nope.job.yaml` → exit 2, stderr contains "no job `nope`" and "nope.job.yaml" exactly once.
- `explicit_path_wins`: `ops/x.job.yml` passed explicitly → used as-is, no jobs-dir probing (missing `jobs/` dir does not error).
- `route_source_still_mandatory`: bare-resolved doc without route source → exit 2, route-source error, stderr has no `routes/` glob mention.
- `jobs_dir_anchored_at_config_root`: CWD in nested subdir, `--config` at root → bare name loads root `jobs/job.job.yaml`.
- `report_without_document_is_usage_error`: `--report out.json`, no document → exit 2, stderr usage error, stdout empty (this task's `None`-document behaviour).

**Acceptance:**
- `cargo test -p camel-cli --test job_one_shot_test` passes with the new cases.
- `cargo clippy -p camel-cli -- -D warnings` exits 0.

- [x] 1.5

### Task 1.6: No-argument listing of the jobs directory

**Files:**
- `crates/camel-cli/src/commands/job/mod.rs` (modified)
- `crates/camel-cli/tests/job_one_shot_test.rs` (modified)

**Steps:**
1. Add `fn list_jobs(args: &JobArgs) -> i32` (private, same file): resolve the jobs root by calling Task 1.5's `fn jobs_root(args, camel_config)` helper (loading the config the same way `run_job` does); read dir entries; keep files whose name ends `.job.yaml` or `.job.yml` (reuse `is_job_document` on the entry path); sort by name; for each, strip the suffix for the display name and probe the description.
2. Probe: `fn probe_description(path: &Path) -> Option<Option<String>>` — reads the file and runs `serde_yaml::from_str::<JobListProbe>` via the crate's YAML facade `noyalib::compat::serde_yaml` (camel-cli has no direct serde_yaml dependency; do NOT add one) where `#[derive(Deserialize)] struct JobListProbe { description: Option<String> }` (NO `deny_unknown_fields` — the probe ignores every other key). Outer `None` = unreadable or unparseable (row renders `(unparseable)`); outer `Some(None)` = parseable without `description:` (renders `(no description)`); outer `Some(Some(d))` = the description string.
3. Render one line per job: `  {name}      {description-or-(no description)}`; a multiline description renders with every `\n` (and `\r\n`) replaced by a single space; unparseable rows show `(unparseable)`. Header line `Jobs in {jobs_dir}/:` where `{jobs_dir}` is the configured dir string.
4. Empty or absent dir: print to stdout `No jobs found in {jobs_dir}/. Create a `<name>.job.yaml` there, or run `camel job <path>`.` and return 0.
5. Non-empty dir: print header + rows to stdout, return 0. Replace Task 1.5's `None`-document early return: no-document now calls `list_jobs` UNLESS `args.report.is_some()`, which keeps the Task 1.5 usage-error exit 2 (listing and report never co-occur; assert in tests).
6. Integration tests (fixtures per scenario): mixed dir with and without `description:`; empty dir; absent dir; unparseable sibling (`{not yaml` content); `legacy.job.yml` listed as `legacy` but `camel job legacy` misses (bare sugar probes `.job.yaml` only); multiline block-scalar description renders on one line; `--report` with no document still exits 2 with empty stdout.

**Tests:** (`cargo test -p camel-cli --test job_one_shot_test`)
- `listing_shows_names_and_descriptions`: `create-user.job.yaml` (with `description: create a user via direct:in`) + `reindex.job.yaml` (none) → stdout has `create-user` + description and `reindex` + `(no description)`, exit 0.
- `listing_empty_dir_exit_0`: empty `jobs/` → stdout "No jobs found in jobs/", exit 0.
- `listing_absent_dir_exit_0`: no `jobs/` → same message, exit 0.
- `listing_unparseable_sibling`: sibling with invalid YAML content → listed as `(unparseable)`, valid job still listed, exit 0.
- `listing_job_yml_not_bare_resolvable`: `legacy.job.yml` only → listed as `legacy`; `camel job legacy` → exit 2 miss error naming `legacy.job.yaml`.
- `listing_multiline_description_one_line`: `description: |` block with two lines → single listing line containing both parts separated by one space, no `\n`.
- `listing_anchored_at_config_root`: CWD in a nested subdir, `--config` at the root `Camel.toml`, no document argument → the ROOT `jobs/` is listed (not `./jobs/` relative to CWD) — the listing-side twin of 1.5's run-side anchoring test, owning the spec's no-argument anchoring scenario.
- `listing_report_never_cooccur`: `--report` + no document → exit 2, stdout empty (no listing, no report).

**Acceptance:**
- `cargo test -p camel-cli --test job_one_shot_test` passes with the new cases.
- `cargo clippy -p camel-cli -- -D warnings` exits 0; `cargo fmt --check` clean.

- [x] 1.6

## repo docs

### Task 1.7: ADR-0062 amendment, CONTEXT-MAP term landing, and the `.test.yaml` job sweep

**Files:**
- `docs/adr/0062-reserved-test-suffix-and-placement-contract.md` (modified)
- `CONTEXT-MAP.md` (modified)
- `crates/camel-cli/CONTEXT.md` (modified)
- `crates/camel-cli/README.md` (modified, only if job-related `.test.yaml` mentions exist)

**Steps:**
1. Amend ADR-0062: change `**Status:** Accepted` → `Accepted (Amended 2026-09-11: reserved-document contract — see "Amendment")`; append an `## Amendment (2026-09-11): Two reserved suffixes` section recording: `.job.yaml`/`.job.yml` names a `camel job` document (bd rc-10d50, OpenSpec change `job-ux-reshape`); `is_reserved_document = is_test_document || is_job_document` is the discovery gate for both; `ReservedTestSuffix` renamed `ReservedDocumentSuffix`; jobs live under `[jobs].dir` (default `jobs`) while test placement rules are unchanged; a `*.test.yaml` declaring `execute:` is a load error directing a rename. Do NOT touch ADR-0069.
2. CONTEXT-MAP.md ADR index (line ~104): update the 0062 entry — the rule is now the two-suffix reserved-document contract living in `is_test_document`/`is_job_document`/`is_reserved_document` — and add the `Amended` marker to the entry per the index convention.
3. CONTEXT-MAP.md Key Terms: update the "Reserved test suffix" term (~185) to cite ADR-0062 (fixing the stale `ADR-0063` citation — 0063 is the Redis repository ADR) and the same fix on the "Route/test placement" term (~186); add two terms: "**Reserved job suffix** — `.job.yaml`/`.job.yml` names a `camel job` document, not a route; route discovery skips it under wildcard globs and errors on literal naming. Authority: ADR-0062 (amended). (camel-dsl + camel-cli)" and "**Jobs directory (`[jobs].dir`)** — the `camel job` discovery dir in `Camel.toml` (default `jobs`), anchored at the Camel.toml root; a job's route source stays explicit, never defaulted to `routes/` discovery. Authority: ADR-0062 (amended), cli-jobs spec. (camel-cli + camel-config)".
4. `crates/camel-cli/CONTEXT.md`: "camel job failure modes" (~64-70) — `*.test.yaml` → `*.job.yaml` in prose and the doc-load-error table row (non-`*.job.yaml` suffix); the job paragraph gains one sentence for bare-name resolution + no-arg listing with exit 0.
5. `crates/camel-cli/README.md`: check for job-related `.test.yaml` mentions; rename any that describe job documents (camel-test mentions stay). Also update the route-discovery sentence (~line 256, "discovery (initial load and watch reload) skips `*.test.yaml` and `*.test.yml`") to name both reserved families — it becomes false after Task 1.1.
6. Sweep gate: `grep -rn '\.test\.yaml' --include='*.rs' --include='*.md' --include='*.toml' --include='*.yaml' crates/ examples/ docs/ CONTEXT-MAP.md 2>/dev/null | grep -iv 'camel test\|test document\|mock-demo\|integration-testing\|hello\.test' | grep -i 'job\|execute'` → zero job-related survivors (camel-test-family mentions are legitimate). `examples/` job fixtures, if any exist, are renamed to `.job.yaml`.

**Tests:**
- `sweep_gate` (manual gate, recorded in the change report): the grep above returns no job-related `.test.yaml` reference in live code, tests, fixtures, examples, or docs; archived OpenSpec history (`openspec/changes/archive/`) is excluded.
- `cargo xtask lint-context-citations` passes (new ADR-0062 citations resolve, status Accepted).
- `cargo xtask lint-non-exhaustive`, `cargo xtask lint-unwrap` pass.

**Acceptance:**
- `cargo xtask lint-context-citations` exits 0.
- ADR-0062 carries the amendment section and the `Amended` status; CONTEXT-MAP index entry matches.
- Sweep gate grep clean for job-related mentions.

- [ ] 1.7
