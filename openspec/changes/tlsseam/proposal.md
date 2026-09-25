# Proposal: tlsseam

## Why

camel-ws lib tests serialize on one `REGISTRY_TEST_LOCK`: the fleet
audit documented a 602s lock chain (43 holders x ~14s under fleet
load), rooted in process-global registries. `ServerRegistry::global()`
(OnceLock) aborts ALL servers on test-only `reset()`, so every test
with a running server must hold the lock for its whole duration.
`TlsReloadRegistry::global()` (camel-component-api) is fed by
`get_or_spawn` production code, piling every test's handlers into one
process singleton. c2a48f20 bounded each wait (900s deadline) — it did
NOT remove the queue. bd rc-fx3xy (owner-sealed rc-p823t Option C +
seams: registries stay process-global in production; tests get a seam).

## What Changes

- `camel-component-api`: `TlsReloadRegistry` gains a dual-handle global
  (`global()` unchanged signature, plus `global_arc()` returning an
  `Arc` to the SAME instance) so an `Arc` handle can ride a struct
  field without breaking the 20+ existing `global()` callers.
- `camel-component-ws`: `ServerRegistry` owns an
  `Arc<TlsReloadRegistry>` (production default: the process global;
  isolated instances: fresh `TlsReloadRegistry`). Methods drop the
  vestigial `&'static self` bounds. `reset()`/test accessors become
  instance methods. `WsConsumer` gets constructor injection
  (`with_server_registry`); `new()` defaults to the global.
- camel-ws lib tests migrate to isolated instances; the
  `REGISTRY_TEST_LOCK` chain is removed for migrated tests.
- Spec delta: seam contract documented under `tls-registry-seam`.
- Suite wall-clock measured before/after; numbers recorded in bd.

Excluded (follow-up, not this mission): repo-wide sweep of the 88
env/global sites, `GLOBAL_CONNECTION_REGISTRIES` restructuring
(consumer-keyed, no reset-all semantics, port-ephemeral keys —
parallel-safe as-is), grpc/http/core test migration.

## Acceptance criteria

- Isolated `ServerRegistry` + `TlsReloadRegistry` instances run
  camel-ws server tests with zero mutation of the process-global
  `ServerRegistry` and `TlsReloadRegistry` (no cross-test abort, no
  shared handler pile). Connection-registry publishes/lookups
  (`GLOBAL_CONNECTION_REGISTRIES`, consumer-keyed with ephemeral
  ports) remain as existing behavior — out of scope.
- Production behavior identical: `TlsReloadRegistry::global()` and
  `global_arc()` are the same instance; reload flow (runtime_bus ->
  `global().find()`) still finds handlers registered through
  production `get_or_spawn`.
- Migrated camel-ws tests hold no cross-test lock; `acquire_deadline`
  count in camel-ws lib tests drops to the sites that genuinely
  exercise the lockdeadline contract.
- Gates: fmt, clippy (4 legs), camel-ws + camel-core tests,
  lint-unbounded-wait (ratchet 296), lint-cancel-tokens, schema-check,
  doc-build — all green.
- Before/after wall-clock (quiet + single-CPU) recorded in bd rc-fx3xy.

## Risk budget

Acceptable: mechanical signature churn inside camel-ws (private/`pub`
method bounds, `&'static` -> `&`); test-only accessor moves. Out of
bounds: any change to production reload semantics, any new dependency,
any change to `TlsReloadRegistry::global()` signature (would break
grpc/http/core callers), touching the 88 out-of-scope sites.
