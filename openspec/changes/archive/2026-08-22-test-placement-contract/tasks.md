# Tasks: test-placement-contract

## Phase 1: Reserved suffix law (D-RUN)

### Task 1.1: camel-dsl discovery owns the reserved suffix

**Files:**
- `crates/camel-dsl/src/discovery.rs` (modified)

**Steps:**
1. Add `pub fn is_test_document(path: &Path) -> bool` near `pattern_targets_json` (discovery.rs:147): returns true when the file name (lossy) ends with `.test.yaml` or `.test.yml`.
2. Add `fn pattern_is_literal(pattern: &str) -> bool`: returns true when the pattern contains none of the glob metacharacters `*`, `?`, `[`, `]`, `{`, `}`. (A literal pattern names exactly one file.)
3. Add error variant `ReservedTestSuffix { path: String }` to `DiscoveryError` (enum is `#[non_exhaustive]`) with a `thiserror` display: `Route file {path} uses the reserved '.test.yaml' suffix, which names a camel test document, not a route. Run it with 'camel test {path}', or rename it if it is a route.`
4. In `discover_routes_inner` (discovery.rs:220), inside the glob-entry loop (pattern is in scope per entry, discovery.rs:234-243), immediately after `path` resolves and before the extension gate (`match ext.as_deref()`): if `is_test_document(&path)` and `pattern_is_literal(pattern)` return `Err(DiscoveryError::ReservedTestSuffix { path: path_str })`; if `is_test_document(&path)` and the pattern has wildcards, `continue` (skip the entry, no error, no read).
5. Keep the existing extension gate and JSON gate untouched for non-test-suffixed names.

**Tests:** (in `discovery.rs` `#[cfg(test)]`, following the existing tempdir fixtures; each test = one `#[test]` fn; run group: `cargo test -p camel-dsl --lib`; every new test fails or does not compile before implementation — `is_test_document`, `pattern_is_literal`, `ReservedTestSuffix` do not exist yet)
- `test_doc_skipped_under_wildcard_pattern`: **setup** tempdir with `demo.yaml` (block-style valid routes document, mirroring existing fixtures at discovery.rs:574+) and `demo.test.yaml` (any bytes). **action** `discover_routes(&[format!("{dir}/routes/*.yaml")])`. **assert** `Ok` with exactly one route from `demo.yaml`. **pre-impl** fails: today the wildcard path parses `demo.test.yaml` as a route and errors.
- `test_doc_literal_pattern_hard_errors`: **setup** tempdir with `demo.test.yaml`. **action** `discover_routes(&[format!("{dir}/routes/demo.test.yaml")])`. **assert** `Err(ReservedTestSuffix)` whose display contains `camel test`. **pre-impl** compile error (variant absent); behavior pre-fix: parse failure of a different error kind.
- `yml_test_doc_also_skipped`: **setup** tempdir with `demo.yml` + `demo.test.yml`. **action** `discover_routes(&[format!("{dir}/routes/*.yml")])`. **assert** `Ok` with exactly the `demo.yml` route. **pre-impl** fails (`.yml` test doc parsed as route).
- `wildcard_over_only_test_docs_returns_empty`: **setup** tempdir containing only `routes/demo.test.yaml`. **action** `discover_routes(&[format!("{dir}/routes/*.test.yaml")])`. **assert** `Ok(vec![])`, no error. **pre-impl** fails (parse error on the test doc).
- `test_json_name_not_test_suffixed`: **setup** tempdir with `x.test.json`. **action** `discover_routes(&[format!("{dir}/routes/*")])`. **assert** `Err(JsonRequiresExplicitPattern)` — the JSON gate, NOT the suffix rule. **pre-impl** passes already (guards regression).
- `is_test_document_predicate`: **action** call predicate on file names. **assert** `("a.test.yaml", true) ("a.test.yml", true) ("atest.yaml", false) ("a.yaml", false) ("x.test.json", false)`. **pre-impl** compile error.

**Acceptance:**
- `cargo clippy -p camel-dsl -- -D warnings` exits 0.
- `cargo test -p camel-dsl --lib` passes, including the six tests above.
- `is_test_document` and `ReservedTestSuffix` are reachable as `camel_dsl::discovery::` public items.

- [x] 1.1

