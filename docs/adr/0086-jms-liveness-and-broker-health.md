# ADR-0086: JMS liveness and broker health

- Status: Accepted (decided 2026-10-20)
- Source: openspec change `jms-health-liveness`
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
- A proactive refresh starts after 5000 ms.
- One owned daemon worker performs at most one refresh at a time.
- There is no refresh queue.
- Broker I/O holds no service-wide lock.
- A result from an invalidated generation cannot overwrite current status.

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

| Broker status | `healthy` | `brokerConnected` | Readiness |
| --- | --- | --- | --- |
| Verified up | true | true | Healthy (200) |
| Verified down | false | false | Unhealthy (503) |
| Not yet checked | true | false | Degraded (200) |
| Skipped | true | false | Degraded (200) |
| Stale | true | false | Degraded (200) |

This split defines readiness:

- Live-unverified status maps to `Degraded` 200. The process is live and
  ready, and broker connectivity is not verified.
- Verified-down status maps to `Unhealthy` 503.
- A healthy RPC that reports `healthy=false` maps to `Unhealthy` 503. The
  adapter does not wait for an RPC failure.
- An RPC failure or timeout also maps to `Unhealthy` 503.

Readiness does not request a process restart. Broker status never publishes
lifecycle `Degraded` for the restart policy.

JMS health fields do not control data-plane admission or settlement. They
describe process liveness and broker reachability only. Delivery success is
not a health guarantee.

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

The bridge version and the protobuf contract stay unchanged. The change is
an engine policy and health-handler behavior, not a wire-format change.

A refresh can expire at 2 s while a replacement holds the process lock. In
that case the caller requests a probe and never restarts the process. Slow
broker work alone cannot restart a live bridge.

The readiness split now distinguishes live-unverified from verified-down.
Operators see `Degraded` 200 for unknown, skipped, or stale broker status.
They see `Unhealthy` 503 for verified-down status or a failed health RPC.

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
