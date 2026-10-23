# Proposal: jms-pool-knobs

## Why

camel-112 (ActiveMQ 5.17.6, bridge 0.8.4) observed connection churn on slow
send cadences and batched broker connection-count drops; these observations do
not establish sweep causality. The pinned libraries default idle timeout to
30000 ms and expiration sweep to -1 (disabled). `JmsClientFactory` sets only
`maxConnections(5)` and exposes neither knob. Their broker is client-owned, so
the default behavior must be predictable and tunable.
Owner decision 2026-10-23 (bd rc-w06u6): option A — expose the knobs, keep
30000/30000 as defaults; keeping 30000 for the sweep deliberately enables
periodic eviction.

## What Changes

- `JmsPoolConfig` (crates/components/camel-jms) gains `pool_idle_timeout_ms` and
  `pool_expiration_check_ms`, defaults 30000 each, validated to [1000, 86_400_000].
- `BridgeProcessConfig::jms` (crates/services/camel-bridge) passes both values to
  the bridge process as `JMS_POOL_IDLE_TIMEOUT_MS` / `JMS_POOL_EXPIRATION_CHECK_MS`.
  This is the additive config channel; the proto carries no config message and
  gains none (pre-flight ruling e_opus 2026-10-23).
- Java `BridgeConfig` reads both env vars with mirrored bounds. `JmsClientFactory`
  applies them before `pool.start()` on both branches: ActiveMQ
  `PooledConnectionFactory.setIdleTimeout` / `.setTimeBetweenExpirationCheckMillis`;
  Artemis `JmsPoolConnectionFactory.setConnectionIdleTimeout` /
  `.setConnectionCheckInterval`.
- Docs: the JMS guide documents defaults, semantics, the camel-112 measurements,
  and the trade-offs. `camel-jms/CONTEXT.md` drops its "idleTimeout stays
  excluded" note.

Behavior honesty: the pinned libraries default the sweep interval to disabled
(-1, lazy cleanup on borrow); our explicit 30000 default turns the sweep on at a
predictable 30 s cadence. Docs and bd record this as the deliberate
predictability change, not as "making an existing default explicit".

Excluded (non-goals): `maxConnections` default, pool implementation swap,
health-path changes, producer send semantics, diagnostic probe connection,
`bridges/versions.toml` / `BRIDGE_VERSION` bumps.

## Acceptance criteria

- New config fields default to 30000 and reach the bridge spawn env verbatim
  (Rust test), including the serde defaults path.
- For each new field, `validate()` rejects 0, 999, and 86400001 with an
  actionable message naming the field and the accepted range, and accepts the
  inclusive endpoints 1000 and 86400000; Java parsing mirrors this (both
  endpoints accepted, out-of-range/non-numeric rejected with an error naming
  the variable and range, unset/blank selects 30000).
- A Java test asserts both pool branches receive the supplied values.
- Both the initial and the replacement (restart) bridge spawn carry the
  configured values.
- JMS guide states defaults, knob semantics, trade-offs, the camel-112
  observations, the -1 library sweep default with deliberate 30000 enablement,
  and the updated-bridge requirement; CONTEXT.md no longer claims idleTimeout
  is excluded.

## Risk budget

Additive and spawn-time only; old bridge binaries ignore the new env vars and
new binaries fall back to defaults without them. Acceptable: the sweep-interval
default enables a previously-absent evictor thread (documented). Out of bounds:
any change to pool sizing, health, or send semantics.

Affected crates: camel-jms, camel-bridge (process spawn), camel-test (test
support call sites), bridges/jms (Java). Bd: rc-w06u6.
