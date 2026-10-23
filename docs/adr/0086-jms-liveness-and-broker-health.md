# ADR-0086: JMS liveness and broker health

- Status: Accepted (decided 2026-10-20; amended 2026-10-23)
- Source: openspec change `jms-health-liveness`
- Amendment: openspec change `jms-health-stale` — explicit broker status,
  retained verification, and additive wire fields supersede the
  stale-always-`Degraded` readiness semantics
- Cross-refs: ADR-0036 (bridge IPC security), ADR-0052 (diagnostics
  exposure), CONTEXT-MAP readiness rule (`Degraded` vs `Unhealthy`)

## Context

The JMS component runs each pool slot as a bridge subprocess and monitors it
through a health RPC. Before this decision, a health check could wait for
broker connectivity. A slow or unavailable broker therefore appeared as a
dead process. The engine restarted a live bridge, dropped attached consumers,
and interrupted message processing.

The health RPC also mixed two questions:

- Is the bridge process alive and able to answer?
- Is the broker reachable and usable?

Those questions have different operational meanings. A broker outage is not
proof that the process must restart. A failed RPC or a timeout is the signal
that process liveness is in doubt.

## Decision

### A completed health RPC proves liveness

The engine treats any completed health RPC response as proof of liveness.
This rule holds when the response reports `healthy=false`,
`brokerConnected=false`, or both. Only consecutive failed liveness probes
restart a bridge.

A failed liveness probe is an RPC error or the expiration of
`health_timeout_ms`. A successful response resets the consecutive failure
count. The count is local to the slot and the bridge generation. A
replacement resets the count. Shutdown and terminal-state guards continue to
prevent respawn.

The threshold is `health_failure_threshold`, with default 3. The timeout is
`health_timeout_ms`, with default 3000 ms. `health_check_interval_ms` keeps
its existing default and meaning. The engine restarts only at the configured
number of consecutive failures.

### Transport errors request a probe, not a restart

A producer or consumer transport error must not restart the process
directly. The production transport-error path sends a coalesced liveness
wake-up to the monitor. The monitor then runs one real health probe. Repeated
wake-ups coalesce and do not create overlapping probes.

One channel refresh has a fixed 2 s budget. The budget covers process-lock
acquisition and the selected connector future. Progress does not renew the
budget. After a failed or expired refresh, the caller requests a liveness
probe within 500 ms. If the refresh expires at 2 s while a replacement holds
the process lock, the caller requests a probe and never restarts the process.
The existing producer and consumer counters keep their initialization,
increments, threshold checks, and reset points.

### Broker status is a cached snapshot

Broker status is a cached snapshot with these limits:

- `HEALTH_TTL_MS` is 10000 ms.
- An owned daemon scheduler requests a refresh at startup and then every
  5000 ms. Reads never admit refresh work; each read observes the
  published snapshot independently.
- One owned daemon worker performs at most one refresh at a time.
- There is no refresh queue.
- Broker I/O holds no service-wide lock.
- A result from an invalidated generation cannot overwrite current status.

With the updated bridge, the health response carries the broker state as
explicit additive fields: a numeric `status` (`UP`, `STALE`, `UNKNOWN`,
`SKIPPED`, `DOWN`) and a numeric `age_ms` stale age that is meaningful for
`STALE`. Future numbers outside the known range are preserved, never
rewritten. The `healthy` and `brokerConnected` booleans keep their
meaning. The fields are additive protobuf additions; an older bridge omits
both, and its response decodes as `UNSPECIFIED` with age 0.

In `reuse` mode the bridge retains one provider diagnostic connection across
refreshes. It does not open a fresh broker connection for each health check.
In `skip` mode the health path performs no broker I/O. Probe failures do not
tear down consumer connections or reset the producer pool.

Unknown, skipped, and stale status are explicit. They never claim verified
broker connectivity. Successful sends and refresh results use the same
atomic generation-checked publication path.

### Readiness preserves liveness and broker verification

The readiness adapter maps a completed RPC to the broker status in the
response. The distinction between live-unverified and verified-down is
binding.

The table below is the legacy mapping for responses without explicit
status (`UNSPECIFIED`). Superseded (2026-10-23, change
`jms-health-stale`): for responses that carry explicit status, the
always-`Degraded` semantics of the Not-yet-checked, Skipped, and Stale
rows are replaced by the retention mapping in the "Explicit status
retains the last verified state" section below.

| Broker status | `healthy` | `brokerConnected` | Readiness |
| --- | --- | --- | --- |
| Verified up | true | true | Healthy (200) |
| Verified down | false | false | Unhealthy (503) |
| Not yet checked | true | false | Degraded (200) |
| Skipped | true | false | Degraded (200) |
| Stale | true | false | Degraded (200) |

For legacy `UNSPECIFIED` responses this split defines readiness; explicit
statuses never use these boolean rules:

- A live-unverified legacy observation maps to `Degraded` 200. The process
  is live and ready, and broker connectivity is not verified.
- A legacy observation that reports `healthy=false` maps to `Unhealthy`
  503. The adapter does not wait for an RPC failure.
- An RPC failure or timeout also maps to `Unhealthy` 503.

