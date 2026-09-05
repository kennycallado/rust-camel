# Design: wasm-bound-address

## Approach

Fourth application of the canon `staged-listener-binding` capability
(ADR lineage: ADR-0069 integration-tier testing contract; capability
spec canon after itest-bound-ports). The wasm source consumer's host
shim owns the bind, so the staged pattern applies without touching the
WIT contract or the guest worlds.

1. **Staged map** — new `pub mod staged_listener` at
   `crates/components/camel-component-wasm/src/staged_listener.rs`.
   Process-global
   `static STAGED: Mutex<HashMap<(String, u16), TcpListener>>` (std
   `Mutex`; never held across an await, mirroring camel-component-ws).
   Two operations, both mapping every failure (duplicate key,
   `local_addr()` failure, poisoned lock) to
   `CamelError::EndpointCreationFailed` — no `String` errors on the
   public surface:
   - `pub fn stage_listener(listener: tokio::net::TcpListener)
     -> Result<(), CamelError>` — reads `local_addr()`, inserts under
     the exact `(host-string, port)` key, rejects duplicates with
     `listener already staged for {h}:{p}`; the first staged listener
     stays in the slot.
   - `pub(crate) fn take(bind_addr: SocketAddr)
     -> Result<Option<TcpListener>, CamelError>` — exact-key hit →
     `Some(listener)` (removed from the map, one-shot); any entry on
     the same port under a different host string →
     `Err(EndpointCreationFailed("staged listener conflict on port
     {p}: staged under host {h}, requested {r}"))` without touching
     the socket or the slot; miss → `Ok(None)`. Host strings compare
     exactly, no DNS or wildcard normalization (canon semantics —
     `0.0.0.0` and `127.0.0.1` are different keys).
2. **Consumer integration** — at the single bind site
   (`source_consumer.rs:284`), AFTER the operator/guest bind agreement
   (step 12b) and the ADR-0061 exposure gate (step 12c):
   `Some(l)` → serve `l`; `None` → `TcpListener::bind` exactly
   as today; `Err(err)` → `return err` (already an
   `EndpointCreationFailed`). Ordering is
   load-bearing: a route refused by agreement or gate never consumes a
   staged slot, and staged consumption never bypasses the exposure gate.
   No registry exists for wasm sources (consumers are route-lifecycle,
   not port-addressed) — the map is consumed once, by the route whose
   config names the port.
3. **Test helper** — `tests/common/mod.rs`:
   `pub async fn stage_wasm_source_listener(host: &str) -> u16` — binds
   `tokio` listener on `{host}:0`, stages it, returns the actual port.
   Mirrors `stage_http_listener`/`stage_ws_listener` in camel-test.
   Of the 29 acquisition sites, 28 stage via the helper; the one site
   whose port feeds an asserted-unbound address
   (`source_bind_gate.rs` `conflicting_binds_fail_before_socket`,
   `port_b` — the test dials it expecting connection-refused, and a
   staged listener is bound by definition) becomes the fixed reserved
   address `127.0.0.1:1` with an explanatory comment. The four source
   binaries declare `mod common;`, all 29 `free_port().await` calls are
   replaced, and the four local `free_port()` copies are deleted.
   Guest config lines (`format!("127.0.0.1:{port}")`) are untouched —
   the port flows as before, but now names a socket the helper holds
   until the consumer consumes it. Readiness wait-loops that poll
   `TcpStream::connect(...).is_ok()` return as soon as the staged socket
   is bound; queued connections are picked up when `axum::serve` starts,
   so their semantics are preserved.

No reset API: staged keys are ephemeral ports (never reused within a
process), each test binary is a fresh process, and unconsumed slots
(refusal-path tests where the route never reaches the bind site) are
inert. A same-key re-stage attempt keeps failing loudly, which is the
intended signal.

## Affected crates

- `camel-component-wasm`: staged_listener module, consumer bind-site
  consumption, tests/common helper, 4 integration binaries migrated.
- No other crate changes. camel-test helpers are referenced for naming
  parity only.

## Architecture boundaries

Component crate, host-side only. No Runtime, DSL, Services, or Functions
boundary is crossed; the guest worlds and WIT contract are untouched
(the guest still declares a bind address; only the host's resolution of
that declaration gains a staged path). Test-tier port acquisition moves
from probe to staged, per ADR-0069's bound-address operator surface —
the papal R1 ruling (component-registry listener injection is operator
surface, not a core test-only event) covers the `pub` staging fn.

## Phases

Single coherent slice (one module, one bind site, one migration) — no
phase grouping; flat task list.
