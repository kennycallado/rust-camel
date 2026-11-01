# Design: 476-template-proto-dev-reload

## Approach

Extend the existing dev-mode file watcher (`camel-core` `reload_watcher`) with
a second event class. Today the watcher watches route directories and treats
yaml/yml/json events as route-reload triggers. After this change, the same
watcher also watches individual template closure files and validator protobuf
descriptor sources, and dispatches the existing
`RuntimeCommand::ReloadTemplates` for the affected routes.

Proto scope is narrowed by master ruling (2026-11-01): validator-protobuf
descriptors only. gRPC live descriptor reload is a documented non-goal and a
deferral-ledger follow-up: gRPC consumers and producers retain descriptors
resolved once at startup (`resolve_descriptors` in `start_inner`; producer
convert helpers take descriptors as parameters), so watching gRPC protos
without a refresh path would be inert.

Three seams carry the work:

1. **Watch-source exposure** — `TemplateReloadTarget`
   (`camel-component-api/src/template_reload.rs`) gains
   `fn watch_sources(&self) -> Vec<PathBuf>`, returning the unique absolute
   FILE paths whose change should reload that target: for a template target,
   every closure file including the entry and nested includes (the
   confinement root DIRECTORY is not a watch target); for a validator
   protobuf target, the schema file plus its resolved local transitive
   imports (built-in and embedded imports excluded). The registry gains an
   enumeration of `(route_id, sources)` pairs. camel-template's
   `ReloadHandler` retains the root path next to its `OwnedHandle` and the
   last committed closure's file list; the validator target resolves its
   import closure during each staged build. Sources reflect COMMITTED state
   only — a failed build changes nothing, and the staged set carries the
   recomputed source list so `commit` swaps sources together with the
   compiled artifact (an import added on reload becomes watched after the
   commit).

   camel-proto-compiler currently keeps its source-closure logic private
   (`source_cache_key` in `cache.rs`); this change adds a public
   import-closure helper there (schema path in, local import paths out,
   built-in and embedded inputs excluded) so the validator does not
   re-implement proto import resolution.

