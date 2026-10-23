# Tasks: jms-pool-knobs

## Rust config: camel-jms

### Task 1.1: Add pool knob fields with defaults and validation to JmsPoolConfig

**Files:**
- `crates/components/camel-jms/src/config.rs` (modified)

**Steps:**
1. Add `default_pool_idle_timeout_ms()` and `default_pool_expiration_check_ms()` free functions returning `30_000u64`, next to `default_bridge_start_timeout_ms()` (line ~126).
2. Add fields to `struct JmsPoolConfig` after `bridge_start_timeout_ms` (line ~612): `#[serde(default = "default_pool_idle_timeout_ms")] pub pool_idle_timeout_ms: u64` and `#[serde(default = "default_pool_expiration_check_ms")] pub pool_expiration_check_ms: u64`.
3. Initialize both in `impl Default for JmsPoolConfig` (line ~632).
4. In `JmsPoolConfig::validate()` (line ~671), after the `bridge_start_timeout_ms` check, add per-field checks for `pool_idle_timeout_ms` and `pool_expiration_check_ms`: reject `0`, reject `< 1000`, reject `> 86_400_000`, each returning `CamelError::Config` (these are top-level fields — match the `max_bridges` convention, not the broker-scoped `ProcessorError`) with a message naming the field, the accepted range `1000..=86400000 ms`, and the trade-off: for the idle timeout "shorter holds fewer idle connections on the broker but reopens more often on slow cadences", for the check interval "this is how long expired connections linger before the sweep removes them".
5. Run `cargo fmt --check --all` and `cargo clippy -p camel-component-jms -- -D warnings`; fix findings.

**Tests:** (in `config.rs` `#[cfg(test)] mod tests`, following the `validate_rejects_zero_bridge_start_timeout` pattern at line ~1142; each Err assertion also checks the error is the `CamelError::Config` variant)
- `validate_rejects_zero_pool_idle_timeout`: base valid config → set `pool_idle_timeout_ms = 0` → `validate()` returns Err of the `CamelError::Config` variant whose message contains `pool_idle_timeout_ms` and the full range `1000..=86400000` (both endpoints present).
- `validate_rejects_sub_second_pool_idle_timeout`: base valid config → set `pool_idle_timeout_ms = 999` → `CamelError::Config` naming `pool_idle_timeout_ms` and the full range.
- `validate_rejects_above_max_pool_idle_timeout`: base valid config → set `pool_idle_timeout_ms = 86_400_001` → `CamelError::Config` naming `pool_idle_timeout_ms` and the full range.
- `validate_rejects_zero_pool_expiration_check`: base valid config → set `pool_expiration_check_ms = 0` → `CamelError::Config` naming `pool_expiration_check_ms` and the full range.
- `validate_rejects_sub_second_pool_expiration_check`: base valid config → set `pool_expiration_check_ms = 999` → `CamelError::Config` naming `pool_expiration_check_ms` and the full range.
- `validate_rejects_above_max_pool_expiration_check`: base valid config → set `pool_expiration_check_ms = 86_400_001` → `CamelError::Config` naming `pool_expiration_check_ms` and the full range.
- `validate_accepts_pool_knob_endpoints`: base valid config → set both fields to `1000`, assert Ok; set both to `86_400_000`, assert Ok.
- `pool_knob_serde_defaults`: `JmsPoolConfig::default()` has both fields `== 30_000`, and `serde_json::from_str::<JmsPoolConfig>("{}")`-style deserialization of a config omitting both fields yields `30_000` for both (use the same test-serialization approach the file already uses for defaults).

**Acceptance:**
- `cargo test -p camel-component-jms --lib` exits 0.
- `cargo clippy -p camel-component-jms -- -D warnings` exits 0; `cargo fmt --check --all` exits 0.

- [x] 1.1

## Spawn plumbing: camel-bridge + camel-jms

### Task 2.1: Pass the knobs through BridgeProcessConfig::jms as env vars

**Files:**
- `crates/services/camel-bridge/src/process.rs` (modified)
- `crates/components/camel-jms/src/component.rs` (modified — call sites only)
- `crates/components/camel-jms/src/bridge_client_test.rs` (modified — call site only)
- `crates/camel-test/tests/support/jms_bridge_procs.rs` (modified — call site only)

