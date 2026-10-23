# Tasks: jms-health-stale

Scope-check: PASS. One coherent JMS cache/protocol/readiness slice, four ordered tasks, no independent subsystem or delivery-phase split. All tasks implement the modified JMS health RPC requirement. No version, producer-pool, or transport-listener changes.

## Java cache

### Task 1.1: Refresh without readers

**Files:**
- `bridges/jms/src/main/java/org/rustcamel/jms/BridgeHealthCache.java` (modified)
- `bridges/jms/src/test/java/org/rustcamel/jms/BridgeHealthCacheTest.java` (modified)

**Steps:**
1. Add an owned daemon scheduler with an injectable deterministic scheduling seam; schedule initial admission and recurring admissions every REFRESH_AFTER_MS. Retain the existing worker, CAS single-flight, generation checks, and exception handling. Scheduled callbacks only admit work.
2. Remove refresh admission from current(); retain pure snapshot age/status mapping and non-blocking behavior.
3. Cancel and shut down scheduling before existing bounded worker/provider teardown. Skip mode never admits probes; invalidation and late results retain generation fencing. Adapt existing deterministic tests to drive initial/recurring ticks explicitly where they previously used reads to trigger work.

**Tests:**
- `scheduledRefreshKeepsLongPollFreshWithoutTraffic`: arrange fake clock=0, deterministic scheduler, successful fake probe and deterministic worker completion; act read once at 0, run scheduled ticks at 5-second increments through 60 seconds with no reads/sends between, then read; assert UP, healthy=true, brokerConnected=true and more than one probe. Command `./gradlew test --tests org.rustcamel.jms.BridgeHealthCacheTest` from bridges/jms. Expected: new behavioral assertion fails before scheduling implementation and passes after.
- `scheduledProbeFailureStillReportsDown`: arrange same seams with probe success then exception/failure; act complete initial tick then next failed tick; assert DOWN, healthy=false, brokerConnected=false. Same command; fails if failures are swallowed or retained UP, passes after.
- `scheduledTicksRemainSingleFlightAndStopOnClose`: arrange blocked fake probe plus fake scheduler; act tick repeatedly, invalidate, release old probe, close, and invoke captured callback; assert max concurrent probe=1, no queue growth, no old-generation publication and no probe after close. Same command; scheduling seam initially absent, then passes. Existing close-bound, skipped-mode, unrelated-RPC and delayed-handler tests must remain green; use existing fixtures, not new test designs.

**Acceptance:**
- Long-poll regression fails on old reader-triggered behavior and passes on new scheduling; retain red/green command evidence.
- Java `./gradlew spotlessCheck test` exits 0 (under contained systemd unit); no current() probe/admission path.
- Existing BridgeHealthCache/JmsBrokerProbe/service tests pass, including bounded teardown and skip mode.

- [x] 1.1

## Protocol and readiness

### Task 2.1: Carry status and retain readiness verification

**Files:**
- `bridges/jms/src/main/proto/jms_bridge.proto` (modified)
- `crates/components/camel-jms/proto/jms_bridge.proto` (modified)
- `bridges/jms/src/main/java/org/rustcamel/jms/HealthSnapshot.java` (modified)
- `bridges/jms/src/test/java/org/rustcamel/jms/BridgeHealthCacheTest.java` (modified)
- `crates/components/camel-jms/src/health.rs` (modified)
- `crates/components/camel-jms/src/component.rs` (modified; compile-only response literal defaults in this task)
- `crates/components/camel-jms/src/consumer.rs` (modified; response literal defaults)
- `crates/components/camel-jms/src/bridge_client_test.rs` (modified; response literal defaults)

**Steps:**
1. Add identical HealthStatus enums UNSPECIFIED=0, UP=1, STALE=2, UNKNOWN=3, SKIPPED=4, DOWN=5 in both proto files; add status and uint64 age_ms with unused field numbers, preserving all old fields.
2. Serialize every HealthSnapshot status explicitly; add a source-compatible stale-age representation so stale(ageMillis) maps numeric age_ms without parsing message text. Other statuses report zero age.
3. In health.rs introduce crate-local `BrokerHealthObservation` and `BrokerHealthMemory` types shared with the next task. Observation represents raw status/booleans/numeric age; memory retains verified UP/DOWN and last observation kind. Map UNSPECIFIED via old booleans, future status numbers as unverified, and never derive verification from message text. Preserve last verification on unverified and transport errors.
4. Extend BridgeProbeResult and real response mapping. Serialize readiness checks and reset memory on observed slot absence, not-ready lifecycle, or Ready-channel replacement. Use existing slot/watch identity to detect replacement; never hold a DashMap guard across await. Test seam must represent replacement explicitly. Update existing fake result constructors and generated-response literals to compile without changing legacy semantics.
5. Return Healthy after verified UP followed by unverified; Unhealthy after DOWN followed by unverified; Degraded before verification; RPC errors/timeouts remain Unhealthy. No debounce and no restart changes.

