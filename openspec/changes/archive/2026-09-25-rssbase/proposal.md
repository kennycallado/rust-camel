# Proposal: rssbase

## Why

`camel test` scenario-tier batches grow RSS linearly at ~0.45–0.49 MB/doc
(~180 MB @ 218 docs; bd rc-wlg8h). Mission 250 re-baselined on main @
d090ffc6 (post-settlefix): the growth persists — slope 0.49 MB/doc on a
218-doc proxy corpus, 1090/1090 rows green. External /proc sampling plus
drop-probe instrumentation isolated two independent retention defects in
camel-core, both per full-tier boot:

1. **Controller-actor retention.** `stop_context` deliberately keeps the
   route-controller actor alive for stop→start restarts; only `abort()` is
   destructive. The scenario path (and any embedder) calls `stop()` then
   drops the context — nobody aborts. The actor task parks on
   `rx.recv()` forever (it holds a sender clone for restart re-sends, so
   the channel never closes), retaining `DefaultRouteController`: +3
   `NamedRegistry` instances/doc (cache, claim-check, idempotent) plus a
   strong component-registry `Arc`.
2. **WasmBundle registry cycle.** `Registry → wasm component →
   Arc<RegistryComponentContext> → Arc<Registry>` — self-sustaining after
   context drop, retaining the whole component map. Symptom: the `xj`
   component never drops, leaking its dedicated `xj-offload` tokio runtime
   (+1 thread, +4 fds: 2×eventpoll, 1×eventfd, 1×socket) and component
   state per doc.

Validated fix pair (diagnostic patches, reverted after measurement):
Drop-path task abort + weak registry reference → slope 0.0036 MB/doc
(−99.3%), threads and fds flat, all rows green.

## What Changes

- `camel-core` context: `Drop for CamelContext` aborts `actor_join` and
  `supervision_join` (drop means no restart is possible; stop→start
  semantics untouched).
- `camel-core` registry domain: `RegistryComponentContext` holds a
  `Weak` registry reference; `resolve_component` upgrades and returns
  `None` when the owning registry is gone.
- `examples/wasm-example`, `examples/wasm-bean-example`,
  `examples/wasm-streaming-plugin`, `examples/security-wasm-policy`:
  retain a strong registry anchor (their only strong `Arc` today moves
  into the wrapper).
- Regression tests: probe-component drop observability after context
  drop; anchored-live and post-drop weak-resolution unit tests.
- Excluded: settle machinery (exonerated), scenario driver bookkeeping
  (exonerated — fake-only no-boot corpus is flat), partner listeners
  (close cleanly), per-caller abort patches in camel-cli.

Affected crates: `camel-core` (behavior), `examples/*` (anchor fix),
`camel-cli` (verification only, no code). bd: rc-wlg8h.

## Acceptance criteria

- 218-doc scenario proxy batch: RSS slope ≤ 0.05 MB/doc, thread and fd
  counts flat (recorded in bd).
- `Drop` runs for every component owned exclusively by the context's
  component registry after context drop (probe test, bounded wait ≤ 5 s).
- `RegistryComponentContext::resolve_component` returns `None` once the
  last strong registry reference is gone.
- Existing stop→start restart tests pass unchanged.

## Risk budget

- `Drop` abort is a NON-graceful termination of the controller actor and
  supervision tasks. It requires a preceding explicit `stop()` for
  route/service teardown and can interrupt an outstanding controller
  command at its await point (ADR-0018: lifecycle intent precedes side
  effects; callers retaining `ctx.runtime()` handles observe the same
  cancellation semantics as `abort()`). `stop()`→`start()` restart
  semantics are untouched. Acceptable for a discarded context.
- Weak resolution changes semantics only for registries whose last
  strong reference is gone (previously: leaked-but-working; now:
  `None`). Main boot paths anchor the registry in the owning context.
  Four example programs (`wasm-example`, `wasm-bean-example`,
  `wasm-streaming-plugin`, `security-wasm-policy`) currently move their
  only strong registry `Arc` into the wrapper — they must be adjusted
  to retain a strong anchor alongside, with a live-resolution test
  covering the anchored standalone case.
- Out of bounds: touching `stop()`/`abort()` semantics, the scenario
  driver, mission 243 (compile/embed) and 249 (job) zones.
