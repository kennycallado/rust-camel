# Design: jms-message-fidelity

## Context

`bridges/jms` (Java sidecar, gRPC `camel.bridge.v1`) converts JMS messages to
protobuf `JmsMessage` for the Rust component `crates/components/camel-jms`.
Producer path already stamps `ContentType` as a JMS string property
(`JmsProducer.java:43`). Consumer path (`JmsConsumer.convertMessage`) branches
only on `BytesMessage`/`TextMessage`. `JmsBridgeService` multiplexes gRPC
subscribe streams over an `activeConsumers` map keyed by client-supplied
`subscription_id`.

Referenced ADRs: ADR-0012 (handler-contract boundary — consumer logs at warn,
route owns the signal), ADR-0032 (exchange-data trust boundary — broker
message content is untrusted). Phase 3 adds ADR-0067 (message-type forwarding
policy).

## Phase 1 — ContentType preservation (rc-kzti)

`convertMessage` TextMessage branch currently hardcodes
`b.setContentType("text/plain")` (`JmsConsumer.java:262`). Change: read the
`ContentType` property BEFORE the branch dispatch (property enumeration happens
after the branches today, so read it explicitly via
`msg.getStringProperty("ContentType")` in a try/catch for absent property —
JMS throws no exception for a missing string property, it returns null). Use
it when non-null and non-empty; else fall back to `text/plain`. The property
STILL lands in headers via the existing property-enumeration loop (no header
change). `BytesMessage` branch keeps its implicit empty content type.

Wire scope: `content_type` field of `JmsMessage` proto — no proto change.

## Phase 2 — Subscription-ID collision guard (rc-5e4l)

`JmsBridgeService.subscribe` (`JmsBridgeService.java:59-62`):
- Replace `activeConsumers.put(subId, consumer)` with `putIfAbsent`.
- On rejection (existing entry): destroy the freshly created consumer via
  `consumerFactory.destroy(consumer)` (fail fast, no leak), then
  `responseObserver.onError(Status.ALREADY_EXISTS.withDescription(...))` and
  return. Error BEFORE any `setOnCancelHandler` registration — the rejected
  stream owns no cleanup.
- `cleanupSubscription` (`:151`): switch
  `activeConsumers.remove(subId)` → `activeConsumers.remove(subId, consumer)`
  (two-arg, owner-checked) so a stale stream can never evict the current
  owner's entry.

gRPC status choice: `ALREADY_EXISTS` (canonical duplicate-resource; house
style already uses specific codes — INTERNAL `:54`, FAILED_PRECONDITION
`:110`).

## Phase 3 — Forwarding policy for Object/Map/Stream (rc-41h3)

Decision inputs (recorded in ADR-0067 before code):
1. Current behavior: fall-through → empty body, headers preserved,
   AUTO_ACKNOWLEDGE receipt acknowledges (`JmsConsumer.java:132`).
2. Java-serialization gadget risk: `ObjectMessage.getObject()` executes
   attacker-controlled `readObject`; broker messages are exchange data under
   ADR-0032 (untrusted, adversary-controlled); a policy of "never
   deserialize" keeps the bridge out of the deserialization attack surface.
3. MapMessage flattening alternative (map → JSON body): rejected for now.
   JMS map values are constrained (primitives, `String`, `byte[]`), but
   faithful flattening still forces real decisions the bytes-only proto
   cannot express today: numeric type preservation (int vs long vs double
   distinction lost in JSON numbers), `byte[]` representation (base64 vs
   hex, versioned how), null-value semantics (JSON null vs absent key), and
   a canonical media type + versioning for the synthesized body. With no
   consumer demand, each is an unforced compatibility commitment. Recorded
   as revisitable.

Ruling: keep empty-body forwarding for all three types; document per-type in
`bridges/jms/README.md` policy table; pin with tests:
`JmsConsumerMessageTypesTest` gains `mapMessageForwardedEmptyAndAcked` and
`streamMessageForwardedEmptyAndAcked` mirroring the existing ObjectMessage
case. Per-type never-invoked verification (Mockito `never()` on the concrete
JMS accessors — `MapMessage` has no `getMap()`):
- ObjectMessage: `verify(objectMessage, never()).getObject()`
- MapMessage: `verify(mapMessage, never()).getMapNames()` and
  `verify(mapMessage, never()).getObject(anyString())`
- StreamMessage: `verify(streamMessage, never()).readInt()`,
  `readString()`, `readBytes(any())`
Ack contract asserted as: `verify(connection).createSession(false,
AUTO_ACKNOWLEDGE)` AND `verify(message, never()).acknowledge()` — receipt
under AUTO_ACKNOWLEDGE acknowledges; we claim session mode + absence of
explicit ack, not provider cardinality. Headers preserved via
`stubCommonAttributes`.

## Rust contract tests (Phases 1-2)

`crates/components/camel-jms/src/bridge_client_test.rs` — in-proc harness
already exercises real gRPC against the Java service? No: it is a Rust-only
mock server implementing the proto. Extend the mock server:
- Phase 1: emit a TextMessage-shaped `JmsMessage` with
  `content_type="application/xml"` — assert the Rust consumer surface exposes
  it (round-trip contract).
- Phase 2: mock server that returns `ALREADY_EXISTS` on second subscribe with
  same id — assert the Rust client surfaces the status error (contract: the
  error code is part of the bridge API).

## Affected boundaries

Components (camel-jms) — contract tests only, no production change. Bridges
(jms) — consumer/service/producer Java. No Runtime, DSL, or Services changes.
No proto change.

## Phases

1. **content-type** — Java fix + Java test + Rust contract test. Exit: XML
   round-trip green both sides.
2. **subscription-guard** — Java putIfAbsent + owner-checked cleanup + Java
   service test + Rust contract test. Exit: collision rejected, no
   cross-eviction.
3. **forwarding-policy** — ADR-0067 + README table + Map/Stream tests.
   Exit: full matrix green, policy documented.
