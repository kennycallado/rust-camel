## ADDED Requirements

### Requirement: JMS automatic recovery depends only on consecutive liveness failures

The engine SHALL treat any successful health RPC response as live, independent of `healthy` and `brokerConnected`. It SHALL restart through the existing recovery path only after `health_failure_threshold` consecutive failed liveness probes. A failed probe is an RPC error or expiration of `health_timeout_ms`. Successful responses SHALL reset the counter. Counters SHALL be local to the slot and reset for a replacement bridge. Shutdown and terminal-state guards SHALL continue to prevent respawn.

Transport-error restart requests from producers or consumers SHALL request liveness verification rather than bypass this threshold. Business/broker failures SHALL NOT count as liveness failures. Initial launch failure recovery remains separate from restarting a running process.

#### Scenario: T1 broker failure is not process failure

- **GIVEN** a ready bridge with an attached consumer
- **WHEN** successful health RPCs report `healthy=false` and `brokerConnected=false`
- **THEN** no restart SHALL occur, the consumer SHALL remain attached, and a credential-safe broker-status signal SHALL be emitted
- **AND** the broker fields SHALL NOT move the lifecycle state into its restart-triggering state

#### Scenario: T2 threshold and recovery reset

- **GIVEN** a configured threshold N greater than one
- **WHEN** one liveness probe times out
- **THEN** the engine SHALL NOT restart
- **WHEN** a successful response follows and then N consecutive probes fail
- **THEN** the earlier failure SHALL NOT contribute and recovery SHALL begin only on the Nth consecutive failure

#### Scenario: T3 configured deadline

- **GIVEN** a fake health server with a controlled response delay and a configured timeout
- **WHEN** the delay exceeds that timeout
- **THEN** the probe SHALL count as a liveness failure
- **WHEN** the response completes within that timeout
- **THEN** it SHALL reset the liveness failure count regardless of broker status

#### Scenario: A genuinely dead bridge still recovers

- **GIVEN** a bridge whose health RPC cannot complete successfully
- **WHEN** the configured number of consecutive probes fails
- **THEN** the existing bounded restart path SHALL replace that bridge unless shutdown or a terminal guard forbids it

#### Scenario: A transport restart request cannot bypass liveness policy

- **GIVEN** a live bridge whose producer channel refresh failed
- **WHEN** a caller requests a slot restart
- **THEN** the request SHALL NOT restart the process before the configured consecutive liveness-failure threshold is met
- **AND** a successful health response SHALL preserve that bridge

### Requirement: JMS health RPC does not wait for broker connectivity

The Java bridge SHALL return a cached broker-status snapshot without waiting for broker connect, start, inspection, or close. Stale status MAY schedule one background refresh. The refresh mechanism SHALL have one owned worker, at most one in-flight refresh, and no accumulating refresh queue. It SHALL NOT hold a service-wide lock across broker I/O. Unknown or skipped status SHALL be explicit and SHALL NOT claim verified broker connectivity.

The bridge SHALL use the following response contract. The engine readiness adapter SHALL preserve the distinction between live-but-unverified and verified broker failure. A health RPC failure remains readiness `Unhealthy`; readiness does not itself request a process restart.

| Broker status | healthy | brokerConnected | Message | Engine readiness |
| --- | --- | --- | --- | --- |
| Verified up | true | true | ok | Healthy (200) |
| Verified down | false | false | Credential-safe failure summary | Unhealthy (503) |
| Not yet checked | true | false | broker status not yet checked | Degraded (200) |
| Skipped | true | false | broker probe skipped; connectivity unverified | Degraded (200) |
| Stale | true | false | broker status stale; age in milliseconds | Degraded (200) |

A cached result or in-flight refresh older than `HEALTH_TTL_MS` (10000 ms) SHALL be reported as stale even if the worker never returns. Every status writer, including successful `send()` calls, SHALL use the same atomic generation-checked publication path. No late send or refresh result SHALL overwrite a newer generation.

#### Scenario: Unknown and skipped do not make a live bridge NotReady

- **GIVEN** a not-yet-checked, skipped, or stale broker status
- **WHEN** the engine reads health
- **THEN** it SHALL report readiness `Degraded` (200), with live/unverified fields from the table
- **AND** an older engine reading only `healthy` SHALL receive true

#### Scenario: A hung refresh cannot preserve a false fresh status

- **GIVEN** a previously verified-up broker and a refresh blocked indefinitely
- **WHEN** the snapshot or refresh age exceeds 10000 ms
- **THEN** health SHALL promptly return stale/unverified with its age, not a fresh ok result
- **AND** no additional refresh worker SHALL be created

