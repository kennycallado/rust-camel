# Tasks: eliminate-devdep-cycles

## Phase 0: SCC-accurate detector + diagnostic

### scripts/xtask

#### Task 0.1: SCC-gated cycle breaking + return broken_weak_edges

**Files:**
- `scripts/xtask/src/main.rs` (modified)

**Steps:**
1. Add `fn tarjan_scc(adj: &[Vec<(usize, EdgeKind)>]) -> Vec<Vec<usize>>` — iterative Tarjan over the combined normal+weak graph (both edge kinds traversable). Returns SCCs (each a vec of node indices).
2. Add `fn find_intra_scc_weak_edge(adj: &[Vec<(usize, EdgeKind)>], scc: &[usize], name_of: impl Fn(usize)->String) -> Option<(usize, usize)>` returning the lexicographically-smallest `(holder_idx, target_idx)` weak edge whose both endpoints are in `scc`, or `None`.
3. Rewrite the cycle-breaking loop in `resolve_publish_order` (~L3295–3352) to: (a) run Kahn; (b) when the queue empties but unscheduled nodes remain, compute `tarjan_scc` over the subgraph induced by unscheduled nodes; (c) collect non-trivial SCCs (size > 1, or size-1 with self-loop); (d) if NONE of the non-trivial SCCs contains a breakable intra-SCC weak edge → return the existing `Err("Cannot compute publish order due to dependency cycles")` (this is the hard normal-only-cycle case); (e) otherwise pick the single globally-lexicographically-smallest breakable intra-SCC weak edge across ALL non-trivial SCCs, record it in `broken_weak_edges` as `(holder_name, target_name)` where holder = the declaring crate, break it, and RE-RUN Kahn + recompute SCC (one edge per global iteration, then full recompute). Repeat until all scheduled or hard cycle confirmed.
4. REMOVE the `eprintln!` cycle-report block from `resolve_publish_order` (reporting moves to callers — `resolve_publish_order` returns data only).
5. Change the return type to `Result<(Vec<WorkspaceCrate>, HashSet<String>, Vec<(String,String)>), String>` — third element is `broken_weak_edges` `(holder, target)` pairs. `no_verify` (second) stays the set of holders.
6. Update call sites: `publish_order` (L3388) destructures `_broken` and prints the report there (it is the order-printing command); `publish_crates` (L3474) destructures `broken` and uses it for the existing `--no-verify` messaging until Phase 3.

**Tests:** (name / setup / action / assert / command / expected)
- `tarjan_identifies_nontrivial_scc` / a 3-node cycle fixture → Tarjan returns one SCC of size 3 / assert the SCC contents.
- `acyclic_weak_graph_breaks_zero_edges` / fixture A-weak->B-weak->C (no back path) → `resolve_publish_order` returns empty broken_weak_edges, all scheduled, empty no_verify / `cargo test -p xtask acyclic_weak` / passes after impl; before impl the old loop would have fabricated edges.
- `deterministic_lexicographic_edge_selection` / fixture with two candidate intra-SCC weak edges (holder names "b","a") → the broken edge's holder is "a" (lexicographically smallest).
- `recompute_scc_after_each_break` / nested fixture requiring 2 breaks → broken_weak_edges has exactly 2 entries, no third phantom.
- `hard_normal_only_cycle_errors` / fixture with A-normal->B-normal->A (no weak edge) → returns `Err` containing "dependency cycles".

**Acceptance:**
- `cargo run -p xtask -- publish-order` prints only real intra-SCC broken edges (expected holders: `camel-core`, `camel-endpoint-macros`-class), NOT 25 phantom edges.
- `resolve_publish_order` contains no `eprintln!` (data-only).
- `cargo fmt --check --all` and `cargo clippy -p xtask -- -D warnings` pass.

- [x] 0.1

#### Task 0.2: `publish --show-cycles` diagnostic with a testable seam

**Files:**
- `scripts/xtask/src/main.rs` (modified)

