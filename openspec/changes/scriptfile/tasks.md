# Tasks: scriptfile

Single-phase change. Task order is dependency order: camel-api helpers
(1.1) → DSL parsing/resolution/hash (2.1) → watcher mechanics (3.1) →
watch registry + CLI wiring (4.1) → template materialization seam (5.1) →
schema + parity (6.1) → scaffold template (7.1) → docs (8.1).

## camel-api

### Task 1.1: Script file helpers and Rust DSL file constructors

**Files:**
- `crates/camel-api/src/script_source.rs` (new)
- `crates/camel-api/src/declarative.rs` (modified)
- `crates/camel-api/src/lib.rs` (modified)
- `crates/camel-api/Cargo.toml` (modified — dev-dependency: `tempfile` workspace dep for the tempdir tests)

**Steps:**
1. Create `script_source.rs` with `pub fn infer_script_language(path: &Path) -> Result<String, CamelError>`: match the extension case-sensitively — `.rhai` → `"rhai"`, `.js` → `"js"`, `.jsonpath` → `"jsonpath"`, `.xpath` → `"xpath"`, `.jinja` and `.jinja2` → `"minijinja"`; anything else returns `CamelError::RouteError` whose message names the resolved path and the extension, e.g. ``cannot infer script language for `<resolved>`: unknown extension `.xyz`; set `language:` explicitly``.
2. In the same file, `pub fn read_script_source(path: &Path) -> Result<String, CamelError>`: one `std::fs::read_to_string`; on IO error return `CamelError::RouteError` naming the resolved path and the source error; if the contents are empty after `trim()`, return `CamelError::RouteError` naming the resolved path with the word `empty`; otherwise return the contents verbatim (no trimming, no interpolation).
3. In `declarative.rs`, add `impl LanguageExpressionDef`:
   - `pub fn file(path: impl AsRef<Path>) -> Result<Self, CamelError>` — infers the language via `infer_script_language`, reads via `read_script_source`, returns `Self { language, source }`.
   - `pub fn file_with_language(path: impl AsRef<Path>, language: impl Into<String>) -> Result<Self, CamelError>` — skips inference, reads via `read_script_source`.
   Relative paths keep std CWD semantics (document with a doc comment on both constructors; note `expect`/`unwrap` are banned — return errors).
4. In `lib.rs`: `mod script_source;` plus `pub use script_source::{infer_script_language, read_script_source};`.

**Tests:** (command: `cargo test -p camel-api --lib`)
- `infer_script_language_maps_known_extensions`: tempdir files `a.rhai`, `b.js`, `c.jsonpath`, `d.xpath`, `e.jinja`, `f.jinja2` → each call returns the exact registry key (`rhai`, `js`, `jsonpath`, `xpath`, `minijinja`, `minijinja`).
- `infer_script_language_rejects_unknown_extension`: `x.xyz` → `Err`, message contains the resolved path and `xyz`.
- `read_script_source_rejects_missing_file`: no file at path → `Err`, message contains the resolved path.
- `read_script_source_rejects_empty_file`: file with only whitespace → `Err`, message contains `empty` and the resolved path.
- `read_script_source_returns_verbatim_contents`: file containing `${env:SECRET}` and trailing newline → returned string is byte-identical.
- `file_constructor_infers_language`: `LanguageExpressionDef::file("<tmp>/scripts/x.rhai")` with known contents → `Ok` with `language == "rhai"` and `source == contents`.
- `file_constructor_explicit_language_wins`: `LanguageExpressionDef::file_with_language("<tmp>/notes.txt", "simple")` → `Ok` with `language == "simple"` and the file's contents.
- `file_constructor_missing_file_errors`: `file("<tmp>/absent.rhai")` → `Err` containing the resolved path.
- `file_constructor_relative_missing_file_names_absolute_path`: `file("nonexistent_dir_xyz/absent.rhai")` relative to the process CWD → `Err` whose message contains an absolute path (CWD-joined) ending in `nonexistent_dir_xyz/absent.rhai` (no filesystem access needed to prove the join).

**Acceptance:**
- `cargo test -p camel-api --lib` exits 0.
- `cargo clippy -p camel-api --all-targets -- -D warnings` exits 0.
- `cargo fmt --check --all` exits 0.

- [x] 1.1

## camel-dsl

### Task 2.1: AST fields, source_file parsing, resolution, and hash folding

**Files:**
- `crates/camel-dsl/src/route_ast.rs` (modified)
- `crates/camel-dsl/src/yaml.rs` (modified)
- `crates/camel-dsl/src/json.rs` (modified)
- `crates/camel-dsl/src/discovery.rs` (modified)