#### Scenario: A late send cannot resurrect old broker status

- **GIVEN** a send initiated in a generation that reset or shutdown invalidates
- **WHEN** the send completes successfully
- **THEN** its health update SHALL NOT overwrite the current generation's snapshot

#### Scenario: T5 slow probe does not hold RPCs hostage

- **GIVEN** a broker check blocked behind a deterministic test latch
- **WHEN** another health RPC and an unrelated RPC execute before the latch is released
- **THEN** both SHALL complete without waiting for that broker check
- **AND** repeated health calls SHALL NOT spawn additional workers or queue repeated refreshes

#### Scenario: T6 delayed broker remains live

- **GIVEN** paired contract tests, not a live-broker or native-image soak
- **WHEN** the Java real health handler uses a blocked fake broker factory for at least three configured probe periods
- **THEN** every health call SHALL complete within `health_timeout_ms` while connection opens remain flat
- **WHEN** the Rust real monitor probes a promptly responding fake bridge with those response states for at least three periods
- **THEN** the restart count SHALL be zero
- **AND** reports SHALL identify this as paired contract coverage, with live-broker/native verification deferred to CI

### Requirement: JMS broker probes reuse resources or skip broker I/O

In `reuse` mode the bridge SHALL reuse a retained live diagnostic connection across health refreshes, including provider-native initialization safeguards. It SHALL NOT open a fresh broker connection for every check. After connection failure it MAY replace that connection no more frequently than the cache refresh cadence and under the single-flight guard. In `skip` mode health probing SHALL perform no broker I/O and open no broker connection. Probe failures SHALL NOT tear down consumer connections or reset the producer pool.

#### Scenario: T4 repeated health checks keep broker opens flat

- **GIVEN** a fake connection factory that counts connection opens in `reuse` mode
- **WHEN** consecutive health refreshes run after the first successful connection, including cache-expired checks
- **THEN** each refresh SHALL use the same retained connection and the open count SHALL remain one

#### Scenario: Skip mode opens no diagnostic connection

- **GIVEN** `health_broker_probe_mode=skip`
- **WHEN** repeated health calls and refresh opportunities occur
- **THEN** the health subsystem SHALL open zero connections and report connectivity as unverified

#### Scenario: Teardown cannot publish stale results

- **GIVEN** an in-flight broker refresh
- **WHEN** bridge reset or shutdown invalidates its generation
- **THEN** a late result SHALL NOT replace the current status or retain a new connection
- **AND** owned connections and the refresh worker SHALL receive bounded cleanup without closing active consumer resources on an ordinary probe failure

### Requirement: JMS pool health settings have explicit defaults and validation

Per-pool configuration SHALL expose `health_timeout_ms` with default 3000 and positive values, `health_failure_threshold` with default 3 and positive values, and `health_broker_probe_mode` with default `reuse` and allowed values `reuse` and `skip`. Existing `health_check_interval_ms` semantics SHALL remain unchanged. Probe mode SHALL reach Java as `JMS_HEALTH_BROKER_PROBE_MODE` on initial and replacement launches. Rust and Java SHALL reject unsupported probe modes.

The repository SHALL provide a scoped JSON Schema at `schemas/jms-pool-health-config.json` for these health settings, including the existing interval, and executable checks for agreement with runtime defaults and constraints. It SHALL permit other pool properties and explicitly state that runtime validation still covers them. Operator docs SHALL describe stale/unknown status, timeout-versus-interval semantics, threshold recovery, and engine/bridge version compatibility. A new ADR SHALL record the liveness-versus-connectivity decision. The bridge version SHALL remain unchanged in this change.

#### Scenario: Existing configuration receives safe defaults

- **GIVEN** a pool configuration omitting all three new keys
- **WHEN** configuration is loaded and a bridge is launched
- **THEN** the timeout SHALL be 3000 ms, the threshold SHALL be 3, and the mode SHALL be `reuse`
- **AND** a replacement launch SHALL receive the same mode

#### Scenario: Invalid settings fail before launch

- **GIVEN** a zero timeout, zero failure threshold, or unknown probe mode
- **WHEN** runtime configuration or the scoped schema validates that setting
- **THEN** validation SHALL reject it with the relevant property identified

#### Scenario: Compatibility does not hide the old blocking handler

- **GIVEN** an older bridge binary with a blocking health handler
- **WHEN** operators read the new health configuration documentation
- **THEN** the documentation SHALL state that the broker-latency isolation guarantee requires the updated bridge handler, although no protobuf change is needed
