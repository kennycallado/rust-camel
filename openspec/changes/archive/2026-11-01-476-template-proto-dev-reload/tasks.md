# Tasks: 476-template-proto-dev-reload

## Phase 1: Watch-source exposure

### camel-component-api

#### Task 1.1: `watch_sources` trait method and registry enumeration

**Files:**
- `crates/components/camel-component-api/src/template_reload.rs` (modified)

**Steps:**
1. Add trait method `fn watch_sources(&self) -> Vec<PathBuf>` to `TemplateReloadTarget` with a doc comment stating the contract: unique absolute FILE paths whose change should reload the target; committed state only; no directories.
2. Add `use std::path::PathBuf;` to the file's imports.
3. Add `pub fn route_watch_sources(&self) -> Vec<(String, Vec<PathBuf>)>` to `TemplateReloadRegistry`: for every registered target, collect `(route_id.clone(), target.watch_sources())`, one entry per route id (merge sources of multiple targets on the same route, deduplicate and sort paths), sorted by route id.
4. Register no behavior change for existing targets — `ReloadHandler` (camel-template) implements the method in Task 1.2; any in-crate test fakes add `watch_sources` returning `Vec::new()`.

**Tests:**
- `route_watch_sources_merges_and_dedups_per_route`: two fakes registered on route "r1" reporting overlapping paths and one fake on "r2" → call `route_watch_sources()` → assert exactly two entries; "r1" entry contains each path once, sorted; "r2" entry matches its fake.
- `route_watch_sources_empty_when_no_targets`: fresh `TemplateReloadRegistry` (not the global) → assert `route_watch_sources()` returns an empty vec.
- `route_watch_sources_excludes_unregistered`: register a fake, drop its `RegistrationGuard` → assert its route no longer appears.

**Acceptance:**
- `cargo test -p camel-component-api --lib` exits 0.
- `cargo clippy -p camel-component-api -- -D warnings` exits 0.
- `cargo fmt --check` clean for the touched file.

- [x] 1.1

### camel-template

#### Task 1.2: ReloadHandler exposes the committed closure as watch sources

**Files:**
- `crates/components/camel-template/src/reload.rs` (modified)
- `crates/components/camel-template/src/lifecycle.rs` (modified — wherever the startup build constructs `ReloadHandler`; locate the construction site with `rg -n "ReloadHandler" crates/components/camel-template/src/`)