### Task 1.2: camel-cli run consumes discovery filtering; delete local filter

**Files:**
- `crates/camel-cli/src/commands/run.rs` (modified)

**Steps:**
1. Delete `expand_patterns_excluding_test_docs` (run.rs:707-731) and the tests that call it: `test_docs_excluded_from_default_expansion`, `yml_suffix_also_excluded`, and `non_matching_pattern_returns_empty` (run.rs:894-903; expansion no longer exists, its concern moves to the renamed verbatim test below).
2. Rewrite `resolve_route_patterns_with` (run.rs:741) so every branch returns patterns verbatim: `Some(override)` → `vec![ov]`; non-empty `config_routes` → clone; otherwise → `defaults.to_vec()`. Update its doc comment: discovery (camel-dsl) owns the reserved-suffix skip and error, on every path.
3. Post-rewrite, `resolve_route_patterns_with` is behaviorally identical to `raw_route_patterns` (run.rs:775-786) — both return unexpanded patterns. Collapse: delete `raw_route_patterns`, update the watch call site at run.rs:603 to use `resolve_route_patterns`, delete the `raw_patterns` binding at run.rs:242, and switch every `raw_patterns` use (run.rs:250, 492, 585, 594, 623 — watch-dir derivation and logs) to the `patterns` variable. The watch nuance survives: the resolver still returns unexpanded globs, so an initially-empty routes dir still yields a watched root.
4. Repurpose `raw_patterns_stay_unexpanded_for_watch_dirs` (run.rs:906-913) as `resolver_returns_unexpanded_globs` asserting `resolve_route_patterns` returns the glob verbatim (no expansion, no filtering) — this is the watch-root guard.
5. Rewrite `none_returns_expanded_defaults` as `none_returns_defaults_verbatim`: `resolve_route_patterns_with(&[pat], &None, &None) == vec![pat.to_string()]`.
6. Keep `override_passthrough_untouched` and `config_routes_passthrough_untouched` passing unchanged.

