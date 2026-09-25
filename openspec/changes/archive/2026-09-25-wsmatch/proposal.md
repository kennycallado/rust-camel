# Proposal: wsmatch

## Why

camel-ws `ws_handler` resolves the `WsConnectionRegistry` for an accepted
connection by scanning `GLOBAL_CONNECTION_REGISTRIES` with a PATH-ONLY
match (`entry.key().2 == path`, lib.rs ~1083). Host and port of the
`(host, port, path)` key are ignored, so a connection accepted by server
B can be inserted into server A's registry when both serve the same
path. Blast radius (bd rc-9xzlw): `consumer.stop()` on one server closes
connections of another server sharing a path, and a producer broadcast
can reach a different port's connections.

The path-only scan cannot be upgraded to an exact-triple match: `start`
keys the map by `canonical_host()` from the URI while
`start_with_listener` keys by the listener's actual IP, so the accept
path (which only knows the bound address) cannot reconstruct the
consumer's key host reliably.

## What Changes

- `WsAppState` (per shared server) gains a server-scoped owned map
  `registries: Arc<DashMap<String, Arc<WsConnectionRegistry>>>`
  (path → registry), symmetric with the existing per-server
  `dispatch` / `path_configs` maps.
- `finish_start` inserts into BOTH the server-scoped map and the
  process-global full-key index; `stop()` removes from both. The
  global map stays the producer-facing exact-key index — unchanged.
- `ws_handler` resolves the registry from the accepting server's
  owned map by path. The process-global scan is deleted from the
  accept path.
- Regression test: two servers, same path, different host:port —
  `stop()` on one leaves the other's live connection echo-working.
  Single-server behavior pinned by the existing suite (166/166).
- Spec delta: new requirement in `tls-registry-seam` (owns
  connection-registry semantics) pinning server-scoped resolution.
- CONTEXT.md (camel-ws): `WsConnectionRegistry` vocabulary updated
  with the server-scoped resolution rule.

Excluded: the `canonical_host` vs listener-IP producer-lookup miss
noted at lib.rs ~1642 (pre-existing, separate concern); any change to
`ServerRegistry` port-keyed sharing semantics.

Affected crates: `camel-component-ws`. Ref: bd rc-9xzlw (P2),
discovered-from rc-fx3xy (tlsseam, landed dcb45b54).

## Acceptance criteria

- Two servers serving the same path on different host:port: stopping
  one consumer closes only its own connections; the other's client
  keeps echo-round-tripping.
- Producer `sendToAll` / targeted send on a single server unchanged.
- Existing camel-ws suite green; no new process-global reads on the
  accept path.
- `openspec validate wsmatch` clean.

## Risk budget

Low. Accept-path resolution narrows from "any registry with same
path anywhere in the process" to "the accepting server's registry for
that path" — strictly more correct. Risk of behavior change only for
tests that relied on cross-server path matching (none should exist —
tests were de-collided to unique paths during tlsseam exactly because
this behavior is wrong).
