# Design: jms-pool-knobs

## Config channel ruling

The mission order assumed config crosses the gRPC boundary ("extend the existing
message"). Verified against the tree: `jms_bridge.proto` has no config message —
config crosses the process boundary as spawn-time environment variables
(`BridgeProcessConfig::jms` in `crates/services/camel-bridge/src/process.rs`
pushes `BRIDGE_BROKER_URL`, `JMS_ATTEMPT_JOURNAL_DIR`, …; Java `BridgeConfig`
reads `System.getenv()` with MIN/MAX/DEFAULT constants and `bounded()`).
Pre-flight (e_opus, 2026-10-23) ruled: wire the new knobs as env vars following
the journal-tunables pattern; do not invent a proto config message. This is the
additive path in both directions: an old bridge ignores the new env vars; a new
bridge without them applies Java-side defaults.

## Rust side (crates/components/camel-jms, crates/services/camel-bridge)

`JmsPoolConfig` (config.rs) is the producer-pool config and already holds the
flat timing knobs (`bridge_start_timeout_ms`, `health_*`). Add:

- `pool_idle_timeout_ms: u64`, `pool_expiration_check_ms: u64`
- `#[serde(default = ...)]` of 30_000 each; update `Default for JmsPoolConfig`.
- `validate()` rejects `0`, `< 1000`, and `> 86_400_000` with messages naming the
  field, the accepted range, and the trade-off (short idle timeout = fewer idle
  broker connections but more reconnect churn; sweep interval = how long expired
  connections linger on the broker). Rationale for 1000 ms floor: a sub-second
  idle timeout reopens the connection on nearly every send; a sub-second sweep
  spins the evictor. 86_400_000 fits `i32::MAX`, so the Java `(int)` cast is
  safe. Java `bounded()` mirrors the same range (defense in depth, same as the
  journal knobs).

`BridgeProcessConfig::jms(...)` gains two `u64` parameters after
`start_timeout_ms` and always pushes `JMS_POOL_IDLE_TIMEOUT_MS` /
`JMS_POOL_EXPIRATION_CHECK_MS` (explicit values win over operator env —
deterministic). The constructor already carries `#[allow(clippy::too_many_arguments)]`.
Call sites to update: `component.rs` (spawn at 1344, tests at 3462/3576/3757),
`bridge_client_test.rs`, `crates/camel-test` support, `process.rs` tests.

Retention across restarts: the initial launch and the monitored replacement
launch build their process config through a separate helper
(`component.rs:691-700` capture, `1186-1210` replacement, `1305-1314` build).
Both configured values are therefore stored on `JmsBridgePool` and carried
through the initial-launch and replacement-launch captures into
`start_bridge_process` and `BridgeProcessConfig::jms`. A test asserts
60000/5000 reaches BOTH the initial and the replacement spawn environment.
Restart policy itself is unchanged.

Values are read at spawn only: a knob change takes effect when the bridge
process restarts. Documented; no hot-reload in scope.

## Java side (bridges/jms)

`BridgeConfig`: add `JMS_POOL_IDLE_TIMEOUT_MS` / `JMS_POOL_EXPIRATION_CHECK_MS`
constants with MIN=1000, MAX=86_400_000, DEFAULT=30_000 and accessors
`poolIdleTimeoutMs()` / `poolExpirationCheckMs()`. `bounded()` throws the
checked `JournalPermanentException` for non-numeric or out-of-range values
(only unset/blank selects the default); the pool accessors wrap that checked
exception in `IllegalArgumentException` preserving its safe message. `bounded()`
and journal handling remain unchanged.

`JmsClientFactory.createFactory()` — setters BEFORE `pool.start()` (pooled-jms
applies eviction settings inside `start()`; ActiveMQ may create connections on
start):

- activemq branch: `PooledConnectionFactory.setIdleTimeout(int)` (ms, ≤0
  disables) and `setTimeBetweenExpirationCheckMillis(long)` (ms).
- artemis branch: `JmsPoolConnectionFactory.setConnectionIdleTimeout(int)` and
  `setConnectionCheckInterval(long)` — different setter names from the ActiveMQ
  class; units are ms in both (pre-flight verified with javap against the
  pinned jars: activemq-jms-pool 5.19.10, pooled-jms 2.0.8).

Library-default honesty: idleTimeout defaults to 30000 in both libraries; the
expiration sweep interval defaults to -1 (no background evictor; idle-expired
connections are dropped lazily on borrow). Our explicit 30000/30000 therefore
changes sweep behavior from "library-dependent, effectively lazy" to "deterministic
30 s sweep" — exactly the predictability the owner asked for. Docs must not
claim the library default sweep is 30 s.

Setters are direct calls — no new native-image reflection entries. The evictor
thread runs in the native build for the first time; native smoke is CI's
responsibility (deferred, see tasks).

## Architectural boundaries

Config/control plane only (config parsing, validation, spawn env, pool setup).
No data-plane (send/consume) code paths, no health-path changes. Affected:
camel-jms (component crate), camel-bridge (service spawn), camel-test (support),
bridges/jms (Java). Domain spec: `specs/jms/spec.md` (ADDED requirement).

## Docs

- `docs/src/components/jms.md`: new pool-knobs section — defaults, what each
  knob does, trade-offs, camel-112 measurements, spawn-time-only caveat, and
  the sweep-default honesty note.
- `crates/components/camel-jms/CONTEXT.md`: rewrite the "Producer idleTimeout
  stays excluded (rc-w06u6)" note to the new state.

## Risk budget

Additive env plumbing; highest-risk detail is setter-name/units mismatch between
the two pool classes (covered by Java tests asserting the getters). Acceptable:
enabling the evictor thread by default (documented, owner-approved). Out of
bounds: `maxConnections`, pool swap, health, send semantics, version bumps.