**Steps:**
1. In `route_ast.rs`: change `ScriptData` to `{ language: Option<String>, source: Option<String>, source_file: Option<String> }` (keep `#[serde(deny_unknown_fields)]`, add `#[serde(default)]` on all three); derive `Default` on `ScriptData`; add `#[serde(default)] pub source_file: Option<String>` to `PredicateBlock` (route_ast.rs:915), `LoopWhileExpr` (route_ast.rs:557), derive `Default` on both, and to each of the 10 value-source step-data structs that carry `pub rhai: Option<String>` (route_ast.rs:565, 750, 771, 841, 891, 923, 1069, 2694, 2735, 2762). Fix every now-broken struct literal in `route_ast.rs` tests (wrap `language`/`source` in `Some(..)` plus `source_file: None`, or extend with `..Default::default()`).
2. In `yaml.rs`, introduce `pub(crate) struct ScriptLoadContext { base_dir: Option<PathBuf>, scripts: RefCell<BTreeMap<PathBuf, String>> }`. The sink is a `RefCell<BTreeMap<PathBuf, String>>`: dedups the probe-then-retry double parse in `parse_with_probe` (discovery.rs:701-712), keeps fold order deterministic by path, and keeps parse closures `Fn` — `parse_with_probe`/`try_coerced_subset` (`env_int_probe.rs:180,249`) require `Fn`, so a `&mut` context would turn the discovery parse closure into `FnMut` and break them; the RefCell confines mutability to the single-threaded document-parse scope (document that invariant on the struct). Methods: `fn resolve(&self, raw: &str) -> Result<PathBuf, CamelError>` — build the candidate (join `base_dir` when `raw` is relative; keep absolute as-is), then make it absolute WITHOUT touching the filesystem (`std::env::current_dir()` join when still relative) so every diagnostic names an absolute path; `fn record(&self, abs_path: PathBuf, contents: String)` — canonicalize the recorded path AFTER a successful read (symlink-realistic recording for the watcher) and insert; `fn into_scripts(self) -> BTreeMap<PathBuf, String>` for the hash fold.
3. Extend `parse_language_expression` (yaml.rs:2298) and `parse_value_source` (yaml.rs:2217) so the shared rules live in ONE place: introduce `fn resolve_script_expression(args: ExprSourceArgs<'_>, script_ctx: &ScriptLoadContext) -> Result<LanguageExpressionDef, CamelError>` where `struct ExprSourceArgs<'a> { language: Option<String>, source: Option<String>, source_file: Option<String>, simple: Option<String>, rhai: Option<String>, jsonpath: Option<String>, xpath: Option<String>, context: &'a str }` (params struct per the ADR-0091 named-fields norm; keeps clippy `too_many_arguments` quiet without allow-attributes — fall back to `#[allow(clippy::too_many_arguments)]`, precedent discovery.rs:656, only if the struct refactor ripples outside yaml.rs). The helper implements the counting rules: `source_file` present together with `source` → `CamelError::RouteError` naming the context, both field names, and the ABSOLUTE resolved path from `script_ctx.resolve()`, raised before any file IO; `source_file` present with any shorthand field (`simple`/`rhai`/`jsonpath`/`xpath`) → exactly-one error; `source_file` alone or with `language` → resolve to the ABSOLUTE path first (diagnostics-ready even when the file is missing), infer the language with `camel_api::infer_script_language` unless `language` is `Some` (explicit wins), read with `camel_api::read_script_source` (missing/empty errors therefore name the absolute path), record via `script_ctx.record`, and return `LanguageExpressionDef { language, source }` with the file contents. Extend the exactly-one alternatives error message and the `VALUE_SOURCE_ALTERNATIVES`/`VALUE_SOURCE_CONFIG_KEYS`/`VALUE_SOURCE_FIELD_ORDER` constants (yaml.rs:2128-2140) to name `source_file`. Call the helper from all three consumers: `parse_language_expression`, `parse_value_source`, and the script-step lowering arm (step 4).
4. Thread `&ScriptLoadContext` from the parse entry points — `load_from_file`/`load_from_file_with_env` (yaml.rs:2371/2388, base dir = parent of the file), `parse_document_routes` (discovery.rs:657, which gains an explicit `base_dir: Option<&Path>` parameter — file-backed callers pass the parent of the document path, while `discover_embedded_text` (discovery.rs:373-378, synthetic `compiled://` paths) and `discover_virtual_store` (discovery.rs:496) pass `None` for CWD semantics — do NOT derive the base dir from `path_str` inside), and `json.rs` `load_json_from_file` (json.rs:140) — through the step-parse dispatch to the call sites at yaml.rs:903, 922, 991, 1017, 1034, 1171, 1377, 1443, 1473, 1834, 3732. The script STEP never flows through `parse_language_expression` — `ScriptData` is built by serde — so resolve it at the lowering arm: change `RouteDslStep::Script(ScriptStep { script: ScriptData { .. } }` lowering (yaml.rs:1047-1051) to call `resolve_script_expression` with the `ScriptData` fields plus the context string (e.g. `"script"`), and do the same for `parse_predicate_block` (yaml.rs:2052) and `parse_loop_while_expr` (yaml.rs:2089). String-based parse entry points pass `base_dir: None` (CWD semantics). Public signature changes stay `pub(crate)` where possible; if a public API must grow a parameter, prefer internal helper fns so `load_from_file` signatures stay stable.
5. In `discovery.rs`: move the `source_hash` finalization to after parse — hash `raw_content`, then hash each recorded script content from `ctx.into_scripts()` (deterministic `BTreeMap` order; dedups the probe-then-retry double parse). Stamp routes at the regular route seams (discovery.rs:718 for yaml, discovery.rs:763 for json); the materialized-instance seam (discovery.rs:891-895) is Task 5.1's target. Keep the pre-parse hash removed only after the post-parse version compiles and tests pass.
6. Fix all `ScriptData` construction/destructuring fallout outside `route_ast.rs` (yaml.rs:1048 and any test sites found by `grep -rn "ScriptData" crates/camel-dsl/src`).

