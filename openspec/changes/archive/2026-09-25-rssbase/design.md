# Design: rssbase

## Approach

Two independent fixes, both in camel-core, each breaking one retention
edge measured in the mission-250 diagnosis (bd rc-wlg8h).

**Fix A — drop-path task termination (non-graceful).** `CamelContext`
gains `impl Drop` that takes `actor_join` and `supervision_join` and
calls `JoinHandle::abort()` on each. Today those handles are stored
(`context.rs` fields; `ContextParts::_actor_join` is even
underscore-named "unused") and silently dropped, detaching both tasks.
`stop_context` intentionally leaves the actor alive for stop→start
restarts and routes destructive teardown to `abort()` — that contract is
preserved: `Drop` fires only when the context value itself is discarded,
a state from which no restart can occur. This `Drop` is explicitly
NON-graceful: it terminates tasks; it does not replace route/service
teardown, which a caller must still drive with `stop()` before dropping
(cancellation mirrors `abort()`: an outstanding controller command is
interrupted at its await point; ADR-0018 sequencing — side effects
confirmed before completion — is not extended to the drop path).
Callers that retained `ctx.runtime()` bus handles see the same
cancellation as under `abort()`. The supervision task is aborted for the
same reason `abort_context` does so: it holds a controller-handle sender
clone and a `crash_rx` receiver, either of which pins the actor channel.

**Fix B — non-owning component context.** `RegistryComponentContext`
(shared/components/domain/registry.rs) stores
`Weak<Mutex<Registry>>` instead of `Arc`. `resolve_component` upgrades,
locks, resolves, releases the guard before returning the component. The
main boot paths hand the wrapper an `Arc` the owning context also holds
strongly (`camel_bundles::boot` WasmBundle wiring via `ctx.registry_arc()`,
`boot_scenario` security compile context, camel-core shared domain), so
the weak reference stays anchored while the context lives. This breaks
the `Registry → wasm component → Arc<RegistryComponentContext> →
Registry` cycle — the mechanism that pinned the entire component map
(including `camel-xj`'s dedicated `OffloadRuntime`: one OS thread + four
fds per boot). When the registry is gone, `resolve_component` returns
`None` — the same answer as an unknown scheme, correct for a context
that no longer exists.

Audit correction (expert review): four executable examples move their
ONLY strong registry `Arc` into the wrapper —
`examples/wasm-example/src/main.rs`,
`examples/wasm-bean-example/src/main.rs`,
`examples/wasm-streaming-plugin/src/main.rs`, and
`examples/security-wasm-policy/src/main.rs` (standalone registries
built before the context exists). Under `Weak` their anchor dies at
construction. Fix: each example retains a strong anchor `Arc` clone in
scope for the program's lifetime (or reuses the context-owned registry
where construction order allows). The existing in-tree test
`resolve_component_unaffected_by_observability_params` moves its sole
strong reference too and keeps an anchor across its assertion. A
live-resolution test covers the anchored standalone case: resolution
succeeds while anchored, returns `None` after the anchor drops.

Measurement summary (dev-profile binary, external /proc sampler,
partner-crud proxy corpus): baseline 0.4909 MB/doc → 0.0036 MB/doc,
threads 13→13 flat (was +1/doc), fds flat (was +4/doc).

## Affected crates

- `camel-core`: `context.rs` (Drop impl), `shared/components/domain/
  registry.rs` (weak field + upgrade in resolve), regression tests in
  both modules.
- `examples/wasm-example`, `examples/wasm-bean-example`,
  `examples/wasm-streaming-plugin`, `examples/security-wasm-policy`:
  retain a strong registry anchor alongside the wrapper
  (compile-semantic fix; no behavior change while anchored).
- `camel-cli`: no code change; verification measurements only.

## Architecture boundaries

Runtime (camel-core lifecycle): Fix A touches only the context drop
path; `stop_context`/`abort_context` algorithms unchanged — the
documented "stop keeps the actor, abort destroys it" contract gains a
third leg ("drop terminates it") in the same spirit as abort.
Components: Fix B changes no component; it changes the resolution seam
components already consume (`ComponentContext::resolve_component`),
keeping the trait and all call sites signature-identical. DSL, Services,
Languages, Functions: untouched. The scenario driver and tier machinery
(camel-cli / camel-integration-test) are explicitly out of zone.

## Phases

Single-phase: one coherent slice (two small fixes + tests + one
measurement verification task). No `## Phase N` headings in tasks.md.
