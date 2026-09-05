# Design: itest-bound-ports

## Approach

Registry-mediated staged listeners. The harness (camel-test) binds a
port-0 `tokio::net::TcpListener` BEFORE any route URI exists, reads its
actual port, hands (stages) the listener into the process-global
component `ServerRegistry` under the exact `(host, port)` key the route
URI will later name, and formats the URI with the actual port. When the
consumer calls `get_or_spawn(host, port)` at context start, the registry
— under its existing mutex, before binding — atomically takes a staged
listener for that key and serves it instead of binding. The
drop-to-rebind window never opens: one socket, bound once, transferred
never.

This mirrors rc-9xsv (`ws-bound-address`) at the registry level instead
of the consumer level, because camel-test drives routes by URI string
through `CamelTestContext`, never touching consumers directly. The staged
slot is test-oriented injection (ADR-0069 discipline: no production
callback; empty slot = behaviorally compatible legacy behavior).

- `camel-component-http` `ServerRegistry::get_or_spawn_with_listener(listener, limits, runtime, tls)`
  threads an owned listener through the existing OnceCell init
  (`run_axum_server_tls` already accepts `std::net::TcpListener`; the
  staged tokio listener crosses via `into_std()`, the ws-proven path on
  axum-server 0.8). Entry stores `bound_addr: SocketAddr`; one-shot
  `stage_listener(listener)` keyed by the listener's own `(ip, port)`;
  legacy `get_or_spawn` consults the slot only when creating a vacant
  entry, under the same registry mutex (closes in-process TOCTOU), and a
  staged listener for the same port under a DIFFERENT host string makes
  the spawn fail deterministically (host strings can resolve to one
  socket; silent fresh bind risks EADDRINUSE).
- `camel-ws` `ServerRegistry`: same one-shot `stage_listener` +
  consumption inside its existing `get_or_spawn` (its
  `get_or_spawn_with_listener` from rc-9xsv already carries bound_addr).
- camel-test `tests/support`: `stage_http_listener(host) -> u16` /
  `stage_ws_listener(host) -> u16` (bind `{host}:0`, stage, return actual
  port; the host parameter equals the route URI's declared host — several
  sites intentionally use `0.0.0.0` for nonloopback coverage). Callsites
  keep their shape: `let port = stage_http_listener("127.0.0.1").await;`.
  17 callsites total (13 loopback, 4 `0.0.0.0`).

Staging semantics (both registries, documented on the fn): exact-key
match, one-shot take; a staged listener under a different host string on
the same port makes `get_or_spawn` return an error without binding (the
slot stays untouched); duplicate staging for an occupied key is rejected
(first listener retained); staged-but-unclaimed listeners are simply
dropped (test bug, not runtime hazard).

## Affected crates

- `camel-component-http`: registry listener plumbing, staged slot, bound_addr.
- `camel-component-ws`: staged slot + consumption in `get_or_spawn`.
- `camel-test`: support helper migration, 17 callsites, helper deletion.

## Architecture boundaries

Components layer only (camel-component-http, camel-component-ws) plus the test workspace
(camel-test). No core/runtime/DSL change; no production boot-path change
beyond an empty-slot map check under the existing registry mutex. Follows
the data/control-plane split: listeners are transport (data plane)
resources owned by component registries, exactly like the sockets they
replace. Terminology per test-determinism canon.

## Alternatives considered

- **Port-0 in the route URI**, registry assigns and rewrites authority —
  rejected: URI identity is load-bearing (SSRF guards, route keys, reload
  handlers match by declared URI); mutation ripples beyond registries.
- **Component-builder `with_listener`** (per component instance) —
  rejected: harness builds components generically; URI-keyed staging is
  component-agnostic and reuses the existing registry key space, so one
  support helper serves http and ws alike.
- **Retry-on-bind-failure loop around `find_free_port`** — rejected:
  narrows the race, does not eliminate the class; oracle requires
  elimination.
