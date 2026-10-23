## MODIFIED Requirements

### Requirement: JMS health RPC does not wait for broker connectivity

The Java bridge SHALL return a cached broker-status snapshot without waiting for broker connect, start, inspection, or close. current() SHALL only read the cache and SHALL NOT initiate broker probes. An owned scheduler SHALL request refresh at startup and every REFRESH_AFTER_MS independently of reads or sends. The refresh mechanism SHALL retain one probe worker, at most one in-flight refresh, no accumulating refresh queue, generation-fenced publication, and bounded shutdown. Skip mode SHALL perform no broker I/O. Scheduler teardown SHALL prevent further admissions after close.

HealthResponse SHALL add an explicit status (UNSPECIFIED=0, UP, STALE, UNKNOWN, SKIPPED, DOWN) and numeric age_ms while preserving existing field numbers and boolean meanings. STALE SHALL report its age; other statuses SHALL report zero age. UP SHALL have healthy=true and broker_connected=true; DOWN SHALL have healthy=false and broker_connected=false. UNKNOWN, SKIPPED, and STALE SHALL retain healthy=true and broker_connected=false for wire compatibility, but an updated engine SHALL NOT interpret those booleans as fresh disconnection evidence.

The engine readiness adapter and monitor SHALL retain the last verified broker state on explicit STALE, UNKNOWN, SKIPPED, or unknown future status values. UP and DOWN SHALL update verification. Before any verification readiness SHALL be Degraded (200); after UP an unverified observation SHALL remain Healthy (200); after DOWN it SHALL remain Unhealthy (503). Retained verification SHALL reset for an absent/not-ready slot or replacement Ready channel. RPC errors and timeouts SHALL remain Unhealthy and SHALL NOT create broker verification. UNSPECIFIED responses SHALL retain the previous boolean mapping for older bridges. Readiness SHALL NOT request a process restart. Monitor logging SHALL report numeric stale age at most once per status transition and SHALL NOT report an unverified observation as newly disconnected. Logs SHALL exclude provider messages and credentials.

#### Scenario: Long poll interval without traffic

- **GIVEN** a deterministic clock and scheduler, a successful fake probe, and no producer traffic
- **WHEN** the cache is read at t=0, scheduled refreshes run through t=60 seconds, and it is read again without intermediate reads
- **THEN** the second read SHALL be UP with broker_connected=true
- **AND** the scheduler SHALL have refreshed independently of current()

#### Scenario: Unverified observations retain verification

- **GIVEN** an engine that has observed UP or DOWN from a bridge instance
- **WHEN** STALE, UNKNOWN, or SKIPPED follows
- **THEN** readiness and reported broker connectivity SHALL retain that verification
- **AND** repeated STALE observations with increasing ages SHALL emit only one stale transition

#### Scenario: Startup and replacement have no inherited verification

- **GIVEN** an unverified bridge or a replacement of a previously UP bridge
- **WHEN** an explicit unverified response arrives
- **THEN** readiness SHALL be Degraded (200) until fresh verification

#### Scenario: Genuine probe failure remains down

- **GIVEN** a fake probe that fails after successful verification
- **WHEN** a scheduled refresh completes
- **THEN** the Java response SHALL be DOWN with healthy=false and broker_connected=false
- **AND** engine readiness SHALL become Unhealthy (503), without a liveness restart

#### Scenario: Legacy responses retain compatibility

- **GIVEN** a bridge omitting the new fields
- **WHEN** the engine receives UNSPECIFIED status
- **THEN** true/true SHALL be Healthy, true/false SHALL be Degraded, and healthy=false SHALL be Unhealthy

#### Scenario: Blocked refresh and teardown stay bounded

- **GIVEN** a verified cache with an indefinitely blocked fake probe
- **WHEN** repeated scheduled ticks, current(), invalidation, and close occur
- **THEN** reads SHALL return promptly, refresh admission SHALL remain single-flight, late old-generation results SHALL not publish, and close SHALL retain its existing fixed bound

#### Scenario: Other RPCs are not blocked

- **GIVEN** a broker check blocked behind a deterministic test latch
- **WHEN** another health RPC and an unrelated RPC execute
- **THEN** both SHALL complete without waiting for the broker check
- **AND** repeated calls SHALL NOT add workers or queue probes

#### Scenario: T6 delayed broker remains live

- **GIVEN** paired contract tests, not a live-broker or native-image soak
- **WHEN** the Java real health handler uses a blocked fake broker factory for at least three configured probe periods
- **THEN** health calls SHALL complete within health_timeout_ms while connection opens remain flat
- **AND** live-broker/native verification SHALL remain deferred to CI
