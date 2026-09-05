# Proposal: itest-bound-ports

## Why

The `camel-test` integration suite still obtains server ports via
`tests/support::find_free_port()` (17 callsites across 5 binaries): bind an
ephemeral port, read it, DROP the listener, then format a route URI with that
port and let the component registry re-bind it at context start. The
drop-to-rebind window is the port-toctou race class that `ws-bound-address`
(bd rc-9xsv) eliminated for `camel-component-ws` lib tests: under CI's
2-core runners the window widens and a sibling process can steal the port,
producing "Failed to bind" flakes that never reproduce on a warm local
machine. bd rc-h0aw (oracle-ordered follow-through) requires full-crate
coverage including integration binaries.

Blocked-on notes: the harness drives routes by URI string, so the
consumer-level `start_with_listener` surface from rc-9xsv cannot be used
directly here — the registry must accept a pre-bound listener out-of-band.

## What Changes

- `camel-component-http` `ServerRegistry`: add `get_or_spawn_with_listener` (serves a
  pre-bound `tokio::net::TcpListener`, keys by its actual local address,
  stores `bound_addr` on the entry) plus `stage_listener` — a one-shot slot
  the process-global registry consumes atomically inside `get_or_spawn`
  before binding, so a staged port has no drop-to-rebind window.
- `camel-ws` `ServerRegistry`: add the same one-shot `stage_listener`
  consumption inside its existing `get_or_spawn` path (its
  `get_or_spawn_with_listener` from rc-9xsv already covers the direct case).

- `camel-test` `tests/support`: replace `find_free_port` with
  `stage_http_listener(host) -> u16` / `stage_ws_listener(host) -> u16`
  (bind `{host}:0`, stage into the registry the component will consult,
  return the actual port).
- Migrate all 17 callsites (http_test 10 — two `0.0.0.0`, audience_substitution
  2 — one WsComponent, auth_multi_credential 1 — `0.0.0.0`), kernel_fail_closed
  2, late_registration_gate 2 — one `0.0.0.0` intentional nonloopback coverage):
  bind `{host}:0` → stage → URI uses the actual port.
- Delete `find_free_port` from `tests/support` (grep = 0).

Excluded: `camel-component-ws`/`camel-component-http` lib-test migrations
(ws done in rc-9xsv; http lib tests use `spawn_test_server` port-0 already),
wasm migration (bd rc-wgba), any CLI surface. Behaviorally compatible when no listener is staged:
`stage_listener` is additive and empty by default.

## Acceptance criteria

- `grep -rn find_free_port crates/camel-test/` returns nothing.
- All 5 integration binaries compile and pass their full target sets
  (`cargo test -p camel-test --features integration-tests --test <each>`),
  not library-only, per the oracle's rc-y5nn lesson.
- `cargo test -p camel-component-http --all-targets` and `-p camel-component-ws
  --all-targets` green (registry additions regression-free).
- New tests per the delta spec scenarios: first `get_or_spawn` on a staged
  key uses the listener; second caller reuses the entry; wrong-host staged
  port fails deterministically (no silent fresh bind); duplicate staging
  rejected; distinct keys consume independently in both registries; staged
  spawn then release/reuse on ws; TLS pre-bound served.
- Quality gates green per AGENTS.md.

## Risk budget

Registry slot adds one `Mutex`-guarded map lookup on the `get_or_spawn` hot
path — acceptable. One-shot semantics (staged listener dropped if no route
ever claims it) must be documented loudly; a silently-dropped listener in a
test is a test bug, not a runtime hazard. No changes to production boot
paths beyond an empty-slot check.
