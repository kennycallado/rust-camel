# Design: jms-health-liveness

## Approach

Treat a completed health RPC as proof of bridge liveness, regardless of broker fields. In `component.rs`, count consecutive RPC errors/deadlines per slot and generation. A response resets the count. Below threshold, retain the usable state; at threshold, use existing bounded recovery. Reset on replacement; preserve terminal/shutdown guards. Route transport-error restart requests through this policy rather than bypassing the threshold. Broker-status signals must not publish lifecycle `Degraded`, which drives restart and rejects consumers.

Add `health_timeout_ms` (default 3000, positive), `health_failure_threshold` (default 3, positive), and `health_broker_probe_mode` (`reuse` default, or `skip`) to `JmsPoolConfig`. Retain `health_check_interval_ms` scheduling. Carry settings through pool construction and both initial/restart launches. Append `JMS_HEALTH_BROKER_PROBE_MODE` to the existing process configuration's environment without changing the shared constructor. Parse it in `BridgeConfig`; reject unsupported values at both configuration boundaries.

Make Java `health()` return an immutable snapshot immediately. Use the spec's response/readiness table: unverified status is live but not broker-connected. In `health.rs`, map that to readiness `Degraded` (200), not `Unhealthy`. A snapshot or in-flight refresh older than the ten-second TTL becomes stale/unverified with its age reported. Schedule at most one refresh on one owned daemon worker, without a refresh queue. Broker I/O holds no service-wide lock. Publish atomically, including send-success updates; reject results from invalidated generations. Bound teardown even when broker I/O hangs.

In `reuse`, retain one raw-factory diagnostic connection across refreshes. Use metadata and provider exception/interruption signals to update status. Replace broken connections at most once per ten-second cache cadence, under single-flight control. Retain both providers' native-image safeguards. Probe failures never reset consumer connections or the producer pool. `skip` performs no health-owned broker I/O and reports unverified connectivity. Cached connectivity does not prove delivery success.

No pool JSON Schema currently exists. Add `schemas/jms-pool-health-config.json`, an explicitly scoped object schema for the three new health properties and the existing interval. Permit other pool properties, which retain their runtime validation. Pin defaults, enum values, and minimums to Rust configuration through executable consistency checks. Document that this schema fragment is not a complete pool validator.

Add ADR 0086 (verify allocation), JMS docs, and context pointers. Reference ADR-0036 for IPC security, ADR-0052 for diagnostics, and CONTEXT-MAP's readiness distinction. Health fields do not control data-plane admission or settlement. T6 is paired contract coverage, not a live-broker soak: Java holds a fake broker check blocked over three probe periods while real handler calls complete within timeout; Rust exercises the real monitor against a prompt fake bridge for three periods with zero restarts. Native/live-broker verification stays with CI.

## Affected crates

- `camel-component-jms`: configuration, monitor, restart policy, launch environment, tests, and component context.
- `bridges/jms`: configuration, cached handler, retained probe resource, lifecycle cleanup, Java tests, and README.
- `schemas/`, `docs/adr/`, and JMS operator docs: validation and operational contract.

## Architecture boundaries

Components own broker status and bridge recovery. The bridge provides a responsive control-plane RPC; broker operations and delivery remain data-plane concerns. Runtime, DSL, Services APIs, Languages, Functions, and protobuf contracts stay unchanged.

## Phases

### Phase 1: Broker-independent Java liveness
- **Goal:** Remove broker waits and per-probe connection churn from health RPCs.
- **Dependencies:** Existing bridge lifecycle and provider workarounds.
- **Externally-visible types/interfaces:** `JMS_HEALTH_BROKER_PROBE_MODE`; unchanged health response.
- **Deliverable:** Cached handler, retained probe, teardown, T4/T5.
- **Exit-criteria:** Repeated checks keep opens flat; blocked broker calls do not block health or other RPCs; existing Java suites and spotless pass.

### Phase 2: Engine policy and complete contract
- **Goal:** Apply configurable liveness thresholds and publish/document pool settings.
- **Dependencies:** Phase 1 supplies broker-independent health responses.
- **Externally-visible types/interfaces:** Three pool keys and scoped JSON Schema.
- **Deliverable:** Monitor/config/launch wiring, ADR/docs/schema, T1/T2/T3/T6.
- **Exit-criteria:** Zero restarts over at least three delayed-broker probe periods; dead process restarts after configured failures; configuration/schema checks and required gates pass.

## Alternatives considered

Increasing the timeout alone still confuses broker failure with process death. Thresholds alone only postpone that mistake. Synchronous probing without a lock still blocks health. A new liveness RPC adds unnecessary wire/version coupling. A fresh broker connection for each check preserves the observed churn.