**Steps:**
1. Add fields to `ReloadHandler`: `root_path: PathBuf` (the configured root path, resolved at construction where the root `OwnedHandle` is opened) and `committed_sources: std::sync::Mutex<Vec<PathBuf>>` (the last committed closure's unique absolute file paths).
2. Initialize `committed_sources` from the startup build's `ClosureSnapshot` entries (root_path joined with each entry name, deduplicated) so sources exist before the first reload.
3. Update `ReloadHandler::commit` to also store the staged closure's file list into `committed_sources` (the staged set must carry the closure entry names; if `StagedSet` lacks them, extend `ReloadHandler::build` to return them alongside `StagedSet` and thread them through `commit`).
4. Implement `watch_sources(&self) -> Vec<PathBuf>` returning `committed_sources.lock().clone()` — unique absolute file paths, entry and includes included, root directory excluded.
5. Do not modify build/validate/commit failure semantics: a failed build leaves `committed_sources` untouched.

**Tests:**
- `watch_sources_reports_entry_and_nested_includes_not_root`: build a root dir with `page.jinja` including `part/head.jinja` which includes `part/logo.jinja`; register the handler for route "t1" → assert `TemplateReloadRegistry::route_watch_sources()` entry for "t1" is exactly `[<root>/page.jinja, <root>/part/head.jinja, <root>/part/logo.jinja]` sorted, and does NOT contain the root directory path itself.
- `watch_sources_refresh_after_closure_grows`: after a successful `reload_route` that commits a template whose closure adds `part/foot.jinja` → assert the new path appears in `watch_sources()` without re-registration.
- `watch_sources_unchanged_after_failed_reload`: commit a valid set; edit the entry to invalid syntax; run `reload_route` (it fails) → assert `watch_sources()` still returns the previously committed closure only.

**Acceptance:**
- `cargo test -p camel-template --lib reload` exits 0 with the three new tests passing.
- `cargo test -p camel-template --test template_reload_integration` exits 0 (existing suite unbroken).
- `cargo clippy -p camel-template --all-targets -- -D warnings` exits 0.

- [x] 1.2

## Phase 2: Dev-watch watcher

### camel-core

#### Task 2.1: Dev-target map and event classification

**Files:**
- `crates/camel-core/src/hot_reload/adapters/reload_watcher.rs` (modified)

**Steps:**
1. Add `use std::collections::{HashMap, HashSet};` (extend the existing `HashSet` import) and `pub(crate) enum EventClass { RouteFile, DevTarget }`.
2. Add `pub(crate) struct DevTargetMap { path_to_routes: HashMap<PathBuf, HashSet<String>> }` with: `pub(crate) fn from_sources(sources: &[(String, Vec<PathBuf>)]) -> Self` (insert path→route edges), `pub(crate) fn contains_path(&self, p: &Path) -> bool`, `pub(crate) fn routes_for(&self, p: &Path) -> HashSet<String>` (empty set when absent), and `pub(crate) fn parent_dirs(&self) -> Vec<PathBuf>` (unique parent dirs of all tracked paths; subtraction of already-subscribed dirs is the WRAPPER's job, not this method's), and `pub(crate) fn is_empty(&self) -> bool`.
3. Add `fn classify_event(event: &Event, dev_map: &DevTargetMap, route_roots: &[PathBuf]) -> EventClass`: a `Create/Modify/Remove` event whose paths include any path in the dev map returns `DevTarget` (dev-target precedence); otherwise a `Create/Modify/Remove` event on a yaml/yml/json path UNDER one of the configured recursive route roots returns `RouteFile`; yaml/yml/json events in asset-only parent dirs (outside `route_roots`) match NEITHER class and are ignored. Keep `is_reload_event` unchanged for its existing internal callers; the new classification replaces its use in the extracted loop only.
4. Keep `is_reload_event` unchanged for route-file semantics.

**Tests:**
- `classify_dev_target_path_takes_precedence_over_extension`: dev map contains `routes/defs.yaml`; event Modify on that path → assert `EventClass::DevTarget` even though the extension is yaml.
- `classify_route_file_when_not_tracked`: event Create on `routes/other.yaml` with an empty dev map and `route_roots = ["routes"]` → assert `EventClass::RouteFile`.
- `unrelated_yaml_outside_route_roots_is_ignored`: event Create on `assets/data.yaml` with an empty dev map and `route_roots = ["routes"]` → assert neither `RouteFile` nor `DevTarget` (classification returns an ignore outcome; assert `reload_pass` and `dispatch_reload` are both uncalled in the loop test).
- `dev_map_limited_to_declared_sources`: `from_sources(&[("t1", vec![a,b,c]), ("v1", vec![schema, import])])` → assert `contains_path` true for exactly those five paths and `routes_for(schema) == {"v1"}`; a grpc proto path never inserted because it was never declared.
- `dev_map_parent_dirs_unique`: sources spanning two files in one dir and one file elsewhere → assert `parent_dirs()` yields exactly two unique dirs (no duplicates).

**Acceptance:**
- `cargo test -p camel-core --lib reload_watcher` exits 0.
- `cargo clippy -p camel-core -- -D warnings` exits 0.

- [x] 2.1

#### Task 2.2: Extract testable watch loop with synthetic event source

**Files:**
- `crates/camel-core/src/hot_reload/adapters/reload_watcher.rs` (modified)
- `crates/camel-core/Cargo.toml` (modified — add `tokio` dev-dependency feature `test-util` under `[dev-dependencies]` if absent, mirroring how other workspace crates declare it)
- `crates/camel-core/tests/template_reload_test.rs` (modified — the existing `TemplateReloadTarget` fake gains `watch_sources` returning `Vec::new()` so the crate's integration tests compile against the extended trait)

**Steps:**
1. Extract the body of `watch_and_reload`'s event loop into `pub(crate) async fn run_reload_loop(event_rx: tokio::sync::mpsc::Receiver<notify::Result<Event>>, route_roots: Vec<PathBuf>, controller: RuntimeExecutionHandle, dev_provider: &(dyn Fn() -> DevTargetMap + Send + Sync), reload_pass: &(dyn Fn() -> futures::future::BoxFuture<'static, Result<Vec<String>, CamelError>> + Send + Sync), dispatch_reload: &(dyn Fn(&str) -> futures::future::BoxFuture<'static, Result<(), CamelError>> + Send + Sync), parents_tx: tokio::sync::watch::Sender<Vec<PathBuf>>, shutdown: Option<CancellationToken>, debounce: Duration) -> Result<(), CamelError>`. Both callbacks return boxed futures; the loop awaits them (never calls them synchronously); `dev_provider` re-derives the map from the registry (production closure calls `TemplateReloadRegistry::global().route_watch_sources()` then `DevTargetMap::from_sources`); `reload_pass` runs the existing route discovery+reload actions and returns the rebuilt route ids (empty vec on the early-continue paths); `dispatch_reload` performs the reload dispatch (see step 3 for the shared bound).
2. Extract a shared bounded-dispatch helper `pub(crate) async fn bounded_dispatch<F>(fut: F, bound: Duration, shutdown: Option<&CancellationToken>) -> Result<(), CamelError> where F: std::future::Future<Output = Result<(), CamelError>>`: wraps the dispatch future in `tokio::time::timeout(bound, fut)`, `select!`-biased with the shutdown token, and returns `Err(TemplateReload("dispatch bound elapsed"))` on timeout/cancel — the future is DROPPED on either path (no late commit). The production `dispatch_reload` closure in the wrapper uses this helper with bound = route registry deadline plus the existing drain timeout; the loop tests inject UNBOUNDED parked callbacks THROUGH the same helper so the tests prove the production bounding behavior, not test-local bounds.
3. Change `watch_and_reload`'s public signature to `pub async fn watch_and_reload<F, D>(watch_dirs: Vec<PathBuf>, controller: RuntimeExecutionHandle, discover_fn: F, dev_provider: D, shutdown: Option<CancellationToken>, drain_timeout: Duration, debounce: Duration) -> Result<(), CamelError>` where `F: Fn() -> Result<Vec<RouteDefinition>, CamelError> + Send + Sync + 'static` (`Sync` added: the wrapper captures `discover_fn` in the shared `&dyn Fn() -> BoxFuture + Send + Sync` `reload_pass` closure, so non-`Sync` F cannot compile) and `D: Fn() -> Vec<(String, Vec<PathBuf>)> + Send + Sync + 'static` (the registry read; synchronous). The wrapper: builds the notify watcher, forwards events into a channel, subscribes route dirs (recursive) plus `dev_provider`-derived parent dirs (NonRecursive, after subtracting already-subscribed route dirs), constructs the `reload_pass` and `dispatch_reload` async closures around `discover_fn`, `controller`, and `bounded_dispatch`, creates the `watch::channel<Vec<PathBuf>>` for subscription reconciliation, and calls `run_reload_loop` (passing `watch_dirs` as `route_roots` for classification). Update the `ReloadWatcher::watch_and_reload` forwarding method and every existing call site in the same file (the forwarding impl and the module tests around line 466), plus the four `watch_and_reload` calls in `crates/camel-test/tests/hot_reload_integration.rs`, to the new signature so the workspace compiles; Task 2.5 updates the camel-cli call site.
4. Batch handling inside `run_reload_loop`: after the debounce window closes over a batch of classified events — (a) any `RouteFile` event: run `reload_pass()` first (its early-continue on discovery failure or empty actions is fine), then re-derive via `dev_provider()`, then dispatch pending dev-target routes NOT rebuilt by the pass; (b) dev-target-only: dispatch `dispatch_reload(route_id)` per affected route that has targets (query `dev_provider`-derived map), log-and-continue on error; then re-derive. Events classified as neither (unrelated yaml outside route roots) dispatch nothing and trigger no route pass.
5. Route-file path keeps existing behavior exactly: same logs, same reload actions, same error handling.
6. No `tokio::time::sleep` in tests; use `tokio::time::pause` for debounce windows (requires the `test-util` dev feature added above).

**Tests:**
- `dev_target_event_dispatches_per_route`: synthetic Modify event on a tracked template path (map has routes {"t1"}) → assert `dispatch_reload` called exactly once with "t1" and `reload_pass` NOT called.
- `debounce_coalesces_rapid_edits`: three Modify events on the same tracked path sent back-to-back with `tokio::time::pause` and debounce of 50ms advanced once → assert a single `dispatch_reload` call.
- `route_without_targets_skipped_and_loop_continues`: Modify event on a path mapped to no route (or a route whose map entry is empty) → assert no dispatch, loop processes a subsequent event normally.
- `dispatch_timeout_drops_without_late_commit`: `dispatch_reload` returns an UNBOUNDED parked future (channel that never answers); the loop's await goes through the production `bounded_dispatch` helper with `tokio::time::pause` advancing past the bound → assert the loop logs and continues; assert the parked future was dropped (drop guard flag set when the future is dropped) and a subsequent event still processes.
- `shutdown_responsive_during_dispatch`: cancel the token while `dispatch_reload` returns a parked future → assert `run_reload_loop` returns promptly (bounded by the debounce/bound under paused time) without waiting for the dispatch, via the same `bounded_dispatch` path.
- `route_file_event_keeps_existing_behavior`: Create event on `routes/x.yaml` with empty dev map and `route_roots = ["routes"]` → assert `reload_pass` called once and no dispatch.

**Acceptance:**
- `cargo test -p camel-core --lib reload_watcher` exits 0 including all new tests.
- `cargo test -p camel-core --test template_reload_test` exits 0 (fake migrated).
- `cargo test -p camel-test --test hot_reload_integration` exits 0 (call sites migrated).
- `cargo clippy -p camel-core -- -D warnings` exits 0.
- `cargo xtask lint-test-sleep` exits 0 (no sleeps introduced).
- `cargo xtask lint-unbounded-wait` exits 0.

- [x] 2.2

#### Task 2.3: Mixed-batch ordering — route reload first, surviving routes still dispatch

**Files:**
- `crates/camel-core/src/hot_reload/adapters/reload_watcher.rs` (modified)

**Steps:**
1. In `run_reload_loop`, track per-batch pending dev-target routes: collect `routes_for(path)` for every `DevTarget`-classified event in the batch.
2. When the batch contains any `RouteFile` event: after `reload_pass()` returns, re-derive the map via `dev_provider()`, then dispatch `dispatch_reload(route)` only for pending routes that still have targets in the refreshed map AND are not in the rebuilt-route-id list `reload_pass` returned. The rebuilt list contains ONLY successfully rebuilt route ids — `Skip` actions, errored actions, and routes whose rebuild failed are NOT rebuilt (their prior generation survives, so their assets still need dispatching). The production wiring derives the list from the executed reload actions in `compute_reload_actions_from_runtime_snapshot` / `execute_reload_actions`, filtering to successful rebuilds only.
3. If `reload_pass` errs (discovery failure), log the existing error and dispatch ALL pending dev-target routes that still have targets (no rebuild happened, so none were rebuilt).
4. A batch with a route-file event and a template event for the SAME route dispatches nothing extra for that route only when that route's rebuild SUCCEEDED.

**Tests:**
- `mixed_batch_dispatches_for_surviving_unrebuilt_route`: map has routes "A" (template path changed) and "B" (route file changed); `reload_pass` returns `Ok(vec!["B"])` → assert dispatch once for "A", none for "B".
- `mixed_batch_same_route_no_double_dispatch`: route-file event for "B" and template event for "B"; `reload_pass` returns `Ok(vec!["B"])` → assert no dispatch for "B".
- `partial_survival_skipped_and_failed_still_dispatch`: routes "A" (skip), "B" (rebuild failed), "C" (rebuilt Ok) all with pending template events; `reload_pass` returns `Ok(vec!["C"])` (skipped and failed ids excluded) → assert dispatch for "A" and "B", none for "C".
- `failed_discovery_still_dispatches_pending`: `reload_pass` returns `Err(CamelError::Config("discovery failed".into()))` → assert dispatch for every pending route still present in the refreshed map.
- `no_action_pass_still_dispatches`: `reload_pass` returns `Ok(vec![])` → assert dispatch for pending route "A".

**Acceptance:**
- `cargo test -p camel-core --lib reload_watcher` exits 0.
- `cargo clippy -p camel-core -- -D warnings` exits 0.

- [x] 2.3

#### Task 2.4: Re-derivation, subscription reconciliation, delete/recreate tolerance

**Files:**
- `crates/camel-core/src/hot_reload/adapters/reload_watcher.rs` (modified)

**Steps:**
1. In `watch_and_reload` (the notify-wiring wrapper), subscribe each `dev_provider().parent_dirs()` with `RecursiveMode::NonRecursive`, skipping dirs already subscribed as route dirs; keep a `HashSet<PathBuf>` of currently subscribed dev-parent dirs.
2. Add `fn reconcile_subscriptions(watcher: &mut RecommendedWatcher, subscribed: &mut HashSet<PathBuf>, wanted: Vec<PathBuf>)` that adds missing and (best-effort) drops removed parent subscriptions; call it after every re-derivation (initial, post-route-pass, post-committed-asset-reload).
3. `Remove` and `Create` kinds on tracked paths count as `DevTarget` events (extend `classify_event` if Task 2.1 did not already include `Create`/`Remove` for tracked paths — it did: `Create/Modify/Remove`).
4. In `run_reload_loop`, re-derive the map after every committed asset reload (successful `dispatch_reload`) and signal the wrapper to reconcile subscriptions: `run_reload_loop` takes a `tokio::sync::watch::Sender<Vec<PathBuf>>`; after each re-derivation it sends `dev_map.parent_dirs()`; the `watch_and_reload` wrapper holds the corresponding `watch::Receiver` and, in its own `select!` loop alongside the notify channel, applies `reconcile_subscriptions` whenever a new parent-dir list arrives.
5. After a failed route pass, re-derivation comes from the registry (surviving registrations) — no code change beyond calling `dev_provider()` again; assert via test.

**Tests:**
- `replace_then_edit_still_dispatches` (uses real notify + real fs, bounded): register a target source at a temp file; start `watch_and_reload` on its parent; WAIT for the subscription barrier (the helper below) before any write; replace the file via `std::fs::write` to a temp sibling + `std::fs::rename` over the target; then edit again → assert two dispatches total arrive, each within a bounded wait-for-condition helper (loop with `tokio::time::timeout` chunks, no sleep calls).
- `watcher_ready_barrier`: expose a `tokio::sync::watch::Sender<bool>` (or oneshot) from the wrapper that fires after the initial subscriptions AND the initial reconciliation are applied; all real-fs tests await it before writing files, so no edit can precede a subscription; the reconciliation receiver acknowledges applies by updating a `watch::Receiver<u64>` version counter the tests can await.
- `failed_route_pass_retains_surviving_targets`: register two fake targets "good" and "bad" in a fresh registry; provider reads that registry; simulate a route pass that fails → re-derived map still contains both routes' sources.
- `rederive_after_committed_reload_picks_up_new_closure_file`: fake target whose `watch_sources` changes after its `reload_route` commits a new list → after one successful dispatch, `dev_provider()` map contains the new path (routes_for(new_path) non-empty).
- `new_parent_subscribed_after_closure_growth` (real notify + real fs, bounded): fake target whose first commit reports a source under `watch_a/` and whose second commit (after a reload) reports a NEW source under a previously unwatched `watch_b/` directory → after the growth dispatch, await the reconciliation version bump, then edit the new source file → assert a further dispatch arrives within the bounded wait (proves `reconcile_subscriptions` actually subscribed the new parent, not just that the map grew).
- `route_pass_growth_subscribes_new_parent` (real notify + real fs, bounded): start with a closure source under `watch_a/`; trigger a ROUTE-file reload pass after which the fake's committed sources grow into `watch_b/` (the fake updates its list when the pass callback runs) → await the reconciliation bump, edit the new source under `watch_b/` → assert a dev-target dispatch arrives (route-pass re-derivation path also reconciles subscriptions).

**Acceptance:**
- `cargo test -p camel-core --lib reload_watcher` exits 0.
- `cargo clippy -p camel-core -- -D warnings` exits 0.
- `cargo xtask lint-test-sleep` and `cargo xtask lint-unbounded-wait` exit 0.

- [x] 2.4

### camel-cli

#### Task 2.5: Wire the dev-target provider into `camel run --watch`

**Files:**
- `crates/camel-cli/src/commands/run.rs` (modified)

**Steps:**
1. At the existing watcher spawn site (`if let Some(watch) = spec.watch`), build the production `dev_provider` closure: reads `camel_component_api::template_reload::TemplateReloadRegistry::global().route_watch_sources()` (camel-cli already depends on crates that re-export it; if not directly reachable, add the dependency-free call through `camel_core` re-exports — check `crates/camel-cli/Cargo.toml` for `camel-component-api`; add it only if absent).
2. Update the single existing `watch_and_reload` call site to the final signature from Task 2.2 (add the `dev_provider` argument; the dispatch wiring lives inside the wrapper — the CLI call site passes only the provider; `ctrl` is already passed as `controller` unchanged).
3. Keep the `WatchSpec` gate unchanged: no watcher is built when `spec.watch` is `None`; non-watch and compiled-artifact paths are untouched.
4. Update the "hot-reload watching" info log to distinguish route dirs from asset parents at debug level only — no log-policy changes elsewhere.

**Tests:**
- `run_watch_passes_dev_provider` (unit, in `run_tests.rs` or the run module's test file): construct the watcher-ingredient values as other run tests do (follow `crates/camel-cli/tests/run_watch_test_doc_test.rs` and existing run unit tests for the harness) → assert the dev provider closure returns registry contents (register a fake target first; assert its sources flow through).
- `dev_provider_ignores_route_definitions` (unit): a route set containing a gRPC endpoint with a local `protoFile=` URI present in discovery output, with only the fake target registered in the registry → assert the provider's map tracks exactly the fake's sources and nothing parsed from URIs (gRPC references create no targets).
- Existing watch behavior: `cargo test -p camel-cli --test run_watch_test_doc_test` exits 0.
- Non-watch isolation: `cargo test -p camel-cli --test compiled_artifact_test` exits 0 (compiled artifacts never watch — existing suite guards this; no regression).

**Acceptance:**
- `cargo test -p camel-cli --lib commands::run` exits 0.
- `cargo clippy -p camel-cli -- -D warnings` exits 0.
- `cargo clippy -p camel-cli --no-default-features --features flavor-regular,exec --all-targets -- -D warnings` exits 0.

- [x] 2.5

## Phase 3: Validator proto reload target

### camel-proto-compiler

#### Task 3.1: Public local import-closure helper

**Files:**
- `crates/services/camel-proto-compiler/src/lib.rs` (modified)
- `crates/services/camel-proto-compiler/src/cache.rs` (modified — reuse/extract from the private `source_cache_key` logic)

**Steps:**
1. Note the actual cache shape before extracting: `source_cache_key` FINGERPRINTS an already-acquired source snapshot (it hashes file contents); it does NOT resolve imports, and cache compilation supports an ordered include-path list. Therefore add `pub fn local_import_closure(proto_path: &std::path::Path) -> Result<Vec<std::path::PathBuf>, ProtoCompileError>` as a THIN WRAPPER over the crate's internal snapshot/enumeration machinery: acquire the source snapshot for `proto_path` the same way `ProtoCache::get_or_compile` does (parse the root file, walk `import` statements transitively, resolve against the schema's parent dir first and the crate's include search rules second, mirroring compile-time resolution), then enumerate the LOCAL file paths in that snapshot (absolute, sorted, unique, excluding the root schema itself, excluding well-known/google built-ins and embedded inputs).
2. Share the snapshot-enumeration logic with the cache: the cache's keying path and the new helper call the SAME internal function (e.g. `fn enumerate_local_sources(root: &Path) -> Result<(Snapshot, Vec<PathBuf>), ProtoCompileError>`) so the closure semantics have one owner; do not duplicate the parser walk.
3. Unresolvable imports return the same error variant `compile_proto` returns for missing includes. Include-shadowing semantics must match the cache's existing behavior — the include-shadow tests (`a shadowing include change must invalidate the cached pool`) must stay green unchanged.
4. Keep `compile_proto`'s ordered-include-path signature untouched; the public helper is path-only.

**Tests:**
- `local_import_closure_transitive`: schema A imports B, B imports C (all sibling files in a temp dir) → assert returns `[B, C]` sorted, A itself excluded.
- `local_import_closure_excludes_well_known`: schema importing `google/protobuf/empty.proto` (protox resolves well-knowns internally) → assert the returned closure contains only local files.
- `local_import_closure_missing_import_errs`: schema importing a non-existent local file → assert the same error variant as `compile_proto` on a missing include.
- `local_import_closure_matches_cache_invalidation_set`: for a schema with local imports (including a shadowed include case mirroring the cache test fixture), assert the helper's returned set equals the set of files whose mutation invalidates the cache entry (drive `get_or_compile` twice, mutating each file in the returned set, asserting a fresh compile each time).

**Acceptance:**
- `cargo test -p camel-proto-compiler --lib` exits 0 (existing cache tests still green after the extraction).
- `cargo clippy -p camel-proto-compiler --all-targets -- -D warnings` exits 0.

- [x] 3.1

### camel-validator

#### Task 3.2: Protobuf descriptor hot reload target

**Files:**
- `crates/components/camel-component-validator/src/compiled.rs` (modified)
- `crates/components/camel-component-validator/src/component.rs` (modified — the endpoint-construction and producer site; locate with `rg -n "CompiledValidator::Protobuf|compile_protobuf" crates/components/camel-component-validator/src/`)
- `crates/components/camel-component-validator/Cargo.toml` (modified — add `arc-swap` and `camel-component-api` deps only if absent; add tokio dev-dependency feature `test-util` under `[dev-dependencies]` if absent)

**Steps:**
1. Introduce `pub(crate) struct ProtobufReloadState { descriptor: prost_reflect::MessageDescriptor, source_files: Vec<std::path::PathBuf> }` held in `Arc<arc_swap::ArcSwap<ProtobufReloadState>>`; `source_files` is the ABSOLUTE schema path (from `schema_path.canonicalize()` or the same absolute form camel-proto-compiler resolves) PLUS `local_import_closure(schema_path)` results — the schema itself must be tracked even though the closure helper excludes it. `CompiledValidator::Protobuf` reads the CURRENT state per validation call (swap in `load_full()` at the top of the protobuf validation arm, so a swapped descriptor is used by the next validation without endpoint rebuild).
2. At endpoint construction: when the route context exposes `route_id` as `Some`, build the initial `ProtobufReloadState` (descriptor + `local_import_closure` of the schema path) and register `pub(crate) struct ProtobufReloadTarget { route_id: String, state: Arc<ArcSwap<ProtobufReloadState>>, schema_path: PathBuf, generation: std::sync::atomic::AtomicU64 }` implementing `TemplateReloadTarget`: `route_id` returns the route; `reload_timeout` returns `Duration::from_millis(5000)` (matching camel-template's `reload_timeout_ms` default — verify the constant in `crates/components/camel-template/src/` and mirror it, citing it in a comment); `current_generation` reads the atomic; `build` runs `compile_proto` + `local_import_closure` inside `tokio::task::spawn_blocking` and returns a staged box (the new `ProtobufReloadState` with recomputed `source_files` = schema path + imports) plus the generation read before the build; `commit` bumps the generation and `store`s the new state; `watch_sources` returns `self.state.load_full().source_files.clone()`. When `route_id` is `None` (the `validator-bridge` fallback), skip registration with a `tracing::debug!` log.
3. Failed `build` (invalid proto, missing file) returns `Err` — the registry keeps the prior state; nothing is staged.
4. OWNERSHIP OF THE REGISTRATION GUARD (critical): the endpoint is dropped during route recompilation (`resolve_send` retains the producer and lifecycle handles, not the endpoint), so the `RegistrationGuard` must NOT live in the endpoint. Store the guard in the PRODUCER-side owner (the long-lived validator component/producer struct that survives endpoint re-creation), alongside the `Arc<ArcSwap<ProtobufReloadState>>`; when route recompilation creates a new endpoint for the same route, the new endpoint replaces the swap cell's CONTENTS (or the producer replaces its target registration) rather than leaking a second live registration — the producer drops the old guard when it registers the new target. Net contract: registration survives endpoint drop and disappears at producer teardown.

**Tests:**
- `descriptor_recompiles_on_reload`: endpoint with schema `syntax = "proto3"; message M { string a = 1; }`; validate a payload; rewrite schema to add field `int32 b = 2;`; run `TemplateReloadRegistry::global().reload_route(route)` → assert reload Ok and a payload using field `b` now validates (round-trips through the new descriptor).
- `import_only_edit_recompiles`: schema imports a local `common.proto` defining message `C`; endpoint validates `C` payloads; edit `common.proto` to add a field → `reload_route` → assert new field validates.
- `reload_grown_import_updates_sources`: schema initially has no imports; reload after editing schema to import `common.proto` → assert `watch_sources()` of the target now includes `common.proto`.
- `failed_recompile_keeps_prior_descriptor`: edit schema to invalid syntax → `reload_route` errs → assert validation with the OLD schema still passes (prior descriptor serving).
- `bridge_route_id_registers_nothing`: construct the endpoint with `route_id = None` (bridge path) → assert `TemplateReloadRegistry` (fresh local instance via the trait seam used by `template_reload_test.rs`, or assert via `find_all` on the global registry scoped to the fallback id) yields no target for `"validator-bridge"`.
- `registration_survives_endpoint_drop`: create endpoint + producer; drop the endpoint (simulate recompilation) → assert `find_all(route)` still returns the target and `reload_route` still works; drop the producer → assert the target is gone from `find_all(route)`.
- `stalled_build_times_out_without_commit`: a `TemplateReloadTarget` fake whose `build` awaits a releasable blocking gate — `Arc<std::sync::Mutex<std::sync::mpsc::Receiver<()>>>` (`Mutex` supplies `Sync` so the fake satisfies `Send + Sync`); inside `spawn_blocking` the build calls `gate.lock().recv()`, which blocks until the test sends on the sender — with `reload_timeout` of 50ms; run `reload_route` under `tokio::time::pause` advanced past the deadline → assert `Err(TemplateReload("reload timeout"))` and `current_generation` unchanged (no commit); then release the gate (send on the sender) and await the blocking handle so the test never leaves a parked worker behind.
- `broken_proto_blocks_same_route_template_reload`: register on ONE route the real `ProtobufReloadTarget` (schema edited to invalid syntax) plus a counting fake template target with a valid build → `reload_route` errs → assert BOTH priors keep serving: the fake's `commit` was never called (generation unchanged) and the validator still validates with the old schema.

**Acceptance:**
- `cargo test -p camel-component-validator` exits 0 (lib + existing tests).
- `cargo clippy -p camel-component-validator --all-targets -- -D warnings` exits 0.
- `cargo xtask lint-unbounded-wait` exits 0.

- [x] 3.2

## Phase 4: Integration + docs

### camel-test

#### Task 4.1: Dev-watch end-to-end integration test — template side

**Files:**
- `crates/camel-test/tests/dev_watch_template_test.rs` (new)
- `crates/camel-test/Cargo.toml` (modified — add `camel-template = { workspace = true }` as a dev-dependency (or regular dependency following the crate's existing convention) so the test crate can build template endpoints; check how `camel-component-validator` is declared there and follow the same optional/feature pattern, adding a dev-feature or dev-dependency entry as needed)

**Steps:**
1. Build a minimal route set (follow the harness patterns of an existing camel-test integration test that boots a context with `template:file:` endpoints — locate with `rg -ln "template:file:" crates/camel-test/tests/`; if none exists, follow the closest component integration test and the camel-template integration suite `crates/components/camel-template/tests/`) with a template root containing `page.jinja` including `part/head.jinja`.
2. Boot the context with the watch watcher started (mirroring how `camel run --watch` spawns `watch_and_reload`; if the test harness cannot use the CLI binary directly, drive `camel_core::reload_watcher::watch_and_reload` with the production closures as `run.rs` does).
3. Implement `async fn wait_for(mut cond: impl FnMut() -> bool, bound: Duration) -> bool` — poll under `tokio::time::timeout` chunks; NO `std::thread::sleep`/`tokio::time::sleep` between polls (use `tokio::task::yield_now` in the poll loop to satisfy `lint-test-sleep`).
4. Exercise: edit `page.jinja` (rendered output changes), edit only `part/head.jinja`, rapid double-edit within debounce, edit to invalid syntax (prior output keeps rendering, route alive, later valid edit recovers). Await the wrapper's watcher-ready barrier (Task 2.4) before the first write.

**Tests:**
- `template_edit_changes_output_without_restart`: capture rendered output via the test harness's exchange/producer seam; edit `page.jinja`; `wait_for(new_output, bound)` → assert new output AND the route status snapshot still `Running` with no restart event recorded (use whatever status/journal observability the harness exposes; assert no new journal events for the route).
- `nested_include_edit_triggers_reload`: edit only `part/head.jinja` → `wait_for` asserts the include change appears in output.
- `rapid_edits_coalesce_in_debounce`: two writes within one debounce window → assert exactly one reload happened by counting generation bumps (generation delta == 1, not merely stable output — a duplicate reload must fail this test).
- `failed_reload_keeps_output_and_route_alive`: install a tracing subscriber capturing `TemplateReload` error events before the edit; write invalid syntax → `wait_for` asserts at least one failed reload attempt was observed (captured error event) → assert prior output still renders; then write a valid template → `wait_for` new output (recovery works). A missing error event fails the test — stable output alone is not acceptance.

**Acceptance:**
- `cargo test -p camel-test --test dev_watch_template_test` exits 0.
- `cargo xtask lint-test-sleep` exits 0.
- `cargo clippy -p camel-test --tests -- -D warnings` exits 0 (or the workspace-equivalent invocation the repo uses for the test crate).

- [x] 4.1

#### Task 4.2: Dev-watch end-to-end integration test — proto and isolation

**Files:**
- `crates/camel-test/tests/dev_watch_proto_test.rs` (new)
- `crates/camel-test/Cargo.toml` (modified — if Task 4.1 did not already, ensure the validator is reachable from integration tests: enable the existing optional `camel-component-validator` dependency for the test target via a dev-feature (follow the crate's feature convention around line 138 of its Cargo.toml))

**Steps:**
1. Boot a watched context with a validator route (`validator:` endpoint with `type=protobuf`, a local `.proto` schema importing `common.proto`), following existing validator integration tests for endpoint URI shape (`rg -ln "validator:" crates/camel-test/tests/ crates/components/camel-component-validator/tests/ | head`).
2. Exercise proto edit and import edit through real fs writes with the `wait_for` helper from Task 4.1 (duplicate the helper inline in this file; it is five lines). Await the watcher-ready barrier before the first write.
3. Isolation checks: boot the same route set WITHOUT the watcher; edit files; assert no reload occurs (registry generation unchanged within a bounded window). Compiled-artifact non-watching is already guarded by `crates/camel-cli/tests/compiled_artifact_test.rs` — add one assertion there ONLY if a cheap seam exists; otherwise cite the existing suite in the task report.

**Tests:**
- `proto_edit_recompiles_validator_descriptor`: validate payload; edit schema adding a field → `wait_for` new-field validation passes.
- `import_edit_recompiles_validator_descriptor`: edit only `common.proto` → `wait_for` the import change reflected.
- `non_watch_mode_never_reloads`: no watcher started; edit template and proto files → assert registry generation for the route unchanged after the bounded window.

**Acceptance:**
- `cargo test -p camel-test --test dev_watch_proto_test` exits 0.
- `cargo test -p camel-cli --test compiled_artifact_test` exits 0.
- `cargo xtask lint-test-sleep` exits 0.

- [x] 4.2

### docs

#### Task 4.3: Document dev-watch behavior and the gRPC non-goal

**Files:**
- `docs/src/cli/index.md` (modified — the `camel run` / `--watch` reference lives in the CLI index; confirm with `rg -n "watch" docs/src/cli/`)
- `docs/src/configuration/hot-reload.md` (modified — the existing hot-reload page)
- `docs/src/components/validator.md` (modified)
- the template component page (modified — locate with `rg -ln "template" docs/src/components/ | head`; if no dedicated page exists, extend `docs/src/configuration/hot-reload.md` instead and record that in the report)

**Steps:**
1. CLI `--watch` documentation (`docs/src/cli/index.md`): document that watch covers route files plus template closures and validator proto descriptor sources (schema + local imports); state dev-only, debounced (`watch_debounce_ms`), atomic swap, failed-reload fallback (prior set keeps serving, redacted error logged), bounded reloads, no route restart/status change/journal events on template or proto edits.
2. Hot-reload page (`docs/src/configuration/hot-reload.md`) plus the template page: add a "Dev watch for template closures" section — closure watching (entry + nested includes), re-derivation after reloads, atomicity and fail-closed semantics; link the CLI watch documentation.
3. Validator page (`docs/src/components/validator.md`): descriptor recompile-on-edit incl. import-only edits; fixed 5000 ms deadline; fail-closed fallback; the cross-target all-or-nothing coupling on one route (a broken proto blocks same-route template reloads and vice versa); add the explicit non-goal note: gRPC endpoints retain descriptors resolved at startup — gRPC proto edits require a route reload or restart (follow-up bd from the deferrals ledger).
4. Run `cd docs && mdbook test` (docs changed — the repo gate runs it whenever `docs/**` changes).

**Tests:**
- `mdbook_build_and_link_check`: `cd docs && mdbook test` → exit 0 (only when docs contain code blocks; otherwise record the run anyway).

**Acceptance:**
- `cd docs && mdbook test` exits 0 (or is recorded as run with no code blocks affected).
- The three pages name the behavior set: dev-only, debounced, atomic, failed-reload fallback.
- English prose throughout (language policy).

- [x] 4.3