For responses that carry explicit status, the retention mapping in the
"Explicit status retains the last verified state" section below is
authoritative.

Readiness does not request a process restart. Broker status never publishes
lifecycle `Degraded` for the restart policy.

JMS health fields do not control data-plane admission or settlement. They
describe process liveness and broker reachability only. Delivery success is
not a health guarantee.

### Explicit status retains the last verified state

With a bridge that sends explicit status, the readiness adapter retains
the last verified state per bridge identity and applies this mapping:

| Broker status | Retained verification | Readiness |
| --- | --- | --- |
| UP | sets verified-up | Healthy (200) |
| DOWN | sets verified-down | Unhealthy (503) |
| STALE, UNKNOWN, SKIPPED, future | last verified-up | Healthy (200) |
| STALE, UNKNOWN, SKIPPED, future | none (startup) | Degraded (200) |
| STALE, UNKNOWN, SKIPPED, future | last verified-down | Unhealthy (503) |
| UNSPECIFIED (legacy bridge) | never updated | legacy boolean mapping |

- An unverified observation with no retained verification (startup, or
  right after a reset) maps to `Degraded` 200. The process is live and
  ready; broker connectivity is not yet verified.
- An unverified observation after a verified-up observation keeps
  `Healthy` 200. A stale snapshot is not disconnection evidence.
- An unverified observation after a verified-down observation keeps
  `Unhealthy` 503 until a new verified observation arrives.
- Retained verification resets when the bridge slot is absent, not ready,
  or replaced (a new Ready channel identity). The next observation starts
  from the startup rule.
- `UNSPECIFIED` keeps the boolean mapping of the legacy table above and
  never updates retained verification.
- An RPC error or timeout maps to `Unhealthy` 503. It is process-liveness
  evidence; it is not verified broker state and does not update retained
  verification.

The health monitor reports status transitions as bounded warnings. A
`STALE` warning emits only when the status enters `STALE`; age-only
changes stay silent. These reports add no restart signal, no consecutive
broker-status debounce, and no operational knob. The liveness threshold,
its counters, and their reset points are unchanged. The producer
idleTimeout exclusion stands (`rc-w06u6`): broker-status reporting does
not touch producer idle handling.

A future consumer transport-listener integration may feed transport events
into the same generation-fenced broker-status cache as additional refresh
inputs. It is explicitly not implemented: the cache admits only
scheduler-driven refreshes today. No live-broker or native-image
verification is claimed by this amendment; the accepted limitation below
stands.

## Consequences

The default worst-case dead-process detection time is the threshold times
the interval plus the timeout:

```text
3 x (5 s + 3 s) = about 24 s
```

This is slower than the previous first-error restart. The slower detection
is accepted because it removes restart loops caused by broker latency. Real
dead-process recovery still occurs through the existing bounded restart
path.

The explicit-status fields are additive protobuf additions, so the wire
contract stays compatible. The guarantees of this amendment need the
updated Java bridge implementation; this mission does not bump the bridge
version. An older bridge keeps the legacy boolean mapping.

A refresh can expire at 2 s while a replacement holds the process lock. In
that case the caller requests a probe and never restarts the process. Slow
broker work alone cannot restart a live bridge.

The readiness split now distinguishes live-unverified from verified-down.
Operators see `Degraded` 200 while broker verification is absent: at
startup and after a bridge replacement. An unverified observation after a
verified-up state keeps `Healthy` 200; after a verified-down state it
keeps `Unhealthy` 503. A failed health RPC maps to `Unhealthy` 503
without writing broker verification.

The producer and consumer counter semantics stay unchanged. No threshold or
reset point moves.

## Evidence and limits

Automated coverage includes T1 to T5 and the T6 paired-contract cases. T6
uses the real Java health handler and the real Rust monitor.

The Java fake-broker test uses the real handler with a blocked fake broker
factory. It calls health over three 5 s probe periods. Every call completes
inside the configured deadline, and diagnostic connection opens stay flat at
one.

The Rust test uses the real `spawn_health_monitor` with a configured
non-default timeout and threshold. It runs for at least three real probe
periods against both stale and verified-down responses. The test observes
zero replacement attempts and an unchanged actual slot child PID.

### Accepted limitation

T6 is paired-contract coverage. It is **not a live-broker or native-image soak**. It does not prove behavior under a production broker, a real network partition, or a native image.

Native and live-broker evidence remains CI-owned. The `CI owner` owns that
verification, and this change does not claim it.

The investigation park remains `not_reproduced` because live-broker and
native evidence are absent. Its `next_steps` are unchanged and assign that
evidence to CI.

## References

- ADR-0036: Bridge IPC mutual TLS. Health RPCs use the same bridge transport
  and certificate posture.
- ADR-0052: Diagnostic endpoint exposure posture. Readiness endpoints expose
  operational metadata, not credential material.
- CONTEXT-MAP: `Degraded` vs `Unhealthy` readiness rule. `Degraded` is HTTP
  200 and Ready. `Unhealthy` is HTTP 503 and NotReady.
- openspec change `jms-health-liveness`, JMS specification and design.