**Tests:** (command: `cargo test -p camel-dsl --lib`; reuse the `write_route_file` helper at yaml.rs:5835 for tempdir fixtures)
- `script_source_file_loads_with_inferred_language`: route doc `script: { source_file: "hello.rhai" }` + sibling non-empty `hello.rhai` → load succeeds; compiled expression has `language == "rhai"` and `source` equal to the file contents. Repeat the same fixture as JSON via `load_json_from_file`.
- `script_source_and_source_file_rejected_pre_io`: expression object with both fields, script file absent → `Err` message names the context, both field names, and the resolved path (absence of the file proves no IO was needed).
- `script_missing_source_form_error_names_source_file`: expression object with no source form → error message contains `source_file`.
- `script_shorthand_plus_source_file_rejected`: `rhai:` + `source_file:` → exactly-one error.
- `explicit_language_wins_over_extension`: `language: simple` + `source_file: notes.txt` → `Ok`, language `simple`.
- `unknown_extension_error_names_resolved_path`: `source_file: req.xyz` without `language:` → error contains the resolved path and `xyz`.
- `source_file_resolves_relative_to_route_file_dir`: `routes/doc.yaml` references `scripts/x.rhai`, script at `routes/scripts/x.rhai` → loads; a second fixture references `scripts/absent.rhai` → error names the ABSOLUTE resolved path `<tmp>/routes/scripts/absent.rhai` (proves both the base dir and the absolute diagnostic).
- `empty_script_file_rejected`: whitespace-only script → error contains `empty` and the resolved path.
- `script_contents_verbatim`: script containing `${env:SECRET}` loads with the literal text as source, env var unset.
- `value_source_accepts_source_file`: `set_header` value expression via `source_file` loads and compiles.
- `predicate_block_accepts_source_file`: `filter` with `source_file` loads.
- `loop_while_accepts_source_file`: `loop-while` with `source_file` loads.
- `absolute_source_file_path_loads_from_string_parse`: parse from string (no base dir) with an absolute `source_file` → loads.
- `route_source_hash_changes_on_script_only_edit`: discover a route referencing a script, edit only the script file, discover again → different `source_hash` on the returned `RouteDefinition`.
- `route_source_hash_stable_when_script_content_identical`: rewrite the script with identical bytes → same `source_hash`.

**Acceptance:**
- `cargo test -p camel-dsl --lib` exits 0 (existing `route_ast.rs` script tests at route_ast.rs:2072-2079 updated, still passing).
- `cargo clippy -p camel-dsl --all-targets -- -D warnings` exits 0.
- `cargo fmt --check --all` exits 0.

- [x] 2.1

## camel-core

### Task 3.1: Watcher script-source classification, reload trigger, and publish hook

