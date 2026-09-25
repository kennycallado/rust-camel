# Design: tlsseam

## Approach

Constructor injection with a dual-handle global — the smallest seam
that decouples camel-ws tests from the process singletons while
keeping production identity byte-identical.

1. **Dual-handle global (camel-component-api, `tls_source.rs`).**
   `TlsReloadRegistry`'s backing store becomes a shared
   `static BACKING: OnceLock<Arc<TlsReloadRegistry>>` with a private
   accessor. `global()` returns the `&'static Self` obtained by
   dereferencing the `&'static Arc` the OnceLock hands out (signature
   and identity unchanged for the 20+ existing callers: runtime_bus,
   grpc, http, core tests); `global_arc()` returns
   `Arc::clone(backing())`. Isolated instances come from `Default`
   (already derived for `get_or_init`).

2. **ServerRegistry owns its TLS registry (camel-component-ws).**
   New field `tls: Arc<TlsReloadRegistry>`. `ServerRegistry` gets the
   same dual-handle backing (`static BACKING: OnceLock<Arc<ServerRegistry>>`;
   `global()` -> `&'static Self` via the backing's `Arc`,
   new `global_arc()`) and initializes its `tls` field with
   `TlsReloadRegistry::global_arc()`
   — so production handler registration is findable by the runtime's
   reload path (camel-core `runtime_bus` calls
   `TlsReloadRegistry::global().find(...)`, and both handles wrap the
   same backing `Arc` allocation). New
   `ServerRegistry::new()` constructs an isolated pair: fresh maps +
   fresh `TlsReloadRegistry`. The two `TlsReloadRegistry::global()`
   register sites inside `get_or_spawn`/`get_or_spawn_with_listener`
   register into `self.tls` (Arc cloned into the OnceCell init
   closure). Accessor `tls_registry()` exposes it for assertions.

3. **Lifetime bounds are vestigial — drop them.** The OnceCell init
   closures capture `Arc<OnceCell<ServerHandle>>`, not `self`; server
   and monitor tasks hold `Arc<WsAppState>`, never `&'static
   ServerRegistry`. `&'static self` -> `&self` on `stage_listener`,
   `get_or_spawn`, `get_or_spawn_with_listener`,
   `ref_count_for_test`, `bound_addr_for_test`; `reset()` becomes an
   instance method (`pub fn reset(&self)`, still `#[cfg(test)]`),
   aborting only that instance's servers.

4. **Consumer injection.** `WsConsumer` stores
   `server_registry: Arc<ServerRegistry>`; `new(cfg, rt)` delegates to
   `with_server_registry(cfg, rt, ServerRegistry::global_arc())`.
   `start`/`start_with_listener`/`stop` use the field instead of
   `ServerRegistry::global()`. The endpoint path (`create_consumer`)
   is untouched — it calls `new`, which keeps the global default.
   `WsProducer` needs no change (it looks up connection registries by
   bound address; consumer-keyed, no reset semantics).

5. **Test migration.** Each camel-ws lib test that touched
   `ServerRegistry::global()` constructs its own isolated instance
   (direct registry-mechanics tests) or injects one via
   `WsConsumer::with_server_registry` (consumer-path tests).
   `ServerRegistry::reset()` calls become `reg.reset()` on the local
   instance; TLS-reload assertions (`TlsReloadRegistry::global().find`)
   move to `reg.tls_registry().find`. `REGISTRY_TEST_LOCK` and its
   `acquire_deadline` scaffolding are deleted from migrated tests.
   Tests keep unique ephemeral ports, so isolated registries never
   collide.

6. **Measurement.** Before/after on this machine, methodology in bd:
   M1 = quiet `cargo test -p camel-component-ws --lib` test-phase
   time; M2 = same pinned to one CPU (`taskset -c 0`) where the
   serialized chain is visible. The audit's 602s was fleet-load
   starvation stretching each of 43 serialized holds; the seam removes
   the serialization itself (holds overlap), so M2 collapses toward
   the longest single test.

## Affected crates

- `camel-component-api`: `tls_source.rs` — dual-handle global. Docs.
- `camel-component-ws`: `lib.rs` — ServerRegistry field/lifetimes,
  reset accessor, WsConsumer injection, test migration (~60 call
  sites, mechanical).
- Docs: both crates' `CONTEXT.md` (registry seam section);
  `openspec/changes/tlsseam/specs/` delta (new capability).

## Architecture boundaries

Components layer only. The runtime (camel-core) reload path is
untouched — it still reads `TlsReloadRegistry::global()`; identity
preservation is the design invariant (one allocation, two handles).
Data plane unchanged; this is test-infrastructure seams on the control
plane. No hexagonal-boundary crossings: the registry stays a
component-api concern; camel-ws consumes it via `Arc`.

Single-phase change — one coherent slice, no milestone grouping.

## ADR alignment

- ADR-0069 (integration-tier testing contract): migrated tests keep
  bounded waits where the lockdeadline contract itself is under test;
  no new unbounded waits (ratchet 296).
- ADR-0070 (staged listeners): seam composes with staged listeners —
  isolation removes the cross-test abort hazard ADR-0070 worked around
  with port determinism.
- rc-p823t ruling (Option C + seams): production TLS material delivery
  stays global/reload-based; this change adds ONLY the test seam.
