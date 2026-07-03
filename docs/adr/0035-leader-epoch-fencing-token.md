# ADR-0035: Leader-Epoch Fencing Token for Split-Brain Safety

**Date:** 2026-07-03
**Status:** Accepted (Batch 4 — Leader Fencing)
**Amends:** none
**Cross-refs:** ADR-0033 (fail-closed policy — fencing is a safety primitive),
`camel-master/src/leadership.rs` (`spawn_epoch_bridge`, `LEADER_EPOCH_PROPERTY`),
`camel-api/src/platform.rs` (`LeadershipHandle::leader_epoch`)

## Context

In a multi-node deployment, a node that loses leadership may still have
in-flight pipeline Exchanges reaching downstream systems. Without a fencing
token, downstream systems cannot distinguish exchanges from the current
leader from those of a stale leader, leading to duplicate processing
(split-brain).

## Decision

Stamp every delegate-emitted ExchangeEnvelope with `x-camel-leader-epoch`,
a monotonic fencing token derived from the leadership backend.

### Epoch Source

**Kubernetes backend**: The epoch is a **server-authoritative annotation
counter** (`camel.io/leader-term`) on the K8s Lease object. Each acquiring
pod reads the current term, increments it, and writes it back via the
Lease `replace` operation (which uses optimistic concurrency via
`resourceVersion`). Only one pod can win the replace; its term is globally
committed. The term is a simple incrementing u64 — not derived from any
pod's clock — so it is globally monotonic across pods. On renew (same
leader), the term is preserved unchanged.

**Noop backend**: Constant epoch=1. Models a single-node deployment with
no split-brain risk. A constant epoch is valid fencing because there is
only ever one leader.

### Key invariants

1. **Global monotonicity**: epoch from the K8s backend is a server-side
   annotation counter (`camel.io/leader-term`) — incremented on each
   takeover via optimistic concurrency. Monotonic across pods.
2. **Snapshot semantics**: the bridge stamps with its spawn-time snapshot,
   not a live read. Stale bridges carry stale terms.
3. **Sink contract**: downstream systems (databases, message brokers,
   external APIs) SHOULD check `x-camel-leader-epoch` and reject envelopes
   whose epoch is older than the current leader's epoch. Batch 4 supplies
   the token; sink-side enforcement is opt-in and deferred.
4. **Trust boundary**: the epoch property is set by the Master component's
   bridge task inside the process. It is not signed or authenticated —
   it assumes the process is not compromised. If untrusted processes can
   send to the pipeline, additional authentication is required.

### Bridge lifecycle

The `spawn_epoch_bridge` function creates a bounded channel (128-deep)
between the delegate consumer and the pipeline. Each envelope is stamped
with the snapshot epoch before forwarding. On delegate stop (sender drop),
the bridge drains its buffer and exits. On route shutdown (parent_cancel),
the bridge aborts immediately. The bridge JoinHandle is stored in
`DelegateState::Active` and awaited by `stop_delegate` within drain_timeout.

## Consequences

- Every ExchangeEnvelope from a `master:` route carries `x-camel-leader-epoch`.
- Downstream sinks gain an opt-in rejection mechanism.
- The epoch-stamping bridge adds one hop (bounded 128-deep channel).
- The K8s backend extracts leader-term from Lease annotations — no
  additional API calls are needed.