**Steps:**
1. Add `#[arg(long)] show_cycles: bool` to `Commands::Publish { dry_run }`; destructure in the dispatch arm (~L332).
2. Add `fn show_cycles(workspace_root: &Path, w: &mut impl std::io::Write) -> Result<(), String>`: calls `resolve_publish_order`; destructures `(sorted, no_verify, broken)`; writes to `w`: `"no_verify set: {n} crate(s)"`, each holder sorted, `"broken weak edges:"`, each `(holder, target)`; returns `Ok(())`. Performs NO `cargo publish`, NO manifest write.
3. In the dispatch, when `show_cycles` is true call `show_cycles(&workspace_root, &mut std::io::stdout())` and `return Ok(())` (do NOT call `publish_crates`).
4. `publish_order` (the existing print command) reuses the same formatting helper for its broken-edge report (extract a `fn format_cycle_report(no_verify: &HashSet<String>, broken: &[(String,String)]) -> String` used by both).

**Tests:**
- `show_cycles_formats_holder_first` / a tempdir fixture workspace with a known cycle → capture `Vec<u8>` writer → assert output contains `"no_verify set: 1 crate(s)"` and a `"holder --dev/build-dep--> target"`-style line with holder named first / `cargo test -p xtask show_cycles_formats` / passes after impl.
- `show_cycles_clean_graph` / tempdir acyclic fixture → captured writer output is `"no_verify set: 0 crate(s)"` + `"broken weak edges:"` with zero lines.
- CLI integration (ignored-by-default or xtask-test-gated): `cargo run -p xtask -- publish --show-cycles` on the real workspace → stdout has no `Publishing` line and `git status --short` is byte-identical (no manifest mutation).

**Acceptance:**
- `cargo run -p xtask -- publish --show-cycles` exits 0, prints the true `no_verify` set, publishes nothing, mutates no manifest.
- `show_cycles` is unit-testable without invoking the CLI dispatch.

- [x] 0.2

#### Task 0.3: Record baseline

**Files:**
- `openspec/changes/eliminate-devdep-cycles/baseline.md` (new)

**Steps:**
1. Run `cargo run -p xtask -- publish --show-cycles`; capture output.
2. Write `baseline.md` with date, the `no_verify` holder set, the broken-edge `(holder, target)` list, and the count. Note this is the post-SCC-fix truthful baseline (expected small: `camel-core`, `camel-endpoint-macros`).

**Tests:**
- File-existence; content lists the expected real holders (not 16 phantom crates).

**Acceptance:**
- `baseline.md` exists with a non-empty `no_verify` set reflecting the two real SCCs.

- [x] 0.3

## Phase 1: Cut the 3 real edges

### crates/camel-core

#### Task 1.1: Stub http/ws in catalog tests + relocate real-option tests

**Files:**
- `crates/camel-core/src/component_metadata_catalog.rs` (modified)
- `crates/camel-test/tests/core_catalog_real_metadata_test.rs` (new) — header `//! Origin: camel-core/src/component_metadata_catalog.rs cfg(test) (relocated per ADR-0055)`.
- `crates/camel-test/Cargo.toml` (modified) — add non-optional `[dev-dependencies]` for `camel-component-cron`, `camel-component-sql`, `camel-component-opensearch`, `camel-component-container` (WS already a dev-dep on L63; http is already a normal dep).
- `crates/camel-core/Cargo.toml` (modified) — remove `camel-component-http` and `camel-component-ws` from `[dev-dependencies]`.

**Steps:**
1. In the `#[cfg(test)] mod tests` block of `component_metadata_catalog.rs`, define a private `StubComponent { scheme: String, metadata: ComponentMetadata }` implementing the `Component` trait (the trait `Registry::register` accepts — read `crates/camel-api/src/` for the exact trait). Implement EVERY required method; for `create_endpoint` return `Err(EndpointCreationFailed)` (the catalog never invokes it — it reads metadata only). Provide a constructor `StubComponent::new(scheme: &str, with_options: Vec<&str>)` that builds a `ComponentMetadata` with the given scheme and a non-empty options list.
2. The catalog cfg(test) functions are: `catalog_exposes_registered_metadata` (L8), `catalog_query_capabilities_default_impl` (L25), `all_phase2_schemes_have_options` (L40), `no_duplicate_option_names` (L86), `all_components_in_catalog` (L137), `no_duplicate_option_names_all` (L209). Only http/ws are cyclic; the two `catalog_*` functions use TimerComponent only (non-cyclic) — leave them unchanged.
3. **STUB** http/ws in `all_components_in_catalog` (L137) and `no_duplicate_option_names_all` (L209): replace `HttpComponent`/`WsComponent` registrations with `StubComponent::new("http", vec!["opt"])` / `StubComponent::new("ws", vec!["opt"])` (the stubs MUST carry the real scheme names `"http"`/`"ws"` because those tests look up schemes by name; the synthetic non-empty options keep option-count assertions valid). Remove the `use camel_component_http::HttpComponent` and `use camel_component_ws::WsComponent` lines from those functions. Preserve every assertion.
4. **RELOCATE** `all_phase2_schemes_have_options` (L40) and `no_duplicate_option_names` (L86) verbatim into `crates/camel-test/tests/core_catalog_real_metadata_test.rs` (they assert the REAL http/ws option catalog a stub cannot reproduce). Add the needed `use` lines. Delete them from `component_metadata_catalog.rs`.
5. Remove `camel-component-http` and `camel-component-ws` from `crates/camel-core/Cargo.toml` `[dev-dependencies]`. RETAIN all other dev-deps (TimerComponent and the rest are non-cyclic once http/ws are gone — verify in Task 1.3).

