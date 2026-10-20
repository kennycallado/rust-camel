# Proposal: jms-health-liveness

## Why

Broker latency currently looks like bridge process failure. The Rust monitor gives the health RPC three seconds; the Java handler opens a broker connection under a lock. One timeout restarts a working consumer. Partner soak evidence records 45 restarts in 17 hours and about 8,600 broker connections per day. Bd: `rc-5vfz3`.

## What Changes

- Separate process/gRPC liveness from broker connectivity. Only consecutive failed liveness probes can trigger automatic recovery.
- Return cached broker status without waiting for broker I/O. Reuse a retained connection for background checks, or disable broker checks through an explicit mode.
- Add per-pool health timeout, failure threshold, and broker-probe mode. Preserve the existing health interval semantics.
- Add a JSON Schema for these pool health settings, operator documentation, and an ADR for the liveness boundary.
- Affected crates: `camel-component-jms`; `camel-bridge` is an integration boundary, not a planned shared API change. Java changes remain in `bridges/jms`.
- Exclude protocol changes, delivery/acknowledgement changes, restart-budget redesign, bridge version changes, and native builds.

## Acceptance criteria

- T1: a successful RPC reporting `healthy=false` and `brokerConnected=false` reports broker trouble without restarting or rejecting an attached consumer.
- T2: one failed liveness probe does not restart with default settings; the configured number of consecutive failures does. A successful response resets the count.
- T3: timeout tests prove that the configured deadline governs the liveness path.
- T4: consecutive broker health checks reuse one connection; steady-state broker opens remain flat.
- T5: a blocked broker check holds no service lock and does not delay health or unrelated RPCs.
- T6: broker latency beyond the liveness deadline, sustained for at least three probe periods, produces zero restarts because health responses remain prompt.
- A dead bridge still restarts after the threshold. Existing JMS Java suites remain green; Java formatting is a mandatory gate.

## Risk budget

Accept cached or unknown broker status and slower dead-process detection with the default threshold of three. Do not accept consumer churn caused by broker checks, unbounded refresh threads, leaked connections, credential-bearing logs, or changes to data-plane settlement. No wire change is required. The isolation guarantee requires the updated Java handler; an older bridge that blocks its health RPC cannot provide that guarantee.
