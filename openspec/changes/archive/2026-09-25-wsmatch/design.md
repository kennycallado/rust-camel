# Design: wsmatch

## Approach

Move connection-registry resolution for accepted connections from a
process-global path-only scan to server-scoped owned state.

`WsAppState` — the per-shared-server state built in `spawn_server` and
injected into the axum router — already owns three per-server,
path-keyed maps: `dispatch` (path → envelope channel), `path_configs`,
`path_policies`. The fix adds a fourth, structurally identical map:

```rust
pub registries: Arc<DashMap<String, Arc<WsConnectionRegistry>>>,
```

- `spawn_server` constructs it empty alongside the others.
- `finish_start` inserts `path → Arc<WsConnectionRegistry>` into the
  server-scoped map (BEFORE the dispatch insert — an accept landing in
  the inter-insert window is rejected at the dispatch check rather than
  admitted unregistered; also before the global insert) AND keeps the
  existing exact-key insert into `GLOBAL_CONNECTION_REGISTRIES`
  (producer-facing index).
- `ws_handler` replaces the global scan
  (`for entry in registry.iter() if entry.key().2 == path`) with a
  single server-scoped `get(&path)`. The over-limit check reuses the
  same resolved entry instead of re-reading the global map.
- `stop()` removes the path entry from `state.registries` (in the
  block that already removes `dispatch` / `path_configs` /
  `path_policies` by path) and the full-key entry from the global map
  (unchanged, already exact).

Why not an exact-triple match on the global map: the accept path
knows only the listener's bound address, but `start` keys by
`canonical_host()` (URI-derived) while `start_with_listener` keys by
the listener's actual IP — there is no server-side reconstruction of
the consumer's key host. A per-server path map sidesteps host
canonicalization entirely: path is unique within one server (the
dispatch table is already keyed by path alone per server), and the
map is written by the same `finish_start` call that owns the
registry, so keying is consistent by construction.

Global map writes and exact-key producer lookups
(`contains_key(&key)` / `get(&key)`) are untouched — tls-registry-seam
pinned those as "existing behavior and remain".

## Affected crates

- `camel-component-ws`: `WsAppState` field, `finish_start`,
  `ws_handler`, `stop()`, two test-local `WsAppState` constructions,
  new regression test, CONTEXT.md vocabulary note.
- `camel-test`: `make_app_state()` in `tests/ws_security_test.rs`
  gains the new `WsAppState` field (`registries: Arc::default()`) —
  compile-fix only, no behavior change.

## Architecture boundaries

Component-internal change; no camel-api / camel-core surface moves.
Aligns with the tlsseam direction (rc-fx3xy): owned state +
constructor injection instead of process-global reads on hot paths.
The process global remains ONLY as a producer-facing exact-key index
— no new global accessors, no reintroduced global mutation on the
accept path.

## Test plan

1. `accept_registers_into_owning_server_registry_only` (new): two
   isolated `ServerRegistry::new()` instances, two consumers, same
   path `/echo-cross`, different ephemeral ports. Membership leg:
   each full-triple key's registry holds exactly one connection.
   Stop-isolation leg: stop consumer A; A's client gets closed (1001),
   B's client still echo-round-trips. Runs lock-free (isolated
   registries).
2. Existing suite pins single-server semantics: broadcast, targeted
   send, max-connections, stop-close, seam injection (166/166).
3. The two tests carrying the path-only warning comments
   (`injected_entry_survives_consumer_stop`,
   `consumer_injection_uses_provided_registry`) get their comments
   updated — the constraint they describe is gone.
