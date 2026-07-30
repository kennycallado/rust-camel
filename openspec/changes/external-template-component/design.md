# Design: external-template-component

## Approach

The Component owns external resource I/O and lifecycle;
`camel-language-minijinja` stays inline-only. It builds a complete bounded
template snapshot off the hot path, compiles and validates the full set, then
swaps it atomically (`ArcSwap`). This mirrors the TLS reload precedent
(`crates/components/camel-component-grpc/src/tls_reload.rs:40-60`,
`crates/components/camel-component-api/src/tls_source.rs:137-140`) and
ADR-0047 §Stage 2.

Startup compiles the initial set inside the new async `StepLifecycle::start`
hook (added to the **existing** trait in `camel-api`) awaited by
`DefaultRouteController::start_route` before pipeline/Consumer spawn; a failed
compile fails the route closed (`Failed`) and successfully-started handles roll
back in reverse order. Hot-path renders read the current compiled set with zero
FS I/O. Reload reads a fresh bounded snapshot, recompiles, and swaps only on
success; failure retains the prior set.

The inline MiniJinja compile/render logic (today inline in
`camel-language-minijinja/src/lib.rs`) is extracted to a public `engine` module
(`pub async fn render`) so both the inline Language and the external Component
share one bounded renderer.

## Affected crates

- `camel-api`:
  - `step_lifecycle.rs:30-36` — add `async fn start(&self) -> Result<(),
    CamelError>` with a blanket default `Ok(())`. Existing implementors
    (`ResequencerService`, `AggregatorService`) are unaffected. The erased form
    remains `Arc<dyn StepLifecycle>`.
  - `error.rs` — add `CamelError::TemplateReload(String)`; force-update
    `variant_name()` (`:159`, compile-error without the arm); add `classify`
    arm returning `"template"` (≤15 ASCII); add a sample in `all_error_samples`
    (`:207`) and a case in `variant_name_covers_all_variants` (`:383`).
- `camel-component-api` (`endpoint.rs:42-52`): add an additive
  `Endpoint::lifecycle(&self) -> Option<Arc<dyn StepLifecycle>>` accessor with a
  blanket default `None`. `create_producer`'s `Result<BoxProcessor, CamelError>`
  return type is unchanged.
- `camel-core`:
  - `step_compilers/endpoints.rs` — populate `CompiledStep::Process.lifecycle`
    from `endpoint.lifecycle()` in BOTH the `To` (`:41`) and `WireTap` (`:52`)
    arms instead of hardcoding `None`; extend `resolve_producer` to surface the
    lifecycle handle alongside the producer.
  - `route_controller_trait.rs:30 start_route` — after assembly, await
    `start()` on every lifecycle handle in order before the pipeline/Consumer
    spawn (`:146`/`:257`); on the Nth failure, call `shutdown(RouteStop)` on
    handles 1..N in reverse order and return `Err`.
- `camel-language-minijinja`: extract private compile/render (`:142`,
  `:214-265`) to a public `engine` module. Zero behavior change; the inline
  Language re-exports `engine::render`.
- `camel-template` (new): Component, Endpoint, URI parser, snapshot, path policy,
  closure walker, reload handler, metrics. Defines `ExternalTemplateLimitsConfig`
  and `TemplateReloadError` here (component-config convention), re-exported by
  `camel-config`.
- `camel-config`: re-export `ExternalTemplateLimitsConfig` (Task 1.4); wire a
  `TemplateBundle` component-config block so both limit layers are
  operator-configurable (Task 4.5). No new `camel-cli` diagnostics command (none
  exists today); only the `run.rs` component registration is added.

## Two limit layers

Acquisition (Stage 2, component-level, `ExternalTemplateLimitsConfig`):
`max_total_source_bytes`, `max_include_count`, `max_include_depth`,
`max_template_size`, `reload_timeout_ms` — govern snapshot/closure acquisition
and reload wall-clock. Render (Stage 1, engine-level, reused
`MinijinjaLimitsConfig`): `max_template_source_size`, `max_context_size`,
`max_output_size`, `fuel`, `max_recursion_depth`, `execution_timeout_ms` —
govern each render. Both layers carry finite non-zero defaults; startup rejects
zero or invalid values. `ExternalTemplateLimitsConfig` lives in the new
`camel-template` crate (NOT `language_limits.rs`) and is re-exported by
`camel-config`, mirroring how `MinijinjaLimitsConfig` is defined in
`camel-language-api` and re-exported.

## Architecture boundaries

Respects the data/control plane split (CONTEXT-MAP ADR-0045): per-Exchange
render is data plane (`Service<Exchange> -> Result<Exchange, CamelError>`);
template compilation and reload are control plane. ADR-0032 (Exchange-data trust
boundary) governs: operator config is trusted; Exchange body/headers/properties
are untrusted and must never select the template resource, cross a control-plane
action, or enter the executable sink unbounded. `ReloadTemplates` mirrors
`ReloadTlsCerts`: the `RuntimeBus::execute` intercept
(`runtime_bus.rs:173`) returns early before journal recovery (`:196`) and dedup
(`:198`), so it persists no lifecycle intent and does not mutate `RouteStatus`
(ADR-0018 not invoked). `template_reloads_total` increments once per
route-scoped aggregation (label `route`).

## Root and dependency-closure contract