**Steps:**
1. In `BridgeProcessConfig::jms` (process.rs line ~344), add two `u64` parameters `pool_idle_timeout_ms` and `pool_expiration_check_ms` after `start_timeout_ms`, and push `(format!("JMS_POOL_IDLE_TIMEOUT_MS"), pool_idle_timeout_ms.to_string())` and `("JMS_POOL_EXPIRATION_CHECK_MS", pool_expiration_check_ms.to_string())` into `env_vars` unconditionally (plain string literals like the existing `BRIDGE_BROKER_URL` entry).
2. Update every call site of `BridgeProcessConfig::jms` to pass the two new args. In `component.rs` `start_bridge_process` (line ~1344, which has no config access yet), pass temporary literals `30_000, 30_000` — Task 2.2 replaces them with the configured parameters. All test-only call sites pass `30_000, 30_000` unless the test asserts knob values: component.rs ~3462/~3576/~3757, bridge_client_test.rs:1088, camel-test/tests/support/jms_bridge_procs.rs:116, process.rs tests at ~913/~935/~955/~1119/~1580/~1596/~1670. (The direct test caller of `start_bridge_process` at component.rs:3648 is NOT touched here — its signature changes in Task 2.2.)
3. Run `cargo fmt --check --all` and `cargo clippy -p camel-bridge -p camel-component-jms -p camel-test --all-targets -- -D warnings`; fix findings.

**Tests:** (process.rs `#[cfg(test)]`, following the existing env-var assertions near line ~935)
- `jms_pool_knob_env_vars_present`: build `BridgeProcessConfig::jms` with the binary path and broker fields used by the existing tests near line ~913, passing `60_000` and `5_000` as the two new trailing `u64` args → inspect `config.env_vars` → contains exactly `("JMS_POOL_IDLE_TIMEOUT_MS", "60000")` and `("JMS_POOL_EXPIRATION_CHECK_MS", "5000")` among the entries.

**Acceptance:**
- `cargo test -p camel-bridge` exits 0; `cargo test -p camel-component-jms --lib` exits 0; `cargo check -p camel-test --tests` exits 0.
- `cargo clippy -p camel-bridge -p camel-component-jms -p camel-test --all-targets -- -D warnings` exits 0.

- [x] 2.1

### Task 2.2: Retain the knobs across bridge restarts in JmsBridgePool

**Files:**
- `crates/components/camel-jms/src/component.rs` (modified)

**Steps:**
1. Add `pool_idle_timeout_ms: u64` and `pool_expiration_check_ms: u64` to `struct JmsBridgePool` (line ~460), populated from `JmsPoolConfig` at construction (follow how `bridge_start_timeout_ms` reaches the pool struct; adapt construction sites).
2. Add the two `u64` parameters to `async fn start_bridge_process` (line ~1305, already `#[allow(clippy::too_many_arguments)]`), after `start_timeout_ms`, forwarding them to `BridgeProcessConfig::jms` (line ~1344) — replacing Task 2.1's temporary `30_000, 30_000` literals.
3. Copy both new pool fields into the owned monitor captures next to `start_timeout_ms` (~lines 981-988) and use those captures in BOTH replacement arms (~1186-1210); the initial launch path (~691) reads the pool fields directly. Also update the direct test caller of `start_bridge_process` at component.rs:3648. Restart policy and replacement logic stay untouched.
4. Run `cargo fmt --check --all` and `cargo clippy -p camel-component-jms --all-targets -- -D warnings`; fix findings.