**Tests:**
- `cargo test -p camel-core --lib component_metadata_catalog` / stubbed functions pass; zero `camel_component_http`/`camel_component_ws` references remain in the file / passes after impl.
- `cargo test -p camel-test --test core_catalog_real_metadata_test` / relocated `all_phase2_schemes_have_options` and `no_duplicate_option_names` pass unchanged.

**Acceptance:**
- `grep -rn 'camel_component_http\|camel_component_ws' crates/camel-core/src/component_metadata_catalog.rs` → no hits.
- `camel-component-http` and `camel-component-ws` absent from camel-core `[dev-dependencies]`.
- Both test suites green; the two `catalog_*` TimerComponent fixture tests unchanged.

- [x] 1.1

### crates/camel-endpoint-macros, crates/camel-endpoint

#### Task 1.2: Relocate endpoint-macros derive + trybuild UI tests to camel-endpoint

**Files (exact):**
- DELETED from `crates/camel-endpoint-macros/tests/`: `derive_integration.rs`, `ui_tests.rs`, `ui/duplicate_path_field_fail.rs`, `ui/duplicate_path_field_fail.stderr`, `ui/kind_typo_fail.rs`, `ui/kind_typo_fail.stderr`, `ui/missing_uri_scheme_fail.rs`, `ui/missing_uri_scheme_fail.stderr`, `ui/non_struct_fail.rs`, `ui/non_struct_fail.stderr`, `ui/no_optin_no_metadata_fn_fail.rs`, `ui/no_optin_no_metadata_fn_fail.stderr`, `ui/secret_with_default_fail.rs`, `ui/secret_with_default_fail.stderr`, `ui/unknown_key_fail.rs`, `ui/unknown_key_fail.stderr`.
- NEW in `crates/camel-endpoint/tests/`: `endpoint_macros_derive_integration_test.rs` (header `//! Origin: camel-endpoint-macros/tests/derive_integration.rs (relocated per ADR-0055)`), `endpoint_macros_ui_tests.rs` (header `//! Origin: camel-endpoint-macros/tests/ui_tests.rs`), and the `ui/` fixtures moved verbatim to `crates/camel-endpoint/tests/ui/` (same 7 `.rs` + 7 `.stderr` filenames, unchanged contents so trybuild snapshots stay valid).
- `crates/camel-endpoint/Cargo.toml` (modified) — add `trybuild` (match endpoint-macros' version) to `[dev-dependencies]`.
- `crates/camel-endpoint-macros/Cargo.toml` (modified) — remove `camel-api` and `camel-endpoint` from `[dev-dependencies]`.

**Steps:**
1. Copy `crates/camel-endpoint-macros/tests/derive_integration.rs` verbatim → `crates/camel-endpoint/tests/endpoint_macros_derive_integration_test.rs` with the origin header.
2. Copy `crates/camel-endpoint-macros/tests/ui_tests.rs` verbatim → `crates/camel-endpoint/tests/endpoint_macros_ui_tests.rs` with the origin header. Keep its `trybuild::TestCases` `ui/` relative path unchanged (the `ui/` dir moves with it).
3. Copy the entire `ui/` directory (7 `.rs` + 7 `.stderr`) verbatim → `crates/camel-endpoint/tests/ui/`.
4. Delete the originals from `crates/camel-endpoint-macros/tests/` (the three entries above).
5. Add `trybuild` to `crates/camel-endpoint/Cargo.toml` `[dev-dependencies]` at the same version endpoint-macros used (read endpoint-macros Cargo.toml for the exact spec). camel-endpoint already normal-deps `camel-endpoint-macros` and `camel-api` → zero new edges.
6. Remove `camel-api` and `camel-endpoint` from `crates/camel-endpoint-macros/Cargo.toml` `[dev-dependencies]`. Keep `trybuild`/`syn`/`quote`/`proc-macro2` and any pure unit-test deps; pure unit tests in `src/` (e.g. `uri_config.rs`) stay.

**Tests:**
- `cargo test -p camel-endpoint --test endpoint_macros_derive_integration_test` / passes (same assertions as origin) / passes after impl.
- `cargo test -p camel-endpoint --test endpoint_macros_ui_tests` / all 7 trybuild UI cases compile/fail as before (snapshots unchanged).
- `cargo test -p camel-endpoint-macros` / remaining pure unit tests pass.

**Acceptance:**
- `crates/camel-endpoint-macros/tests/` contains neither `derive_integration.rs` nor `ui_tests.rs` nor `ui/`.
- `camel-api` and `camel-endpoint` absent from endpoint-macros `[dev-dependencies]`.
- All relocated tests green under `camel-endpoint`; endpoint-macros pure unit tests still pass.

- [x] 1.2

### scripts/xtask + relocation manifest

#### Task 1.3: Verify empty no_verify + write relocation manifest

**Files:**
- `crates/camel-test/tests/RELOCATIONS.md` (new)

**Steps:**
1. Run `cargo run -p xtask -- publish --show-cycles`. Assert the output is `no_verify set: 0 crate(s)` and `broken weak edges:` with zero lines. If any holder remains → STOP and report the remaining `(holder, target)` edge rather than inventing remediation.
2. Run `cargo test -p camel-core -p camel-endpoint -p camel-endpoint-macros -p camel-test -p camel-otel` → all green.
3. Run `cargo build --benches -p camel-core` → passes (benches use TimerComponent, a retained non-cyclic dev-dep).
4. Write `crates/camel-test/tests/RELOCATIONS.md` as a table with ONE row per moved item (18 rows total): 2 catalog test FUNCTIONS (`all_phase2_schemes_have_options`, `no_duplicate_option_names`) → `crates/camel-test/tests/core_catalog_real_metadata_test.rs`; and 16 endpoint-macros FILES (`derive_integration.rs`→`endpoint_macros_derive_integration_test.rs`, `ui_tests.rs`→`endpoint_macros_ui_tests.rs`, and the 7 `.rs` + 7 `.stderr` under `ui/` moved verbatim to `crates/camel-endpoint/tests/ui/`) → their exact destinations. Each row: origin path → destination path.

**Tests:**
- name: `relocation_manifest_destinations_exist` / setup: read `RELOCATIONS.md` after Phase 1 / action: parse each destination path / assert: every destination exists on disk; the table has 18 data rows; for the 16 FILE rows each origin path no longer exists under the old location; for the 2 FUNCTION rows (`all_phase2_schemes_have_options`, `no_duplicate_option_names`) the function definition is absent from `crates/camel-core/src/component_metadata_catalog.rs` (the origin FILE stays, only the functions move) / command: a check script (xtask test or `tests/relocation_manifest_check.rs` reading the file + grep) / expected: passes after impl.
- name: `show_cycles_zero_after_phase1` / setup: real workspace post-Phase-1 / action: run `--show-cycles` / assert: output contains `no_verify set: 0 crate(s)` / command: `cargo run -p xtask -- publish --show-cycles` / expected: 0 edges.
- name: `affected_test_suites_green` / setup: post-Phase-1 workspace / action: run the affected suites / assert: all pass / command: `cargo test -p camel-core -p camel-endpoint -p camel-endpoint-macros -p camel-test -p camel-otel` / expected: pass.

**Acceptance:**
- `--show-cycles` reports 0 edges; `RELOCATIONS.md` accounts for every move in Phase 1.

- [x] 1.3

## Phase 2: Document + enforce the invariant

### docs/adr

#### Task 2.1: Write ADR-0055 + cite from CONTEXT-MAP.md

**Files:**
- `docs/adr/0055-publish-topology-no-cyclic-devdeps.md` (new)
- `CONTEXT-MAP.md` (modified)

**Steps:**
1. Create ADR-0055 following `docs/adr/0054-*.md` structure. Title: "Publish topology: no cyclic dev/build-dependencies on publishable crates".
2. Decision: a publishable crate MUST NOT declare a `camel-*` dev/build-dependency that closes a publish-order cycle (SCC-accurate detection); `camel-test` is the publish-order leaf sink; remediation patterns are StubComponent substitution (incidental scaffolding) and proc-macro test relocation to the consumer crate (inherent proc-macro-testing cycle).
3. Document forces: publish-cycle constraint; SCC-accuracy requirement + the over-breaking-loop bug it fixes; the leaf invariant; the rejected manifest-mutation hack (dirty-tree failure mode, published-manifest ≠ source drift).
4. Rejected alternatives: mass relocation (phantom cycles from the buggy loop), `*-test-support` split crate (non-problem), `publish=false` on camel-test (already a leaf + downstream utility), keeping the hack.
5. In `CONTEXT-MAP.md`, add a one-line citation to ADR-0055.

**Tests:**
- name: `adr_0055_exists_and_cited` / setup: post-Task-2.1 tree / action: read `docs/adr/0055-publish-topology-no-cyclic-devdeps.md` and `CONTEXT-MAP.md` / assert: ADR file exists with the decision statement AND `CONTEXT-MAP.md` contains the string `ADR-0055` / command: `grep -l "ADR-0055" CONTEXT-MAP.md && test -f docs/adr/0055-publish-topology-no-cyclic-devdeps.md` / expected: passes after impl.
- name: `context_citations_lint_green` / setup: post-Task-2.1 tree / action: run the lint / assert: exit 0 / command: `cargo xtask lint-context-citations` / expected: exit 0.

**Acceptance:**
- ADR-0055 exists with decision + forces + rejected alternatives; `CONTEXT-MAP.md` cites it; lint-context-citations green.

- [x] 2.1

### scripts/xtask

#### Task 2.2: Implement `lint-publish-cycles` (SCC-accurate + leaf guard)

**Files:**
- `scripts/xtask/src/main.rs` (modified)

**Steps:**
1. Add `Commands::LintPublishCycles` variant (no args).
2. Implement `fn lint_publish_cycles(workspace_root: &Path) -> Result<(), String>`: (a) call `resolve_publish_order`; if `no_verify` non-empty → return `Err` listing each holder + its broken `(holder, target)` edge; (b) scan every workspace manifest; if any publishable crate (not `publish = false`) declares `camel-test` in `[dependencies]`/`[dev-dependencies]`/`[build-dependencies]` → return `Err` naming the crate + dependency kind; (c) on success return `Ok(())` (the dispatch prints `lint-publish-cycles: OK (0 violations)`).
3. Wire the dispatch arm mirroring `lint-unwrap` (`lint-publish-cycles: OK` on success; `lint-publish-cycles error: {e}` + non-zero exit on failure).
4. Both `--show-cycles` and this lint consume the SAME `resolve_publish_order` predicate.

**Tests:**
- `lint_fails_when_no_verify_nonempty` / setup: fixture workspace with one real cycle / action: call `lint_publish_cycles` / assert: returns `Err` naming the holder + edge / command: `cargo test -p xtask lint_fails_when_no_verify` / expected: passes after impl.
- `lint_passes_on_clean_graph` / setup: post-Phase-1 real workspace (no_verify empty) / action: `lint_publish_cycles` / assert: `Ok(())` / command: `cargo run -p xtask -- lint-publish-cycles` / expected: exit 0.
- `lint_fails_on_camel_test_dependent` / setup: a publishable fixture crate declaring `camel-test` as a dev-dep / action: `lint_publish_cycles` / assert: `Err` names crate + "dev-dependencies" / command: `cargo test -p xtask lint_fails_on_camel_test_dependent` / expected: passes after impl.
- `lint_and_show_cycles_report_identical_set` / setup: shared fixture with a known cycle / action: run both `show_cycles` and `lint_publish_cycles` / assert: the `no_verify` holder set each reports is byte-identical / command: `cargo test -p xtask lint_and_show_cycles_report_identical_set` / expected: passes after impl.
- `lint_asserts_camel_test_remains_publishable` / setup: read `crates/camel-test/Cargo.toml` / action: parse `[package]` / assert: `publish` is absent OR not `false` / command: `cargo test -p xtask lint_asserts_camel_test_remains_publishable` / expected: passes after impl.

**Acceptance:**
- `cargo run -p xtask -- lint-publish-cycles` exits 0 (graph clean post-Phase-1).
- `cargo clippy -p xtask -- -D warnings` + `cargo fmt --check --all` pass.

- [x] 2.2

### AGENTS.md

#### Task 2.3: Wire `lint-publish-cycles` into QUALITY GATES

**Files:**
- `AGENTS.md` (modified)

**Steps:**
1. In the `## QUALITY GATES` code block, add near the other `lint-*` gates:
   ```
   - name: lint-publish-cycles
     run: cargo xtask lint-publish-cycles
   ```

**Tests:**
- `cargo run -p xtask -- lint-publish-cycles` / exits 0 / passes.

**Acceptance:**
- `AGENTS.md ## QUALITY GATES` contains the `lint-publish-cycles` entry; gate green.

- [x] 2.3

## Phase 3: Delete the hack

### scripts/xtask

#### Task 3.1: Delete strip/restore code + simplify publish_crates (fail-closed on no_verify)

**Files:**
- `scripts/xtask/src/main.rs` (modified)

**Steps:**
1. Delete `comment_out_camel_dev_deps` (~L3678) + its doc comment.
2. Delete `is_weak_dependency_section` (~L3759) + its doc comment.
3. Delete the associated unit tests (`strip_comments_only_weak_camel_deps` ~L4843 and any sibling tests).
4. In `publish_crates` (L3474): BEFORE any network/filesystem publish action, destructure `(sorted, no_verify, broken)` and if `!no_verify.is_empty()` return `Err` produced by `format_cycle_report(&no_verify, &broken)` listing every holder + its broken `(holder, target)` edge (fail-closed — do NOT publish a graph whose cycles were repaired only by dropping weak edges). Then run the plain linear topo-sort loop over `sorted` with normal verification (no `--no-verify`).
5. Remove the deleted `needs_strip`/`restore` block (L3521–3568) entirely: the `restore: Option<String>` block, the `comment_out_camel_dev_deps` call, the stripped-manifest `std::fs::write`, the post-publish restore, and the `CRITICAL: failed to restore` path.
6. Remove the `--no-verify` flag from the `cargo publish` invocation. Keep the `dry_run` skip-existing behavior.
7. Verify no remaining reference to the deleted symbols compiles.

**Tests:**
- `publish_crates_builds_clean` / setup: post-Phase-3 xtask compiles with zero references to deleted symbols / action: `cargo build -p xtask` / assert: succeeds / command: `cargo build -p xtask` / expected: passes after impl.
- `deleted_symbols_gone` / setup: the source tree / action: grep / assert: `grep -n 'comment_out_camel_dev_deps\|is_weak_dependency_section' scripts/xtask/src/main.rs` returns no hits / command: (grep) / expected: empty.
- `show_cycles_still_zero` / setup: real workspace post-Phase-3 / action: run `--show-cycles` / assert: `no_verify set: 0 crate(s)` / command: `cargo run -p xtask -- publish --show-cycles` / expected: 0 edges.
- `lint_still_green` / setup: real workspace / action: run lint / assert: exit 0 / command: `cargo run -p xtask -- lint-publish-cycles` / expected: exit 0.
- `publish_crates_fail_closed_on_no_verify` / setup: fixture with non-empty no_verify / action: call `publish_crates` / assert: returns `Err` WITHOUT invoking `cargo publish` (assert no subprocess spawned — use an injectable spawner seam or assert on the early-return) / command: `cargo test -p xtask publish_crates_fail_closed_on_no_verify` / expected: passes after impl.
- `clean_publish_writes_no_manifest` / setup: real workspace, snapshot all `Cargo.toml` mtimes/hashes before / action: `cargo xtask publish --dry-run` / assert: no `Cargo.toml` modified (hashes unchanged), no `stripped + --no-verify` line in output / command: `cargo xtask publish --dry-run` + manifest hash compare / expected: identical hashes, no strip warning.

**Acceptance:**
- Deleted symbols unreferenced; `publish_crates` writes no `Cargo.toml` during the loop and errors if `no_verify` is non-empty; `--show-cycles` 0 edges; `lint-publish-cycles` exits 0; `cargo build --workspace` passes.

- [x] 3.1