**Files:**
- `crates/camel-core/src/hot_reload/adapters/reload_watcher.rs` (modified)
- `crates/camel-cli/src/commands/run.rs` (modified — minimal adaptation to keep the workspace compiling)
- `crates/camel-test/tests/hot_reload_integration.rs` (modified — pass a no-op publish hook at the `watch_and_reload` call sites, lines ~97, 185, 289, 380)
- `crates/camel-test/tests/dev_watch_template_test.rs` (modified — same, line ~165)
- `crates/camel-test/tests/dev_watch_proto_test.rs` (modified — same, line ~201)

**Steps:**
1. Introduce `pub enum DevSourceKind { Template, Script }` (PUBLIC — camel-cli and camel-test construct its variants; `pub(crate)` would make the kind-tagged provider signature unnameable outside camel-core) next to `DevTargetMap` (reload_watcher.rs:714) and export it on the crate surface camel-cli/camel-test already import (`camel_core::…reload_watcher…` — follow the existing re-export path used by `watch_and_reload` consumers; if none exists, add the module to the public hot-reload surface). Change `DevTargetMap::from_sources` to `&[(String, Vec<(PathBuf, DevSourceKind)>)]`; `contains_path`/`routes_for` keep template semantics; add `script_routes_for(path)` (or a kind-tagged equivalent) so the loop can tell template targets from script sources.
2. Add `EventClass::ScriptSource` to the enum at reload_watcher.rs:701 and extend `classify_event` (reload_watcher.rs:843): tracked template path → `DevTarget` (unchanged precedence), tracked script path → `ScriptSource`, route-extension file under a route root → `RouteFile`.
3. In `run_reload_loop` (reload_watcher.rs:559): treat a batch containing `RouteFile` OR `ScriptSource` as a full-reload trigger for `reload_pass()` (template-only batches keep the per-route `ReloadTemplates` dispatch path unchanged); a mixed batch runs exactly one pass. Enrich the pass result so publication can distinguish outcomes: change the `reload_pass` closure's return from `Result<Vec<String>, CamelError>` to a `PassOutcome { rebuilt: Vec<String>, removed: Vec<String>, aborted: bool }` — `rebuilt` stays `derive_rebuilt_route_ids(&actions, &errors)`; `removed` is the route ids of `ReloadAction::Remove` actions that executed WITHOUT error (failed removals keep the route running, so they must keep their targets); `aborted = true` on the early `Ok(Vec::new())` returns (discovery failure at reload_watcher.rs:229-236, runtime-snapshot failure at :238-245). Per-action errors are NOT an abort — `derive_rebuilt_route_ids` already yields the successfully rebuilt subset, and failed removals are simply absent from `removed`, so partial-success passes still publish. Call the new parameter `publish_script_sources: &(dyn Fn(&[String], &[String]) + Send + Sync)` with `(&rebuilt, &removed)` exactly once per pass, ONLY when `!aborted` — a no-change pass (`actions.is_empty()`) publishes with empty slices, which is a no-op under the Task 4.1 registry rules (promotion of nothing, deletion of nothing).
4. Thread the new hook and the kind-tagged provider through every caller — KEEP CORE GENERIC: `watch_and_reload` (reload_watcher.rs:93), the `ReloadWatcher` wrapper, and the barrier entry point already accept generic `D: Fn() -> … + Send + Sync + 'static` providers (reload_watcher.rs:130-143) and MUST STAY GENERIC — both integration providers capture variables (`dev_watch_template_test.rs:156-162`, `dev_watch_proto_test.rs:193-198`) and closures do not coerce to `fn` pointers. Update the generic provider return type to `Vec<(String, Vec<(PathBuf, DevSourceKind)>)>` and the publish hook to `&(dyn Fn(&[String], &[String]) + Send + Sync)`. Function-pointer specialization stays ONLY in the CLI adapter: run.rs's `watch_reload_with_dev_sources` `fn`-pointer type becomes the kind-tagged `fn() -> Vec<(String, Vec<(PathBuf, DevSourceKind)>)>` (plain fns coerce). camel-cli wraps `dev_target_sources()` output as `DevSourceKind::Template` and passes a no-op publish hook for now (Task 4.1 replaces both); the camel-test integration providers convert their returned paths to `DevSourceKind::Template` and pass the no-op hook.
5. Update the existing watcher tests that construct `DevTargetMap::from_sources` for the new signature, and the three camel-test integration files above (no-op publish hook — the integration scenarios do not exercise script sources).