**Tests:** (component.rs test module; model the harness on the existing `start_bridge_exports_health_probe_mode` test at ~3624-3672, which drives `start_bridge_process` directly with a stub bridge and inspects the spawned child's environment — `EVENT_CAPTURE_CHILD_ENV` (~4688) is tracing-event capture, not child-env capture)
- `pool_knobs_reach_initial_and_replacement_spawn_env`: stub bridge process records the environment of every launch → drive an initial launch through pool acquisition, then a replacement launch with an empty replacement-outcome queue, synchronizing on the pool's bounded `replacement_processed` watch/state and cleaning up monitor + child → assert the recorded child environment of BOTH launches contains `JMS_POOL_IDLE_TIMEOUT_MS=60000` and `JMS_POOL_EXPIRATION_CHECK_MS=5000`.
- `pool_knob_defaults_reach_initial_spawn_env`: configure the pool with default `JmsPoolConfig` (no explicit knobs) → drive an initial launch with the same stub → assert the recorded child environment contains `JMS_POOL_IDLE_TIMEOUT_MS=30000` and `JMS_POOL_EXPIRATION_CHECK_MS=30000`.

**Acceptance:**
- `cargo test -p camel-component-jms --lib` exits 0 including the new test.
- `cargo clippy -p camel-component-jms --all-targets -- -D warnings` exits 0.

- [x] 2.2

## Java bridge: bridges/jms

### Task 3.1: Parse the pool env vars in BridgeConfig with mirrored bounds

**Files:**
- `bridges/jms/src/main/java/org/rustcamel/jms/BridgeConfig.java` (modified)
- `bridges/jms/src/test/java/org/rustcamel/jms/BridgeConfigPoolKnobsTest.java` (new)

**Steps:**
1. Add constants next to the journal constants (line ~42): `static final long POOL_IDLE_TIMEOUT_MS_MIN = 1_000L`, `POOL_IDLE_TIMEOUT_MS_MAX = 86_400_000L`, `POOL_IDLE_TIMEOUT_MS_DEFAULT = 30_000L`; the same MIN/MAX/DEFAULT trio for `POOL_EXPIRATION_CHECK_MS`.
2. Add env constants: `static final String POOL_IDLE_TIMEOUT_MS_ENV = "JMS_POOL_IDLE_TIMEOUT_MS"`, `POOL_EXPIRATION_CHECK_MS_ENV = "JMS_POOL_EXPIRATION_CHECK_MS"`.
3. Add accessors `public long poolIdleTimeoutMs()` and `public long poolExpirationCheckMs()` following the `journalRetentionMs()` pattern (line ~186): `env.get(...)` → `bounded(POOL_IDLE_TIMEOUT_MS_ENV, raw, MIN, MAX, DEFAULT)`, wrapping the checked `JournalPermanentException` in `IllegalArgumentException` with the original (safe, redaction-free) message.
4. Run `./gradlew test` from `bridges/jms`; fix findings.

**Tests:** (new `BridgeConfigPoolKnobsTest`, modeled on `BridgeConfigHealthProbeModeTest.java`, using the explicit-values `BridgeConfig(Map)` constructor; every test parameterizes over BOTH variables/accessors — `JMS_POOL_IDLE_TIMEOUT_MS`/`poolIdleTimeoutMs()` and `JMS_POOL_EXPIRATION_CHECK_MS`/`poolExpirationCheckMs()`)
- `poolKnobEndpointsAccepted`: for each variable, map with the value `1000` then `86400000` → the accessor returns exactly the supplied value.
- `poolKnobUnsetOrBlankSelectsDefault`: map without both keys, then map with each variable set to `" "` → both accessors return `30_000L`.
- `poolKnobBelowMinRejected`: for each variable, value `999` → the accessor throws `IllegalArgumentException` whose message contains the variable name and BOTH range endpoints (`1000` and `86400000`).
- `poolKnobAboveMaxRejected`: for each variable, value `86400001` → throws `IllegalArgumentException` with the same message contract.
- `poolKnobNonNumericRejected`: for each variable, value `abc` → throws `IllegalArgumentException` with the same message contract.

**Acceptance:**
- `./gradlew test --tests 'org.rustcamel.jms.BridgeConfigPoolKnobsTest'` exits 0 (run from `bridges/jms`).
- Full `./gradlew test` exits 0.

- [x] 3.1

### Task 3.2: Apply the pool knobs in JmsClientFactory on both broker branches

**Files:**
- `bridges/jms/src/main/java/org/rustcamel/jms/JmsClientFactory.java` (modified)
- `bridges/jms/src/test/java/org/rustcamel/jms/JmsClientFactoryPoolKnobsTest.java` (new)

**Steps:**
1. In `createFactory()` (line ~139), activemq branch: before `pool.start()`, add `pool.setIdleTimeout((int) config.poolIdleTimeoutMs());` and `pool.setTimeBetweenExpirationCheckMillis(config.poolExpirationCheckMs());`.
2. Artemis branch: before `pool.start()`, add `pool.setConnectionIdleTimeout((int) config.poolIdleTimeoutMs());` and `pool.setConnectionCheckInterval(config.poolExpirationCheckMs());` (different setter names from the ActiveMQ class — pre-flight verified via javap).
3. Extract the pool construction into package-private static builders mirroring the `buildActiveMqFactory` testability seam (line ~177): `static PooledConnectionFactory buildPooledActiveMqFactory(ActiveMQConnectionFactory cf, BridgeConfig config)` and `static JmsPoolConnectionFactory buildPooledArtemisFactory(ConnectionFactory cf, BridgeConfig config)`, each applying credentials-free pool setup exactly as the branch does today (maxConnections 5, the two new setters, start). `createFactory()` delegates to them.
4. Run `./gradlew test` from `bridges/jms`; fix findings.

**Tests:** (new `JmsClientFactoryPoolKnobsTest`, modeled on `JmsClientFactoryTlsTest.java`'s pool handling; construct a real `BridgeConfig(Map)` with values 60000/5000)
- `activeMqPoolReceivesKnobs`: `buildPooledActiveMqFactory(cf, config)` with `JMS_POOL_IDLE_TIMEOUT_MS=60000`, `JMS_POOL_EXPIRATION_CHECK_MS=5000` → assert `pool.getIdleTimeout() == 60000` and `pool.getTimeBetweenExpirationCheckMillis() == 5000L` (stop the pool in a finally block).
- `artemisPoolReceivesKnobs`: `buildPooledArtemisFactory(cf, config)` with the same map → assert `pool.getConnectionIdleTimeout() == 60000` and `pool.getConnectionCheckInterval() == 5000L` (stop the pool in a finally block).
- If a getter asserted above does not exist on the pinned dependency version: STOP and report `test-design-gap: <getter>` instead of weakening the assertion.

**Acceptance:**
- `./gradlew test --tests 'org.rustcamel.jms.JmsClientFactoryPoolKnobsTest'` exits 0 (run from `bridges/jms`).
- Full `./gradlew test` exits 0.

- [x] 3.2

## Docs

### Task 4.1: Document the pool knobs and update the excluded-knobs note

**Files:**
- `docs/src/components/jms.md` (modified)
- `crates/components/camel-jms/CONTEXT.md` (modified)

**Steps:**
1. In `docs/src/components/jms.md`, add a "Producer pool idle knobs" section near the existing bridge-pool configuration prose (the section that explains `max_bridges`/`health_check_interval_ms`, ~line 95-177): state `pool_idle_timeout_ms` (default 30000) = how long an idle pooled producer connection is kept before the pool drops it; `pool_expiration_check_ms` (default 30000) = how often the pool sweeps for expired connections, i.e. how long an expired connection can linger on the broker; accepted range 1000-86400000 ms with the validation error behavior; the trade-offs (shorter idle timeout = fewer idle connections on the broker but more reconnect churn on slow send cadences; longer = fewer reconnects but more idle broker connections).
2. State the camel-112 observations without over-claiming causality: against ActiveMQ 5.17.6 the producer pool was observed reopening connections when the send cadence exceeded the idle timeout, and expired connections were observed leaving the broker in batches. State that values apply when the bridge process starts (a config change takes effect at the next bridge spawn, not on a live bridge).
3. State the honesty note: the pinned libraries default the sweep interval to disabled (-1, lazy cleanup on borrow); the bridge deliberately sets a 30000 ms sweep so the cadence is explicit and predictable, and applying these knobs requires the updated Java bridge (older bridge binaries ignore the env vars).
4. In `crates/components/camel-jms/CONTEXT.md`, rewrite the note at line ~183 that begins `Producer idleTimeout stays excluded (rc-w06u6)` so it states that the producer pool idle timeout and expiration check are now configurable pool knobs wired through the spawn environment (rc-w06u6), keeping the sentence about a future consumer transport-listener integration intact.
5. Run `cd docs && mdbook test` and confirm it passes; run `RUSTDOCFLAGS="-D warnings" cargo doc -p camel-component-jms --no-deps`.

**Tests:**
- Documentation-only change; verification is the two commands in step 5 plus `grep` checks: `grep -c 'pool_idle_timeout_ms' docs/src/components/jms.md` returns ≥ 3 and `grep -c 'idleTimeout stays excluded' crates/components/camel-jms/CONTEXT.md` returns 0.

**Acceptance:**
- `cd docs && mdbook test` exits 0; `RUSTDOCFLAGS="-D warnings" cargo doc -p camel-component-jms --no-deps` exits 0.
- Both grep checks above hold.

- [x] 4.1
