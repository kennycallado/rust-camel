# Proposal: cohort-activation-barrier

## Why

The rc-ava7 audit (e_gpt, 2026-08-25) established a latent reentrancy class:
consumers emit exchanges after readiness while the controller actor is still
starting the route, and context startup is sequential
(`context_lifecycle.rs:97-105`), so later siblings are still Registered. A
control-plane command in that first exchange (controlbus stop) hits lifecycle
pre-validation → "invalid transition: Registered -> Stopped" (the rc-slvd
class). EXPOSED today: JMS, CXF, MCP, Kafka, HTTP; THEORETICAL: redis×2,
direct, surrealdb, mqtt, grpc, ws.

Design lineage: spec v1 (per-route gate) REJECTED; papal investigation (e_opus)
ruled the architecture; spec v2 REJECTED on spec-accuracy deviations; papal
review (e_opus, docs/superpowers/specs/rc-jxkj-papal-review-v3-preblessing.md,
durable record = bd rc-jxkj) upheld all six v2 findings, ruled D1-D4, and
pre-blessed this v3 property list. PAPAL VERDICT: PROCEED-V3.

bd: rc-jxkj (discovered-from rc-ava7).

## What Changes

Implements the pre-blessed v3 properties exactly:

1. **Primitive**: `tokio::sync::watch<bool>` seeded `false` (StartupSignal
   precedent, component-api consumer.rs). Open = `send_if_modified`
   false→true (idempotent). Drains await via `wait_for(|open| *open)` raced
   against their cancel token.
2. **Ownership**: gate created CLOSED inside the controller actor at
   construction; handle cloned into each drain task at spawn time.
3. **Opener**: TWO required async port methods on `RouteOrderingPort`
   (crate-private, single implementor — no breakage): `reset_cohort()`
   (close, idempotent) and `activate_cohort()` (open, idempotent), both
   invoked from `start_context` — reset at entry, activate after startup.
4. **Per-boot lifecycle** (supersedes the original R6 "boot-once"): the gate
   re-closes at `start_context` entry (alongside the existing `cancel_token`
   reset at context_lifecycle.rs:52) and re-opens after the loop. Ground:
   `auto_startup_route_ids` filters on the static `definition.auto_startup()`
   flag (route_registry.rs:95), so stop→start re-issues StartRoute for every
   auto-startup route and drains re-spawn. Hot-reload of a single route while
   the context is up sees an already-open gate → zero added latency.
5. **Failure semantics (D1)**: the gate opens on EVERY return after the
   entry reset — service startup, validation, reconciliation, route-id
   listing, and the StartRoute loop all funnel through a single
   capture-result → always `activate_cohort().await` → return-original-result
   pattern (a Drop guard cannot await an async port call). Boot failure must
   not be made worse than today's partial-up-and-return behavior
   (already-Started routes keep draining; a stale drain from a failed stop
   cannot strand).
6. **Gated sites — consumer-envelope dispatch, three sites**: the
   pre-blessing lists two (route_controller.rs:1047 aggregate envelope branch;
   route_controller_trait.rs:488 simple `_` branch). DEVIATION (documented,
   for the spec-blessing expert to adjudicate): route_controller_trait.rs:416
   — the `ConcurrencyModel::Concurrent` branch — is ALSO a consumer-envelope
   drain site (verified: `rx.recv()` of `ExchangeEnvelope` with pipeline
   dispatch). Gating only two sites would let the Concurrent topology bypass
   activation, violating the ruling's own no-topology-bypass principle. v3
   gates all three.
7. **NOT gated**: the late-exchange branch at route_controller.rs:1034
   (`late_rx` carries aggregator OUTPUT `Exchange`, fed only after a consumer
   envelope traversed the pre-pipeline → aggregator; it is transitively
   post-activation — ruling D3).
8. **Placement (R2)**: the gate-await lives INSIDE the cancellation-aware
   select, guarding dispatch after `recv` — not wrapping `recv`. Parked InOut
   `reply_tx` resolves to `Err(ChannelClosed)` on stop.
9. **Backpressure contract (R4 phrasing, per ruling)**: after `recv`, one
   envelope is held outside the capacity-256 mpsc; the channel still buffers
   up to 256; a producer emitting >256 pre-activation backpressures on
   `send().await` (ADR-0044). Backpressure, not a hang.
10. **Docs (R5)**: CONTEXT-MAP.md Key-Term "Cohort Activation Barrier";
    camel-core CONTEXT.md architecture note.

Explicitly excluded: RuntimeBus validation changes, handshake protocol
changes, component API changes, rc-a7rh (outer-task panic watcher),
component-level reorders (rc-ragw superseded for covered paths).

## Acceptance criteria

- Deterministic F8 regression test (camel-core unit test): route A Immediate
  (deterministic first-tick emission, pipeline stops route B via controlbus),
  route B held mid-Starting via `#[cfg(test)] emit_start_route_event` hooks
  plus a dispatch-observation signal; assert A's exchange is buffered-not-
  dispatched while the gate is closed; release B, the loop completes,
  `activate_cohort` fires; A's StopRoute(B) then dispatches and SUCCEEDS.
- Barrier unit tests: open idempotent; `wait_for` resolves immediately when
  already open; parked dispatch races cancellation.
- All three envelope sites gated — verified by test/code review; Concurrent
  topology covered.
- Component API untouched (`rg 'pub fn sender'` unchanged; no component crate
  modified).
- `cargo test -p camel-core` green; full gate suite green (rc-q74u exemption).

## Risk budget

Acceptable: boot-latency for first exchanges bounded by cohort startup
duration (the same bound sequential boot already imposes); worst case is a
parked drain if `activate_cohort` is never called — bounded by the D1
open-on-both-paths rule and by racing the per-pipeline cancel tokens. Out of
bounds: changing sequential boot order, RuntimeBus semantics, consumer API
surface, the handshake protocol.