**Tests:** (command: `cargo test -p camel-core --lib hot_reload`)
- `classify_script_path_yields_script_source_class`: dev map containing `(route, scripts/enrich.rhai, Script)` → `classify_event` on a modify event for that path returns `EventClass::ScriptSource`.
- `script_event_triggers_full_reload_pass`: capturing reload pass + dispatch fns (reuse the existing harness at reload_watcher.rs:1903 `route_file_event` pattern) → a script-only event batch runs the reload pass and dispatches zero `ReloadTemplates`.
- `template_event_still_dispatches_reload_templates`: template-kind target event → dispatch path unchanged (no full pass).
- `script_and_route_edit_in_one_batch_single_pass`: batch containing one script event and one route-file event inside the debounce window → exactly one `reload_pass` invocation.
- `publish_hook_called_with_outcome_after_pass`: capturing publish hook → called exactly once per non-aborted pass with `(rebuilt, removed)`; a pass whose discovery fails (aborted) NEVER calls the hook; a successful `ReloadAction::Remove` appears in `removed`, a failed one does not.

**Acceptance:**
- `cargo test -p camel-core --lib` exits 0 (existing watcher tests pass under the new signature).
- `cargo build --workspace` exits 0 (cli adaptation compiles).
- `cargo clippy -p camel-core -p camel-cli -p camel-test --all-targets -- -D warnings` exits 0 (the camel-test integration call sites compile — `cargo build --workspace` does not build test targets).
- `cargo fmt --check --all` exits 0.

- [x] 3.1

## camel-dsl + camel-cli

### Task 4.1: ScriptWatchRegistry, discovery staging, CLI merge, and the watch-set load test

**Files:**
- `crates/camel-dsl/src/script_watch.rs` (new)
- `crates/camel-dsl/src/lib.rs` (modified)
- `crates/camel-dsl/src/yaml.rs` (modified — `register_direct` calls from `load_from_file`/`load_from_file_with_env`)
- `crates/camel-dsl/src/json.rs` (modified — `register_direct` call from `load_json_from_file`)
- `crates/camel-dsl/src/discovery.rs` (modified — `begin_discovery_pass` + per-route staging)
- `crates/camel-cli/src/commands/run.rs` (modified)
- `crates/camel-test/tests/dev_watch_script_test.rs` (new — runtime-backed watch coverage, modeled on `crates/camel-test/tests/dev_watch_template_test.rs`)