**Tests:**
- `snapshotResponseCarriesExplicitStatusAndAge`: arrange UNKNOWN, SKIPPED, STALE(age=60000), UP, DOWN snapshots; act serialize; assert each enum/old boolean pair and only STALE age_ms=60000. Command `./gradlew test --tests org.rustcamel.jms.BridgeHealthCacheTest`; new fields initially fail to compile, then passes.
- `jms_health_stale_keeps_last_verified_state`: arrange sequential fake responses UP, STALE(60000), STALE(120000), UNKNOWN, SKIPPED, DOWN, STALE; act check each; assert Healthy for first five then Unhealthy for last two, with no stale-only Degraded. Command `cargo test -p camel-component-jms --lib health::tests`; record failing behavioral assertion before retention implementation, then green. This and task 1.1 long-poll test are the two primary regression proofs.
- `jms_health_unverified_startup_and_replacement_are_degraded`: arrange unverified initial status, then UP, then changed bridge identity and STALE; act checks; assert Degraded, Healthy, Degraded; exercise absent/not-ready reset and new Ready observation. Same command; fails before identity/reset implementation, passes after.
- `jms_health_legacy_future_status_and_rpc_errors`: arrange each UNSPECIFIED boolean pair and future numeric status, plus UP followed by RPC error and timeout then STALE; act check; assert legacy true/true Healthy, true/false Degraded, false/false Unhealthy; future status preserves verification or is Degraded initially; RPC errors/timeouts Unhealthy and subsequent STALE retains previous UP. Same command; initially new status cases fail, then pass.

**Acceptance:**
- Both proto copies byte-identical; no field renumbering or bridge version changes.
- Java serialization tests and all Rust JMS lib tests pass; fmt/clippy for camel-component-jms exit 0.
- `/readyz` existing CheckResult mapping remains unchanged: Healthy/Degraded HTTP 200, Unhealthy HTTP 503; readiness tests assert these HealthStatus variants without changing generic service code.

- [x] 2.1

## Monitor

### Task 3.1: Report status transitions without false disconnections

**Files:**
- `crates/components/camel-jms/src/component.rs` (modified)
- `crates/components/camel-jms/src/health.rs` (modified; shared mapping adjustments only)

**Steps:**
1. Feed HealthResponse status/age/booleans through BrokerHealthObservation and BrokerHealthMemory in BrokerStatusReporter; keep existing legacy tests and test transitions compatible.
2. Track observation kind separately from retained verification. Emit fixed credential-safe transition message with numeric age_ms once on entering STALE, not when age alone changes. Explicit unverified never logs a new disconnected broker; before verification report unverified, not verified DOWN.
3. Reset memory for replacement monitor/channel; preserve successful-RPC liveness counters, restart threshold and lifecycle rules unchanged. Expose only minimal cfg(test) transition evidence needed by tests.

**Tests:**
- `monitor_stale_preserves_verified_state_and_logs_once`: arrange reporter memory and UP, STALE(age=60000), STALE(age=120000), UNKNOWN, SKIPPED, DOWN, STALE responses; act through reporter; assert last verification stays UP through first unverified run and DOWN after failure, only one stale-entry transition for consecutive STALE, numeric first stale age present, no false BrokerDown transition, and no raw provider message/credential in captured event fields. Command `cargo test -p camel-component-jms --lib monitor_stale_preserves_verified_state_and_logs_once`; initially fails or status API absent, passes after.
- `monitor_status_resets_for_replacement`: arrange old UP reporter then new monitor/channel epoch with STALE; act report replacement response; assert no inherited verified UP and unverified transition, zero restart attempts on completed RPCs. Command `cargo test -p camel-component-jms --lib monitor_status_resets_for_replacement`; initially fails, passes after.
- Existing `t1`, `t2`, `t6` monitor tests: arrange existing scripted server fixtures; act run unchanged cases; assert failed broker observations and repeated stale responses never restart while RPC errors restart exactly at existing threshold. Command `cargo test -p camel-component-jms --lib`; expected green before and after.

**Acceptance:**
- Stale-age changes alone produce no repeated transition log; credential-safe captured logging test passes.
- Existing liveness threshold behavior remains tested and unchanged; no broker-failure debounce.
- JMS lib tests, cargo fmt --check and cargo clippy -p camel-component-jms -- -D warnings exit 0.

- [x] 3.1

## Operator contract

### Task 4.1: Record freshness and compatibility decisions

**Files:**
- `docs/adr/0086-jms-liveness-and-broker-health.md` (modified)
- `docs/src/components/jms.md` (modified)
- `crates/components/camel-jms/CONTEXT.md` (modified)
- `CONTEXT-MAP.md` (modified)

**Steps:**
1. Amend ADR-0086 to distinguish unverified startup from unverified observations after verification; document reset, RPC failures, scheduler, status/age fields and legacy fallback. Mark previous stale-always-Degraded table semantics superseded rather than contradicting them.
2. Align guide and architecture context with readiness retention and additive compatibility; keep healthy versus broker_connected meanings. State new explicit-status guarantees need matching bridge implementation, with no version bump in this mission.
3. Record consumer transport-listener integration as a future generation-fenced cache input, not implemented. Cite rc-w06u6 for producer idleTimeout exclusion. No new operational knobs, debounce, or implied live-native verification.

**Tests:**
- `jms_health_documentation_contract`: arrange final modified docs and code; act run `mdbook test` from docs and `cargo xtask lint-context-citations`; assert both exit 0 and manually check all stale/readiness tables match startup Degraded, UP retention Healthy, DOWN retention Unhealthy, reset and legacy rules. Expected: mechanical commands may pass before changes; obsolete semantic statements removed after.

**Acceptance:**
- mdbook test and lint-context-citations exit 0; English prose and valid ADR links.
- No changed version registry, producer pool, generic readiness service, or implemented consumer listener.

- [x] 4.1