**Tests:** (run group: `cargo test -p camel-cli --lib run`; pre-impl: the rewritten/repurposed tests fail against today's expansion behavior because they assert the NEW verbatim contract)
- `none_returns_defaults_verbatim` (rewritten): **setup** tempdir with `routes/demo.yaml` + `routes/demo.test.yaml`; `pat = format!("{dir}/routes/*.yaml")`. **action** `resolve_route_patterns_with(&[pat], &None, &None)`. **assert** result equals `vec![pat.to_string()]` — no expansion, no filtering.
- `resolver_returns_unexpanded_globs` (repurposed): **setup** a glob string `routes/**/*.yaml`. **action** `resolve_route_patterns` with the glob as override, and separately with the glob inside `config_routes`. **assert** both return the glob string verbatim (watch-dir derivation input).
- `override_passthrough_untouched`: **setup** override string. **action** `resolve_route_patterns(&Some(ov), &None)`. **assert** `vec![ov]` (existing test, unchanged).
- `config_routes_passthrough_untouched`: **setup** config entries vec. **action** `resolve_route_patterns(&None, &Some(vec![...]))`. **assert** entries verbatim (existing test, unchanged).
- `literal_test_doc_path_reaches_discovery`: **setup** tempdir with `routes/demo.test.yaml`; `p = format!("{dir}/routes/demo.test.yaml")`. **action** `resolve_route_patterns(&Some(p), &None)`. **assert** `vec![p.to_string()]` verbatim (the ReservedTestSuffix error is discovery's job, asserted in Task 1.1).

**Acceptance:**
- `rg -n "expand_patterns_excluding_test_docs" crates/` returns zero hits.
- `cargo clippy -p camel-cli -- -D warnings` exits 0.
- `cargo test -p camel-cli --lib run` passes.

- [x] 1.2

### Task 1.3: camel lint applies the shared predicate with info skip

**Files:**
- `crates/camel-cli/src/commands/lint.rs` (modified)
- `crates/camel-cli/tests/lint_corpus.rs` (modified)
- `crates/camel-cli/tests/lint_test_doc_skip.rs` (new)

**Steps:**
1. Add field `pub cli_info: Option<String>` to `LintOutcome` (lint.rs:26). Initialize it `None` at every existing construction site (lint.rs:64, 76, 89).
2. In `run_lint` (lint.rs:60), before reading the file: if `camel_dsl::discovery::is_test_document(path)` return `LintOutcome { diagnostics: vec![], source: String::new(), exit_code: 0, cli_error: None, cli_info: Some(format!("skipped: {} is a camel test document", path.display())) }`.
3. In `lint.rs::run()` (lint.rs:98-109 — where `run_lint` is called and `cli_error` is printed via `eprintln!`), print `cli_info` to stdout when present, then keep the existing `std::process::exit(outcome.exit_code)` at :108. `main.rs` only dispatches (`commands::lint::run(args).await`) and needs no change.
4. In `lint_corpus.rs` `discover_corpus` (:94-97), replace the local `ends_with` closure with `camel_dsl::discovery::is_test_document(p)`. Keep the doc comment; note the predicate is now shared with discovery.

**Tests:** (run group: `cargo test -p camel-cli --lib lint` plus `cargo test -p camel-cli --test lint_test_doc_skip`; pre-impl: in-process tests fail because `cli_info`/predicate branch absent; subprocess test fails because today the engine lints the test doc and emits R-SCHEMA diagnostics with exit 1)
- `lint_skips_test_document_with_info` (lint.rs tests, async, mirroring existing `run_lint` tests): **setup** tempdir with `demo.test.yaml` containing `routeFiles: [x]` + `expects: {}`. **action** `run_lint(&path).await`. **assert** `exit_code == 0`, empty `diagnostics`, `cli_info` contains `camel test document`.
- `lint_routes_normal_yaml_unchanged` (lint.rs tests): **setup** minimal valid route file. **action** `run_lint(&path).await`. **assert** `exit_code == 0`, `cli_info == None` (guards predicate overreach).
- `cli_lint_test_doc_prints_single_info_line` (tests/lint_test_doc_skip.rs): **setup** tempdir with `demo.test.yaml` (`routeFiles`/`expects` content). **action** `Command::new(env!("CARGO_BIN_EXE_camel")).args(["lint", demo_test_yaml_absolute_path])`, capture stdout/stderr. **assert** exit code 0, stdout contains exactly one line mentioning `demo.test.yaml` and `camel test document`, stderr has no diagnostics.
- Update any existing `LintOutcome` literal tests to the new `cli_info: None` field.

**Acceptance:**
- `cargo test -p camel-cli --lib lint` passes.
- `cargo test -p camel-cli --test lint_corpus` passes with the shared predicate.

- [x] 1.3

### Task 1.4: ADR-0062 and CONTEXT-MAP key terms

**Files:**
- `docs/adr/0062-reserved-test-suffix-and-placement-contract.md` (new)
- `CONTEXT-MAP.md` (modified)
- `crates/camel-config/README.md` (modified)

**Steps:**
1. Write ADR-0062 following the house ADR format (see `docs/adr/0061-unified-transport-auth.md` for structure: Context / Decision / Consequences / Alternatives). Record: (1) `.test.yaml`/`.test.yml` is a reserved suffix owned by `camel test`; (2) camel-dsl discovery always skips it on wildcard patterns and hard-errors on explicit literal naming (explicit-gate idiom, cross-reference `DiscoveryError::JsonRequiresExplicitPattern`); (3) the suffix rule lives only in `camel-dsl::discovery::is_test_document` — run, watch, lint, corpus all consume it; (4) colocation is the blessed placement, separate-dir is first-class via `routeFilesFromRoot` (lands in this same change); (5) sigil rejection rationale: `@/` frontend idiom, `$` collides with `${env:}`, `~/` reads as home, URI pseudo-schemes muddy the endpoint model; (6) accepted breaking change: explicit `*.test.yaml` glob override removed — route-shaped files using the suffix must be renamed; (7) known costs: watch no-op wake on test saves, wildcard-over-only-tests reports no routes. Also state the monorepo semantics: root anchoring uses the nearest ancestor `Camel.toml`, not the workspace root.
2. Add two entries to the `## Key Terms`-style glossary section of `CONTEXT-MAP.md` (find where ADR-0059/0060 terms like *MCP Server Consumer* live, entries with Authority citations): **Reserved test suffix** — `.test.yaml`/`.test.yml` names a camel test document; route discovery skips it and literal naming errors. Authority: ADR-0062. (camel-dsl + camel-cli) — and **Route/test placement** — colocation is the blessed default, separate-dir first-class via `routeFilesFromRoot` anchored at nearest ancestor `Camel.toml`. Authority: ADR-0062. (camel-cli).
3. STE-writing discipline: short sentences, no banned verbs, no em-dashes.
4. Update `crates/camel-config/README.md`: in the `routes` field documentation (the `[String]` glob-patterns row around README.md:35 and :128), state that wildcard globs skip `*.test.yaml`/`*.test.yml` camel test documents and that naming one literally is rejected with the reserved-suffix error. One or two sentences, referencing `camel test`.

**Tests:** (run: `cargo xtask lint-context-citations`; pre-impl: passes today and must keep passing — it guards the NEW citations)
- `lint-context-citations gate`: **setup** ADR-0062 file created, CONTEXT-MAP.md entries citing it. **action** run the xtask gate from the worktree root. **assert** exit code 0 (all ADR references resolve).

**Acceptance:**
- `cargo xtask lint-context-citations` exits 0.
- ADR file exists, numbered 0062, follows the house format.
- CONTEXT-MAP.md contains both key terms citing ADR-0062.

- [x] 1.4

### Task 1.5: watch reload no-op integration test

**Files:**
- `crates/camel-cli/tests/run_watch_test_doc_test.rs` (new)

**Steps:**
1. New integration test file following the subprocess pattern of `crates/camel-cli/tests/run_exec_guard_test.rs` (`Command::new(env!("CARGO_BIN_EXE_camel"))`, piped stdio, `kill` cleanup).
2. Fixture: tempdir project with `Camel.toml` containing explicit `routes = ["routes/*.yaml"]` and `watch = true` (the former crash path), `routes/demo.yaml` (valid direct route), `routes/demo.test.yaml` (minimal test doc: `routeFiles: [demo.yaml]`, `inputs: []`, `expects: {mock:result: {count: 1}}`).
3. Test body: spawn `camel run` in the tempdir; wait for the startup marker in stdout/stderr (reuse the wait helper pattern from run_exec_guard_test.rs; generous 10s deadline); rewrite `routes/demo.test.yaml` with an identical body plus a trailing newline (touch semantics without truncating mid-read); sleep 2s past the debounce; assert the process is STILL ALIVE (it did not crash on reload); send SIGTERM via `kill`; assert the process exits (any code) and its captured stderr contains no discovery error naming `demo.test.yaml`.

**Tests:**
- `watch_reload_test_doc_save_is_noop`: full flow above. **run** `cargo test -p camel-cli --test run_watch_test_doc_test -- --test-threads=1`. **assert** alive-after-save, no `demo.test.yaml` error in stderr. **pre-impl** fails: today the explicit-`routes` watch path re-globs, parses the test doc, and the run dies or errors on save.

**Acceptance:**
- `cargo test -p camel-cli --test run_watch_test_doc_test -- --test-threads=1` passes.
- Test uses no sleeps shorter than the CLI debounce window without justification; kill cleanup is unconditional (drop guard) so a failed assertion cannot leak the process.

- [x] 1.5

## Phase 2: Root-anchored test documents (D-TEST)

### Task 2.1: routeFilesFromRoot field with three-way exclusivity

**Files:**
- `crates/camel-cli/src/commands/test/document.rs` (modified)

**Steps:**
1. Add field `pub route_files_from_root: Option<Vec<String>>` to `TestDocument` after `route_files` (document.rs:37). The struct already has `#[serde(rename_all = "camelCase")]`, so the YAML key is `routeFilesFromRoot` with no extra attribute. Doc comment: paths resolved against the nearest ancestor `Camel.toml` directory.
2. Extend error enum `TestDocError` with `NoProjectRoot { doc_dir: String }`, display: `routeFilesFromRoot requires a Camel.toml in an ancestor directory of {doc_dir}; none was found.` Keep `std::error::Error` impl coverage (document.rs:183 pattern).
3. Convert `TestDocError::RouteSourceConflict` from a unit variant to a struct variant `RouteSourceConflict { present: Vec<&'static str> }` carrying the declared source keys (values from `["routeFiles", "routeFilesFromRoot", "routes"]`). Display: when `present` is non-empty, name the present keys and state they are mutually exclusive with exactly one required; when empty, state that exactly one route source (`routeFiles`, `routeFilesFromRoot`, or `routes`) is required. Update the Display arm (document.rs:154-157) and every existing construction site.
4. Replace the two-way check at document.rs:211 (`route_files.is_some() == routes.is_some()`) with a three-way count over `route_files`, `route_files_from_root`, `routes`: collect the present key names; if the count is not exactly 1, return `RouteSourceConflict { present }`.
5. Leave every other validation step ((b)-(f) in `parse_test_document`) untouched and in the same order.

**Tests:** (document.rs tests, following existing style at :267+; run group: `cargo test -p camel-cli --lib document`; pre-impl: every new test fails — `routeFilesFromRoot` is an unknown field (deny_unknown_fields) or the RouteSourceConflict shape mismatch makes the case not error as asserted)
- `route_files_from_root_parses`: **setup** YAML doc with `routeFilesFromRoot: [config/routes.yaml]` + one valid `mock:` expect. **action** `parse_test_document(text)`. **assert** `Ok`, `route_files_from_root == Some(vec!["config/routes.yaml"])`, `route_files == None`, `routes == None`.
- `route_files_from_root_plus_route_files_rejected`: **setup** doc with both `routeFiles` and `routeFilesFromRoot` + valid expects. **action** parse. **assert** `Err(RouteSourceConflict { present })` where display names `routeFiles` and `routeFilesFromRoot`.
- `route_files_from_root_plus_routes_rejected`: **setup** doc with both `routeFilesFromRoot` and inline `routes`. **action** parse. **assert** `Err(RouteSourceConflict { present })`, display names both keys.
- `no_route_source_rejected`: **setup** doc with valid expects and none of the three source keys. **action** parse. **assert** `Err(RouteSourceConflict { present: vec![] })`, display states exactly one route source is required.
- `all_three_route_sources_rejected`: **setup** doc with `routeFiles` + `routeFilesFromRoot` + `routes`. **action** parse. **assert** `Err(RouteSourceConflict { present })` naming all three keys.
- `route_files_and_routes_still_rejected`: **setup** legacy doc with `routeFiles` + `routes`. **action** parse. **assert** `Err(RouteSourceConflict { present })` naming both (regression guard).

**Acceptance:**
- `cargo clippy -p camel-cli -- -D warnings` exits 0.
- `cargo test -p camel-cli --lib document` passes, including the six tests above.

- [x] 2.1

### Task 2.2: root walk-up resolution in the runner

**Files:**
- `crates/camel-cli/src/commands/test/runner.rs` (modified)

**Steps:**
1. Add `pub(crate) fn find_camel_toml_root(start: &Path) -> Option<PathBuf>` in `runner.rs`: iterate `start.ancestors()` (includes `start` itself); the first directory containing `Camel.toml` is the root. Strict walk — no workspace `Cargo.toml` fallback. Doc comment MUST cross-reference `find_camel_root` in `crates/camel-cli/src/commands/plugin.rs:249` and state the semantic difference: `find_camel_root` picks the nearest marker of EITHER kind (Camel.toml or workspace Cargo.toml) per ancestor level, so the two must not be merged; do not refactor plugin.rs.
2. In `load_routes` (runner.rs:80), add the `routeFilesFromRoot` branch before the `routeFiles` branch: call `find_camel_toml_root(doc_dir)`; `None` → return `Err(TestDocError::NoProjectRoot { doc_dir: doc_dir.display().to_string() }.to_string())`; `Some(root)` → for each entry `root.join(entry)`, load via `camel_dsl::load_from_file` exactly like the `routeFiles` branch. Inline `routes` branch unchanged.
3. Ensure the error string surfaces as a document-level error (it flows through `TestDocResult.doc_error` → exit code 2 per the existing path in test.rs).

**Tests:** (runner.rs `#[cfg(test)]` unit tests only — integration resolution tests live in Task 2.3; run group: `cargo test -p camel-cli --lib runner`; pre-impl: compile errors — field from Task 2.1 exists but no branch consumes it, helper absent)
- `find_camel_toml_root_strict_walk`: **setup** tempdir; write `Camel.toml` at its top; create nested subdir `a/b` inside it. **action** `find_camel_toml_root(&b)`. **assert** `Some(tempdir_root)`.
- `find_camel_toml_root_no_marker_is_none`: **setup** tempdir whose top has a `Cargo.toml` (workspace marker, NOT accepted) and a nested subdir. **action** call on the nested subdir. **assert** `None` (strictness guard — plugin's fallback semantics must not leak in).

**Acceptance:**
- `cargo clippy -p camel-cli -- -D warnings` exits 0.
- `cargo test -p camel-cli --lib` passes, including the two unit tests above.

- [x] 2.2

### Task 2.3: routeFilesFromRoot integration tests

**Files:**
- `crates/camel-cli/tests/test_runner.rs` (modified)

**Steps:**
1. Add fixtures and tests to the existing integration test file `crates/camel-cli/tests/test_runner.rs` (read its existing helpers first; reuse its scaffold/tempdir style).
2. In-process tests use `commands::test` entry points exactly as existing tests in that file do.
3. The cwd-independence test runs `camel test` as a CHILD PROCESS (`Command::new(env!("CARGO_BIN_EXE_camel"))`) with `.current_dir(second_unrelated_tempdir)` and the document given by absolute path — never mutate the test process's own cwd.

**Tests:** (run group: `cargo test -p camel-cli --test test_runner -- route_files_from_root`; pre-impl: every test fails — `routeFilesFromRoot` is an unknown field, parse exit 2)
- `route_files_from_root_nested_doc_passes`: **setup** project tempdir with `Camel.toml` (minimal `[default]` table) at its top, `routes/orders.yaml` (`direct:start` → `mock:result`), and `tests/integration/orders.test.yaml` declaring `routeFilesFromRoot: [routes/orders.yaml]`, one `direct:` input, `expects: {mock:result: {count: 1}}`. **action** run the document (absolute path) through the in-process `run_tests` entry. **assert** exit code 0, one PASS line, `failed == 0`.
- `route_files_from_root_nearest_ancestor_wins`: **setup** outer tempdir with `Camel.toml` at top; inside it `services/orders/Camel.toml` and `services/orders/routes/a.yaml` (valid direct route) and `services/orders/tests/a.test.yaml` declaring `routeFilesFromRoot: [routes/a.yaml]` — `routes/a.yaml` exists ONLY under `services/orders/`. **action** run in-process. **assert** exit code 0, PASS (resolved against `services/orders`, the nearest ancestor).
- `route_files_from_root_no_root_exit_2`: **setup** tempdir with the test doc declaring `routeFilesFromRoot` and NO `Camel.toml` anywhere in its ancestors. **action** run in-process. **assert** exit code 2 and error output contains `NoProjectRoot` plus the document directory path.
- `route_files_from_root_cwd_independent_subprocess`: **setup** same project fixture as `nested_doc_passes`, plus a second unrelated tempdir. **action** child `camel test` invoked with the test document's absolute path and `.current_dir(second_tempdir)`. **assert** exit code 0. No `../` anywhere in the document.

**Acceptance:**
- `cargo test -p camel-cli --test test_runner` passes, including the four tests above.
- No test mutates the test process working directory.

- [x] 2.3

## Phase 3: Scaffold teaches the contract

### Task 3.1: camel new basic template — testable route, colocated test, README

**Files:**
- `crates/camel-cli/templates/basic/routes/hello.yaml` (modified)
- `crates/camel-cli/templates/basic/routes/hello.test.yaml` (new)
- `crates/camel-cli/templates/basic/README.md.tpl` (modified)
- `crates/camel-cli/tests/new_test.rs` (modified)

**Steps:**
1. Rewrite `templates/basic/routes/hello.yaml` to the testable shape (same step spelling as `examples/yaml-dsl/config/mock-demo.yaml`), keeping a one-line leading comment `# Sample route for camel test — see routes/hello.test.yaml`:
   ```yaml
   routes:
     - id: "hello"
       from: "direct:start"
       steps:
         - set_header:
             key: "source"
             value: "hello"
         - to: "mock:result"
   ```
2. Create `templates/basic/routes/hello.test.yaml` with exactly (leading comment mirroring the route file's):
   ```yaml
   routeFiles: [hello.yaml]

   inputs:
     - to: "direct:start"
       body: "hello"

   expects:
     mock:result:
       count: 1
       bodies: ["hello"]
       headers:
         source: "hello"
   ```
   Deterministic; no `settle` key.
3. In `README.md.tpl`, insert a `## Test` section between the title and `## Run`: one line, `camel test routes/hello.test.yaml`. Keep the existing `## Run` and later sections unchanged.
4. `embedded.rs` `collect_files` already picks up every file under the template dir except `Camel.toml.*` and `README.md.tpl`, so `routes/hello.test.yaml` is emitted with no code change. Verify by reading `collect_files` (embedded.rs:23-45); do not modify it.
5. Update `new_test.rs`: rewrite `hello_yaml_is_valid` to assert `id: "hello"`, `direct:start`, `mock:result` (drop `timer:tick`/`log:` assertions). Add `scaffolds_colocated_test_doc` (assert `routes/hello.test.yaml` exists in the scaffold and contains `routeFiles: [hello.yaml]`). Add `readme_teaches_test_before_run` (assert `## Test` appears before `## Run` in the generated README). Add `scaffolded_test_doc_passes` (`#[tokio::test]`: scaffold to tempdir, call `commands::test::run_tests(&[project.join("routes/hello.test.yaml")], &mut stdout, &mut stderr)`, assert exit code 0 and `failed == 0`). Add `scaffolded_project_run_discovery_skips_test_doc` (scaffold to tempdir, `camel_dsl::discover_routes(&[format!("{}/routes/*.yaml", project.display())])` returns exactly one route, id `hello`).
6. Add `scaffolded_project_runs_under_camel_run`: subprocess test following the `Command::new(env!("CARGO_BIN_EXE_camel"))` + kill pattern from `run_exec_guard_test.rs` — scaffold to tempdir, spawn `camel run` with `.current_dir(project)`, wait up to 10s for a startup marker in captured output (route/context started line, same helper style as run_exec_guard_test.rs), then SIGTERM via `kill` and assert the process terminates without a discovery error naming `hello.test.yaml` in captured stderr.

**Tests:** (run group: `cargo test -p camel-cli --test new_test`; pre-impl: new tests fail — template files absent, README has no `## Test`, scaffolded route is timer-based)
- `scaffolds_colocated_test_doc`: **setup** scaffold via existing `run_new` helper. **action** read `routes/hello.test.yaml` from the project. **assert** file exists and content contains `routeFiles: [hello.yaml]`.
- `readme_teaches_test_before_run`: **setup** scaffolded project. **action** read generated `README.md`. **assert** `find("## Test")` and `find("## Run")` are both `Some` and Test index < Run index.
- `scaffolded_test_doc_passes`: **setup** scaffolded project. **action** `#[tokio::test]` calling `commands::test::run_tests(&[project.join("routes/hello.test.yaml")], &mut stdout, &mut stderr)`. **assert** `exit_code == 0`, `failed == 0`.
- `scaffolded_project_run_discovery_skips_test_doc`: **setup** scaffolded project. **action** `camel_dsl::discover_routes(&[format!("{project}/routes/*.yaml")])`. **assert** `Ok` with exactly 1 route, id `hello`.
- `scaffolded_project_runs_under_camel_run`: **setup** scaffolded project. **action** subprocess per step 6. **assert** startup marker reached within 10s, process terminated by kill, captured stderr has no `hello.test.yaml` discovery error.
- `hello_yaml_is_valid` (rewritten): **setup** scaffolded project. **action** read `routes/hello.yaml`. **assert** contains `direct:start` and `mock:result`, does NOT contain `timer:tick`.
- Existing layout tests regression guard: **setup** scaffolded projects from the unmodified existing tests (`creates_project_with_env_layout`, `creates_project_with_simple_layout`, nested/absolute-path tests). **action** run `cargo test -p camel-cli --test new_test`. **assert** all pre-existing tests still pass without edits to their assertion bodies.

**Acceptance:**
- `cargo test -p camel-cli --test new_test` passes, including the six new/rewritten tests.
- `cargo clippy -p camel-cli -- -D warnings` exits 0.
- Scaffold template tree contains `routes/hello.yaml` + `routes/hello.test.yaml` + `README.md.tpl` with `## Test` before `## Run`.