**Steps:**
1. Create `script_watch.rs` with `ScriptWatchRegistry`: internally `published: BTreeMap<String, BTreeSet<PathBuf>>` and `staged: BTreeMap<String, BTreeSet<PathBuf>>`. All behavior lives on instance methods taking `&self` (the project's fresh-instance test pattern — `template_reload.rs:553-557` `fresh_registry()` via `Box::leak` — exists precisely to avoid global-singleton test races): `fn stage(&self, route_id: &str, paths: BTreeSet<PathBuf>)` plus `fn begin_discovery_pass(&self)` (clears `staged` at the start of each discovery run); `fn register_direct(&self, route_id: &str, paths: BTreeSet<PathBuf>)` (for `load_from_file`/`load_json_from_file`, writes to `published` immediately); `fn publish_commit(&self, rebuilt: &[String], removed: &[String])` — for every id in `rebuilt`, promote `staged[id]` into `published`; for every id in `removed`, delete its `published` entry; ids in NEITHER list keep their published entries unchanged (failed/skipped retention; a route whose Remove action failed is still running and stays watched). Staging absence NEVER deletes — only explicitly committed removals do, so an aborted or failed discovery pass cannot silently unwatch running routes. `fn route_watch_sources(&self) -> BTreeMap<String, BTreeSet<PathBuf>>` snapshot of `published`. `fn global() -> &'static Self` is a thin `OnceLock`/lazily-initialized wrapper (match `TemplateReloadRegistry`'s global style) used only by production wiring; tests use `Box::leak`-ed fresh instances.
2. In `discovery.rs`: call `begin_discovery_pass()` at the start of each discovery run and stage each discovered route's recorded script paths (from Task 2.1's sink) under the route id during route accumulation.
3. In `yaml.rs` `load_from_file`/`load_from_file_with_env` and `json.rs` `load_json_from_file`: call `register_direct` on the global registry with the parsed route ids and their resolved script paths.
4. In `lib.rs`: `pub use script_watch::ScriptWatchRegistry;`.
5. In run.rs: `dev_target_sources()` merges `TemplateReloadRegistry::global().route_watch_sources()` (kind `Template`) with `ScriptWatchRegistry::global().route_watch_sources()` (kind `Script`); the production watch entry passes `ScriptWatchRegistry::global().publish_commit` as the publish hook (adapting the no-op from Task 3.1). CRITICAL — startup publication: after the initial discovery+deploy in the watch path (run.rs, before `watch_reload_with_dev_sources` is awaited at run.rs:605), call `ScriptWatchRegistry::global().publish_commit(&all_deployed_route_ids, &[])` — startup route loading is fail-closed (run.rs:548), so every staged dependency belongs to a successfully added route. Without this the published set is empty until some route-file edit triggers a pass, and script edits never reload — the headline feature dead on arrival.
6. Create `crates/camel-test/tests/dev_watch_script_test.rs` driving the REAL watcher loop (real `ReloadWatcher`, real debounce, tempdir project) so the blessed cli-dev-watch runtime scenarios have executable coverage: (a) script-only edit → reload pass runs and the route swaps to the rebuilt generation; (b) edit then restore identical bytes within one debounce window → no swap (the folded hash equals the committed hash, route skipped); (c) partial success — two routes, edit route B's script (swap expected) while route A's document is edited to reference an unknown language (compile fails at route build) → route A keeps its previously published script dependency while route B publishes its new one. Assert swaps via the same runtime-projection/controller observations the template dev-watch test uses, and registry state via `route_watch_sources()`.

**Tests:** (commands: `cargo test -p camel-dsl --lib script_watch` and `cargo test -p camel-cli --lib`)
- `load_test_watch_set_contains_resolved_script_path`: tempdir project with `routes/doc.yaml` referencing `../scripts/enrich.rhai` (script exists at project root) → run `camel_dsl::load_from_file` on the route file (this is the `register_direct` path — discovery alone only stages) → `ScriptWatchRegistry::global().route_watch_sources()` contains the canonical resolved script path under a route id unique to this test (unique ids + unique tempdirs avoid cross-test pollution with the global registry).
- `publish_commit_promotes_staged_paths`: fresh registry; stage `a.rhai` for route `r1`; `publish_commit(&["r1"], &[])` → `route_watch_sources()` maps `r1` to `a.rhai`.
- `publish_commit_keeps_failed_route_paths`: fresh registry; published `old.rhai` for `r1`; stage `new.rhai` for `r1`; `publish_commit(&[], &[])` (route neither rebuilt nor removed) → `r1` still maps to `old.rhai`.
- `staging_absence_never_deletes`: fresh registry; published `x.rhai` for `r1`; `begin_discovery_pass()` then stage nothing; `publish_commit(&[], &[])` → `r1` still maps to `x.rhai` (deletion requires an explicitly committed removal, never staging absence).
- `publish_commit_drops_removed_routes`: fresh registry; published `x.rhai` for `r1`; `publish_commit(&[], &["r1"])` → `r1` gone from `route_watch_sources()`.
- `changed_script_path_publishes_new_target`: fresh registry; published `old.rhai`; stage `new.rhai`; `publish_commit(&["r1"], &[])` → `r1` maps to `new.rhai` only.
- `startup_publish_seeds_watch_sources` (camel-cli test): after the startup publish call wired in step 5 (extract the startup publication into a testable fn in run.rs if the watch path is not directly testable) → `route_watch_sources()` is non-empty for a loaded project with a `source_file` reference.
- `script_edit_swaps_route_and_restored_content_skips` (dev_watch_script_test.rs, scenario a+b): real watcher loop → script-only edit swaps the route; identical-content restore inside one debounce window does not.
- `partial_success_keeps_failed_route_script_dep` (dev_watch_script_test.rs, scenario c): route A compile-fails after edit, route B swaps → A's published script dependency retained, B's new dependency published.

**Acceptance:**
- Both unit-test commands exit 0 AND `cargo test -p camel-test --test dev_watch_script_test` exits 0.
- `cargo clippy -p camel-dsl -p camel-cli -p camel-test --all-targets -- -D warnings` exits 0.
- `cargo fmt --check --all` exits 0.

- [x] 4.1

## camel-dsl

### Task 5.1: Template materialization seam for source_file

**Files:**
- `crates/camel-dsl/src/template/materializer.rs` (modified)
- `crates/camel-dsl/src/discovery.rs` (modified)

**Steps:**
1. Thread template document origins into materialization WITHOUT touching the camel-api template type (`RouteTemplateSpec` at camel-api/src/template.rs:35-42 has no origin field and is public): `discovery.rs` accumulates a parallel `template_origins: HashMap<String, PathBuf>` (template id → originating document's parent dir, populated wherever templates are parsed in `parse_document_routes` using the `base_dir` from Task 2.1) alongside the existing `templates: HashMap<String, RouteTemplateSpec>` accumulation, and passes it into `materialize_templated_routes` (discovery.rs:640-649) as a new parameter.
2. In `template/materializer.rs`: extend the materialization entry point to accept `template_origins` and a `&ScriptLoadContext` (Task 2.1's type) — the generated route JSON is parsed as today (materializer.rs:244-266), but the lowering of expressions inside materialized routes now runs with `base_dir = template_origins[template_id]`, so `source_file` (including `{{param}}`-substituted paths — substitution precedes parsing, so resolution sees the substituted value) resolves against the TEMPLATE document's directory. Keep a compatibility wrapper for existing public callers that passes an empty origins map with `base_dir: None` if any caller cannot provide origins (grep the callers; list them in the commit body if wrappers are needed).
3. Register resolved script paths as instance dependencies: stage them under the materialized instance's route id via Task 4.1's `ScriptWatchRegistry::stage`, and fold their contents into the instance hash at the instance-hash seam (discovery.rs:891-895) so a script-only edit changes the instance's change-detection hash.

**Tests:** (command: `cargo test -p camel-dsl --lib`)
- `template_source_file_resolves_against_template_dir_after_substitution`: `templates/payment.yaml` defines a route template whose steps contain `script: { source_file: "{{script_name}}" }`; `routes/orders.yaml` instantiates it with `script_name: fraud-check.rhai`; `templates/fraud-check.rhai` exists → the loaded instance's expression compiles with language inferred from `.rhai` and the file's contents (proving template-dir resolution after substitution), AND the resolved script path is registered (staged) under the instance's route id in `ScriptWatchRegistry` (assert via a fresh registry injection or the global registry with a route id unique to the test).
- `instance_hash_changes_on_script_only_edit`: with the same fixture, discover, edit only `templates/fraud-check.rhai`, discover again → the instance route's `source_hash` differs.

**Acceptance:**
- `cargo test -p camel-dsl --lib` exits 0.
- `cargo clippy -p camel-dsl --all-targets -- -D warnings` exits 0.
- `cargo fmt --check --all` exits 0.

- [x] 5.1

## Contract

### Task 6.1: Schema regeneration and YAML↔JSON parity coverage

**Files:**
- `schemas/dsl/route-schema.json` (regenerated)
- `schemas/ts/ScriptData.ts` (regenerated)
- `schemas/ts/PredicateBlock.ts` (regenerated)
- `schemas/ts/LoopWhileExpr.ts` (regenerated)
- `schemas/ts/SetHeaderData.ts` (regenerated)
- `schemas/ts/SetPropertyData.ts` (regenerated)
- `schemas/ts/SetBodyConfig.ts` (regenerated)
- `schemas/ts/LogMessageExpr.ts` (regenerated)
- `schemas/ts/SplitExpressionConfig.ts` (regenerated)
- `schemas/ts/DynamicRouterData.ts` (regenerated)
- `schemas/ts/RoutingSlipData.ts` (regenerated)
- `schemas/ts/RecipientListData.ts` (regenerated)
- `crates/camel-lint/schema/route-schema.json` (synced by xtask)
- `crates/camel-dsl/src/parity_tests.rs` (modified)

**Steps:**
1. Run `cargo xtask schema` from the worktree root to regenerate the DSL schema, TS types, and the camel-lint schema sync. `schemas/canonical-route-spec.json` must come out unchanged (`LanguageExpressionDef` is untouched).
2. Extend the parity harness: add optional fixture support to `ParityCase` (parity_tests.rs:35) — `fixtures: &'static [(&'static str, &'static str)]` written into the case tempdir before parsing, and a `{scripts_dir}` placeholder substituted with the tempdir's absolute path in both `yaml` and `json` before parse.
3. Add parity case `ScriptSourceFile`: script step with `source_file: "{scripts_dir}/enrich.rhai"` (absolute path, language inferred), identical fixture content for both formats; assert the parsed ASTs are equal.
4. Add parity case `ScriptSourceFileFilter`: filter predicate with `source_file` (explicit `language`), covering the predicate path in both formats.

**Tests:** (commands: `cargo test -p camel-dsl --lib parity` and `cargo xtask schema --check`)
- `script_source_file_parity`: the two new cases pass — YAML and JSON parse to equal ASTs for both `source_file` forms.
- schema check: `cargo xtask schema --check` exits 0 with no drift.

**Acceptance:**
- `cargo xtask schema --check` exits 0.
- `cargo test -p camel-dsl --lib` exits 0.

- [x] 6.1

## camel-cli

### Task 7.1: Basic template demonstrates source_file

**Files:**
- `crates/camel-cli/templates/basic/scripts/hello.rhai` (new)
- `crates/camel-cli/templates/basic/routes/hello.yaml` (modified)
- `crates/camel-cli/templates/basic/routes/hello.test.yaml` (modified)
- `crates/camel-cli/tests/new_test.rs` (modified — extend the existing scaffold tests; `new_test.rs::scaffolded_test_doc_passes` at :312 is the end-to-end harness)

**Steps:**
1. Add `templates/basic/scripts/hello.rhai`: a small rhai script that sets a header `greeting` to a fixed string (verify the exact rhai processor API against an existing rhai route test in the repo before writing it).
2. In `templates/basic/routes/hello.yaml`: add a `- script: { source_file: "../scripts/hello.rhai" }` step (no `language:` key — inference) after the existing `set_header` step and before `to: "log:done"`. Keep the existing comments intact.
3. In `templates/basic/routes/hello.test.yaml`: extend the expectations to assert the `greeting` header produced by the script, keeping the existing `source` header expectation.
4. Extend `crates/camel-cli/tests/new_test.rs` (the scaffold tests — `scaffolded_test_doc_passes` at :312 runs the full new→test flow) to assert the emitted project contains `scripts/hello.rhai` (non-empty) and `routes/hello.yaml` contains `source_file: ../scripts/hello.rhai` with no `language:` key; keep the end-to-end scaffold test green — it now exercises the script step through the real `camel test` flow.
5. Confirm the template file copier (`crates/camel-cli/src/template/embedded.rs` `collect_files`) copies the new `scripts/` subdirectory without special-casing.

**Tests:** (command: `cargo test -p camel-cli`)
- `new_template_emits_script_file`: scaffold into a tempdir → `scripts/hello.rhai` exists, non-empty; `routes/hello.yaml` contains `source_file: ../scripts/hello.rhai` and no `language:` on that step.
- `scaffolded_route_loads_script_from_file`: `camel_dsl::load_from_file` on the scaffolded `routes/hello.yaml` → Ok, and the compiled script step's source equals the script file contents.
- Existing scaffold tests (route discovery, reserved test suffix, test-document flow) keep passing with the added script step.

**Acceptance:**
- `cargo test -p camel-cli` exits 0.
- `cargo clippy -p camel-cli -- -D warnings` exits 0 (both default and the `--no-default-features --features flavor-regular,exec` variant if template code is feature-gated).
- `cargo fmt --check --all` exits 0.

- [x] 7.1

## docs

### Task 8.1: Script guide gains the file form

**Files:**
- `docs/src/eip/script.md` (modified)
- `docs/src/yaml-dsl/step-verbs/index.md` (modified)
- `docs/src/languages/rhai.md` (modified)
- `docs/src/languages/js.md` (modified)
- `docs/src/languages/minijinja.md` (modified)

**Steps:**
1. In `docs/src/eip/script.md`, add a "Scripts from files" section after the inline form: the `source_file:` field, the extension inference map (`.rhai`, `.js`, `.jsonpath`, `.xpath`, `.jinja`/`.jinja2` → `minijinja`), the resolution rule (relative to the referencing route file; absolute allowed; Rust DSL constructors resolve against the process CWD), the verbatim-read rule (no env/templating inside script files), and the failure modes (missing file, empty file, both fields set, unknown extension without `language:` — all named errors including the resolved path). Add one sentence on IDE support: a real extension gives highlighting, lint, format, and go-to-definition. Tag all non-Rust fences (`yaml`, `text`) so `mdbook test` treats them as non-Rust.
2. In `docs/src/yaml-dsl/step-verbs/index.md`: extend the expression-source list to include `source_file`.
3. In `docs/src/languages/rhai.md` and `docs/src/languages/js.md`: add a short file-form example using `source_file` with the respective extension.
4. In `docs/src/languages/minijinja.md`: add a short file-form note (`.jinja`/`.jinja2` → `minijinja` inference) consistent with the byte-identical passthrough rule.

**Tests:** (command: `cd docs && mdbook test`)
- mdbook test compiles every Rust fence and passes; the new sections use `yaml`/`text` fences only.

**Acceptance:**
- `cd docs && mdbook test` exits 0.
- No untagged code fences added (grep the diff for bare ``` fences).

- [x] 8.1