The configured root is the parent directory of the entry template file. Include,
extends, import, and from targets resolve openat-relative to that root through an
iterative single-pass DFS closure walker (`acquire_closure`); each new source is
read before its outgoing edges are known. Dynamic (render-time-computed)
template names are rejected — only statically discoverable closures are
permitted. Symlinks, `..`, absolute paths, cycles (on-stack Gray detection),
and duplicate file identities are rejected. Cross-platform `FileIdentity`:
Unix `{inode,length,mtime_nsec}`, Windows
`{volume_serial,file_index_high,file_index_low,length,last_write_100ns}`.

## Reload generation semantics

Generation is producer-assigned and monotonic: it starts at 0 for the initial
dry/startup compile and increments by one on each successful swap.
`ReloadTemplates` carries NO generation in the command payload (mirrors
`ReloadTlsCerts`, which carries no generation). The ONLY reload path is
`reload_route` (there is no single-producer convenience on the erased trait); it
serializes per route via a mutex, so no concurrent bump can break all-or-nothing.
`reload_route` is all-or-nothing: it builds every target, validates every staged
generation, then commits all via an infallible commit (any build or validation
failure commits none). Stale-generation rejection is the validate phase: a build
tagged at generation G is rejected if the current generation is already `> G`.

## Phases

The change is delivered in five ordered phases (design-time decision; `tasks.md`
carries all five, blessed once):

### Phase 1: types

- **Goal:** shared types + error variant + engine extraction. Zero behavior
  change.
- **Dependencies:** none (safe entry point).
- **Externally-visible types:** `CamelError::TemplateReload`; public `engine`
  module (`pub async fn render`); `TemplateReloadError`; `ExternalTemplateLimitsConfig`.
- **Deliverable:** compiles standalone; inline Language unchanged.
- **Exit-criteria:** `cargo test -p camel-language-minijinja` green; `cargo test
  -p camel-api` green with `variant_name_covers_all_variants` including
  `TemplateReload` and `classify` returning `"template"`.

### Phase 2: path-policy

- **Goal:** URI parser + cfg-gated path I/O + iterative closure walker + limits.
- **Dependencies:** Phase 1.
- **Externally-visible types:** `template:file:///` URI parser; `OwnedHandle`
  (Unix `OwnedFd` / Windows `OwnedHandle`); `acquire_closure`; `FileIdentity`.
- **Deliverable:** off-path bounded snapshot acquisition, root-confined,
  TOCTOU-safe.
- **Exit-criteria:** `cargo test -p camel-template` covers symlink/`..`/absolute/
  cycle/duplicate-identity rejection; `acquire_closure` is transitively complete.

### Phase 3: producer-start-lifecycle-spi

- **Goal:** async startup hook + endpoint lifecycle wiring.
- **Dependencies:** Phase 1 (parallel with Phase 2).
- **Externally-visible types:** `StepLifecycle::start`; `Endpoint::lifecycle()`.
- **Deliverable:** `start_route` awaits `start()` in order before spawn with
  reverse-order `shutdown(RouteStop)` rollback; `endpoints.rs` populates
  `lifecycle` from the accessor.
- **Exit-criteria:** existing routes unaffected (blanket no-op `start` + default
  `None` accessor); `cargo test -p camel-core` lifecycle rollback tests green;
  `ResequencerService`/`AggregatorService` unchanged.

### Phase 4: render

- **Goal:** snapshot + render + Component/Endpoint/Service + startup-build +
  per-render metrics + CLI.
- **Dependencies:** Phases 1, 2, 3.
- **Externally-visible types:** `TemplateComponent`, `TemplateEndpoint`;
  `template:file:` scheme registration.
- **Deliverable:** end-to-end render from file URI; root template renders against
  the Exchange, replaces body only on success, preserves headers/properties;
  compile-once hot path.
- **Exit-criteria:** AC 1-4, 7, 9 met via `cargo test -p camel-template`
  integration tests; zero FS I/O on the hot path asserted.

### Phase 5: reload

- **Goal:** ReloadHandler + process-global registry (mirrors TLS reload) +
  `RuntimeCommand::ReloadTemplates` + RAII registration + route-scoped
  all-or-nothing commit + metrics + tests + docs.
- **Dependencies:** Phase 4.
- **Externally-visible types:** `ReloadHandler`; `RuntimeCommand::ReloadTemplates`;
  `RuntimeCommandResult::TemplatesReloaded`.
- **Deliverable:** atomic valid swap with prior-set retention on failure;
  route-scoped stage-all/commit-all; `reload_timeout_ms` prevents late swap.
- **Exit-criteria:** AC 5, 6, 8, 10 met via `cargo test -p camel-template` reload
  + stale-generation + timeout tests and `cargo test -p camel-core` runtime_bus
  dedup-bypass tests; CONTEXT-MAP ADR-0047 index entry added.

## Alternatives considered

- **Monolithic delivery:** rejected twice (self-grill, 7 then 5 critical
  findings) — too many interdependent architectural questions for one planning
  pass.
- **Inline-only (no Stage 2):** insufficient for SSR / shared sets / hot-reload;
  leaves ADR-0047 incomplete.
- **Lazy compile (XSLT `poll_ready` precedent):** fails on first use, not
  fail-closed at startup.
- **`block_in_place` (XJ precedent):** explicit hack; rejected for a generic
  async `start` hook.
- **Changing `create_producer` return type:** rejected as too invasive; an
  additive `Endpoint::lifecycle()` accessor with a default `None` achieves the
  same wiring.
- **`CreateFileW` on Windows:** no relative-to-handle mode; `NtCreateFile` with
  `OBJECT_ATTRIBUTES.RootDirectory` required.
