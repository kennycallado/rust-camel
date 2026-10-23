# Proposal: jms-health-stale

## Why

camel-112 polls every 60 seconds. The JMS cache expires after 10 seconds and currently refreshes only after a read. Each isolated poll can therefore report STALE while traffic continues. The engine treats this unverified result as disconnected. Diagnosis is established in rc-bgv1f; this change fixes that behavior without changing liveness recovery.

## What Changes

- Refresh the Java cache on its own schedule, independent of readers.
- Add an explicit protobuf health status and numeric stale age without changing existing fields.
- Retain the last verified broker state on STALE, UNKNOWN, and SKIPPED in engine readiness and monitor reporting.
- Record startup, compatibility, reset, and logging rules in ADR-0086 and component context.
- Exclude producer-pool idleTimeout (rc-w06u6), consumer transport listeners, consecutive-failure debounce, and bridge version changes.

## Acceptance criteria

- A fake-clock test reads at 0 and 60 seconds without traffic or intermediate reads and observes verified UP after scheduled refreshes.
- Real probe failures remain DOWN with healthy=false. Stale status never changes verified-up readiness to Degraded or verified-down readiness to healthy.
- Unknown startup stays Degraded (200); verified down stays Unhealthy (503). A replacement bridge starts without inherited verification.
- Existing Java cache/probe and Rust JMS tests pass; liveness restart rules remain unchanged.

## Risk budget

Keep single-flight admission, generation fencing, diagnostic reuse, skip mode, non-blocking reads, and bounded shutdown. Retaining verification during staleness intentionally favors continuity; RPC errors/timeouts still report Unhealthy. Legacy responses retain their existing boolean interpretation because they cannot distinguish stale from disconnected.