2. **Watcher extension** (`crates/camel-core/src/hot_reload/adapters/reload_watcher.rs`)
   — the watcher maintains a dev-target map `path -> set(route_id)` derived
   ONLY from registered reload targets (committed per-route metadata), never
   from raw discovered `RouteDefinition`s. Initial derivation runs after
   startup; re-derivation runs after every route reload pass and after every
   committed asset reload. On partial route-reload failure the map keeps the
   surviving routes' targets (their registry registrations survive); entries
   are removed only when their owning endpoint unregisters. Tracked files
   are watched via their unique PARENT DIRECTORIES with
   `RecursiveMode::NonRecursive` (deduplicated against route-directory
   watches); events are filtered to the tracked path set. Route directories
   keep the existing recursive watches. Watching parents instead of files
   survives delete-and-recreate: a per-file watch dies with the inode, but a
   parent-dir watch sees the `Remove` and the later `Create` for the tracked
   name; `Remove` and `Create` on a tracked path both count as dev-target
   events. When the tracked set changes after re-derivation, subscriptions
   are reconciled (parents added or dropped accordingly). Event
   classification splits a debounce batch into route-file events and
   dev-target events; a path present in the dev-target map is classified
   as a dev-target event FIRST, even if it carries a route-file extension
   inside a route directory:

   - **Any route-file event** → run the existing route-reload pass first
     (unchanged semantics — including its early-continue paths when
     discovery fails or the pass computes no actions, in which case pending
     dev-target events still dispatch), then re-derive the target map, then
     process the batch's pending dev-target events against the refreshed
     map: routes the pass rebuilt (identified from the computed reload
     action list) already recompiled their assets, but routes whose
     generation survived the pass WITHOUT a rebuild still get their
     `ReloadTemplates` dispatch if their files changed.
   - **Dev-target events only** → for each affected route with at least one
     registered target, dispatch `RuntimeCommand::ReloadTemplates {
     route_id }` through the existing `RuntimeExecutionHandle`, one command
     per route. Routes without targets are skipped with a debug log. After a
     successful dispatch the map is re-derived (a template edit may grow the
     closure).

   **Atomic-save tolerance**: because subscriptions sit on parent
   directories and events are filtered by tracked path, an editor's
   write-temp-rename or delete-and-recreate sequence does not disable later
   reloads: the `Remove` and the subsequent `Create`/`Modify` on the tracked
   name are all observed through the parent watch.

   **Dispatch bounding and cancellation**: each dispatch is awaited under
   `tokio::time::timeout` bounded by the route's registry reload deadline
   plus the existing drain timeout; the await is `select!`-biased with the
   shutdown token, so shutdown stays responsive. Events arriving during a
   dispatch buffer in the existing channel. On timeout or shutdown the
   command future is DROPPED — `reload_route` performs build, validate, and
   commit inside that future with no `.await` in the commit loop, so a
   dropped dispatch cannot commit later; a detached blocking build task may
   finish but its staged set is unreachable (same build/commit split
   guarantee as camel-template's reload handler). Failures and timeouts log
   a redacted error and the loop continues; the registry guarantees the
   prior committed set is retained.

   **Cross-target coupling**: `reload_route` is all-or-nothing per route
   across ALL registered targets and uses the tightest deadline. A broken
   validator proto therefore blocks template reloads on the same route (and
   vice versa); nothing commits and both prior sets keep serving. This
   coupling is accepted and documented rather than worked around.

   The reload command goes through the `RuntimeBus` intercept that already
   bypasses journal append and dedup, so no journal events are written and
   `RouteStatus` is untouched.

3. **Validator proto reload target** (`camel-validator`) — the protobuf
   `CompiledValidator` currently fixes its `MessageDescriptor` at endpoint
   construction. The endpoint moves the descriptor behind an `ArcSwap`
   shared with a reload target registered in `TemplateReloadRegistry` for
   its route — registered only when the endpoint has a real `route_id`
   (the existing `validator-bridge` fallback id is skipped with a debug
   log, so bridge targets never enter the per-route registry).
   `build` runs the synchronous protox compilation inside `spawn_blocking`
   and recomputes the local import closure (bounded, fail-closed — an
   invalid schema returns `Err` and nothing is staged); `commit` is
   infallible and swaps the descriptor together with the recomputed source
   list. The target's `reload_timeout` returns a fixed 5000 ms default
   matching camel-template's `reload_timeout_ms` — no new URI parameter;
   `watch_sources` returns the committed schema file plus local imports.

## Affected crates

- `camel-component-api`: `TemplateReloadTarget::watch_sources` + registry
  enumeration.
- `camel-template`: retain root path and committed closure paths; implement
  `watch_sources` (unique closure files, entry included, root directory
  excluded).
- `camel-core`: reload_watcher event classification, committed-state target
  map, parent-dir subscriptions, `ReloadTemplates` dispatch, mixed-batch
  ordering, re-derivation, bounded/cancellable dispatch.
- `camel-cli`: wire the extended watcher (`camel run --watch` only;
  `WatchSpec` gate unchanged).
- `camel-validator`: protobuf descriptor `ArcSwap`, reload-target
  registration with committed import sources, spawn_blocking build.
- `camel-proto-compiler`: NEW public import-closure helper (local imports
  of a schema path, built-in and embedded inputs excluded), extracted from
  the private cache keying logic.
- `camel-component-grpc`: NO change (out of scope by master ruling).
- Docs: `--watch` reference, template hot-reload, proto dev workflow, gRPC
  non-goal note.

## Architecture boundaries

Control plane only: the watcher is a dev-mode trigger; it dispatches an
existing control-plane command through the `RuntimeExecutionHandle`. No
data-plane processor changes. camel-core keeps its dependency direction (it
reads the registry through `camel-component-api`; route definitions still
arrive via the injected discovery closure — camel-core never depends on
camel-dsl). ADR compliance: atomic swap per ADR-0004; reload seam per ADR-0002
(CQRS RuntimeBus); template confinement per ADR-0047 (closure files are
watched by path, the reload build still acquires openat-relative to the
root); hermetic proto per ADR-0084 (protox in-process); compiled artifacts
never watch per ADR-0075 non-goals.

## Phases

### Phase 1: Watch-source exposure

- **Goal:** reload targets can tell the world which files they were built
  from, reflecting committed state only.
- **Dependencies:** none.
- **Externally-visible types/interfaces:** `TemplateReloadTarget::watch_sources`
  (trait method), `TemplateReloadRegistry` enumeration of per-route sources.
- **Deliverable:** trait + registry API + camel-template implementation with
  unit tests.
- **Exit-criteria:** a registered template target reports the unique closure
  files (entry + nested includes, no root directory) as absolute paths;
  failed builds leave reported sources unchanged; tests pass in
  camel-template and camel-component-api.

### Phase 2: Dev-watch watcher

- **Goal:** `camel run --watch` watches closure and validator proto files and
  dispatches `ReloadTemplates` without route restart.
- **Dependencies:** Phase 1.
- **Externally-visible types/interfaces:** extended `watch_and_reload`
  signature; internal event classification and target map.
- **Deliverable:** watcher changes + camel-cli wiring + unit tests
  (classification, committed-state target map, mixed-batch ordering with
  surviving-route dispatch, debounce coalescing, delete/recreate tolerance,
  no-target skip, bounded dispatch). Tests feed synthetic `notify::Event`s
  into the watcher channel through an injectable event source and drive the
  debounce with `tokio::time::pause` — no sleeps, no unbounded waits.
- **Exit-criteria:** dev-target events dispatch per-route reload commands;
  route-file events keep existing behavior; mixed batches dispatch for
  surviving unrebuilt routes (including when discovery fails or the pass
  computes no actions); replaced files keep firing; the loop stays
  responsive to shutdown during dispatches; no late commit after timeout.

### Phase 3: Validator proto reload target

- **Goal:** a validator `.proto` edit recompiles the descriptor live.
- **Dependencies:** Phase 1 (registry, watch_sources).
- **Externally-visible types/interfaces:** none (endpoint-internal swap).
- **Deliverable:** `ArcSwap` retrofit + reload-target registration (committed
  schema + import sources, fixed 5000 ms deadline, spawn_blocking build,
  registration skipped for the `validator-bridge` fallback id) + the
  camel-proto-compiler import-closure helper + tests (edit observed incl.
  import-only edit, failed recompile keeps prior descriptor, timeout aborts
  without commit, reload-grown import becomes a watch source).
- **Exit-criteria:** proto or local-import edit through the reload command
  swaps the descriptor; invalid proto keeps the old descriptor and the route
  alive; a stalled build times out with no commit.

### Phase 4: Integration + docs

- **Goal:** end-to-end dev-watch behavior proven and documented.
- **Dependencies:** Phases 1–3.
- **Externally-visible types/interfaces:** docs.
- **Deliverable:** dev-watch integration test (template edit changes output
  without restart/status change; nested include; validator proto edit
  recompiled; failure fallback; non-watch never watches; targets limited to
  declared sources), CLI/template/proto docs with the gRPC non-goal note.
  Integration tests wait on real filesystem events only through bounded
  wait-for-condition helpers — no sleeps, no free ports, no raw HTTP test
  servers.
- **Exit-criteria:** order test list green with no sleeps, no free ports, no
  raw HTTP test servers; docs state dev-only, debounced, atomic,
  failed-reload fallback, and the gRPC deferral.

## Alternatives considered

- **gRPC descriptor-refresh targets**: rejected by master ruling 2026-11-01 —
  narrow over widen; gRPC endpoints retain startup descriptors, live refresh
  is a follow-up ticket, and gRPC proto references create no watch targets.
- **ControlBus `reloadTemplates` action** (bd note option): rejected — the
  order lists new trigger channels as a non-goal; the watcher is the
  sanctioned user-facing trigger.
- **Recursive watch on template root directories**: rejected — watches
  arbitrary files; individual closure files plus re-derivation cover the same
  edits.
- **Full route reload on template change**: rejected — the order forbids it
  for template-only/proto-only changes; it would also bounce consumers.
- **Deriving proto targets from discovered RouteDefinition URIs**: rejected —
  discovery output is not committed state; after a partial reload failure it
  diverges from what is actually running. Registered-target metadata is the
  committed truth.
- **New `ReloadProtos` runtime command**: rejected — duplicates the registry
  intercept machinery; `ReloadTemplates` already reloads every registered
  target for a route, and proto holders register targets in that registry.
- **Watching from inside components**: rejected — components would each own
  watcher lifetimes; the single dev-mode watcher keeps one debounce and one
  event loop in the CLI layer.
