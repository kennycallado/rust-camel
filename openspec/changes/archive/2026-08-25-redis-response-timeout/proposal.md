# Proposal: redis-response-timeout

## Why

The redis 1.6.0 driver enforces `DEFAULT_RESPONSE_TIMEOUT` (500 ms) on every
command pipelined over a multiplexed connection. The component's
`MultiplexedExecutor::get_conn` builds connections with the config-less
`get_multiplexed_async_connection()`, so that 500 ms deadline governs every
command the repository service crate issues. The service crate's own 30 s
per-command backstop (commit 1945babc, ADR-0063 Decision 13) therefore never
fires:
slow-but-healthy peers (large SCAN batches, loaded networks) trip the 500 ms
driver deadline first, classify as transient, and trigger
drop/re-resolve/reconnect churn exactly when the system is stressed.
e_opus review follow-up (bd rc-dq7a, via r_glm re-review of
rc-redis-repositories).

## What Changes

- `camel-redis` (component): `MultiplexedExecutor` accepts an optional
  driver-level response timeout. Builder-style
  `with_response_timeout(Duration)`; `new(...)` keeps its signature and
  default behavior. `get_conn` — the single connection-build point used by
  initial connect, `refresh`, and `reconnect` — builds through
  `get_multiplexed_async_connection_with_config` with
  `AsyncConnectionConfig::set_response_timeout` when a value is set.
- `camel-redis-repo` (repository service crate): `connect_executor_with_topology`
  passes a driver response timeout strictly above the crate-local 30 s
  backstop, so the local backstop keeps governing error classification
  (tested transient-Io message) and the driver value remains
  defense-in-depth.

Excluded: no new `RedisEndpointConfig` field, no URI parameter, no change to
component producer/consumer default behavior, no change to `RedisCommand`
dispatch semantics, no change to the connect timeout
(`connection_timeout_secs` still bounds only the TCP connect).

## Acceptance criteria

- Executor built without `with_response_timeout` is behaviorally identical
  to today (driver default path preserved; zero call-site churn).
- Executor built with response timeout `T` against a silent peer: the
  command deadline comes from `T`, not the 500 ms driver default.
- Repo executor connections carry a driver response timeout greater than
  the local backstop; existing backstop tests keep passing unchanged.
- `refresh()` rebuilds honor the configured value (failover path).
- Public-surface test extended for the new builder; fmt/clippy gates green.

## Risk budget

Low. Additive, default-off builder on one executor; no public API break.
Timing tests run under tokio's paused clock (deterministic virtual-time
boundaries — no wall-clock margins). Out of bounds: any behavior change on
the default (unset) path, any config-surface growth.
