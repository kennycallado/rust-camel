# ADR-0067: JMS Message-Type Forwarding Policy

**Date:** 2026-08-29
**Status:** Accepted
**Origin:** bd rc-41h3 (epic rc-41h3), Phase 3
**Cross-refs:** ADR-0032
**Amended:** 2026-10-13 for change `jms-bridge-080` (0.8.0): completion settlement and the terminal-discard exception.

## Context

The JMS bridge forwards broker messages to a bytes-only proto. The proto
carries a body and headers. It has no branch for the JMS message types
`ObjectMessage`, `MapMessage`, or `StreamMessage`. Today
`JmsConsumer.convertMessage` falls through for these types and produces an
empty body. This ADR pins that behavior as policy.

## Decision inputs

### Input 1: current empty-body behavior

The fall-through path produces an empty body with headers preserved. In 0.7.x
the session used `AUTO_ACKNOWLEDGE`, so receipt acknowledged the message
(`JmsConsumer.java:132`). No body accessor was invoked (property accessors still
run for headers and content-type). The 0.8.0 amendment below replaces receipt
acknowledgement with completion settlement.

### Input 2: Java-serialization gadget risk

`ObjectMessage.getObject()` executes attacker-controlled `readObject`.
Broker messages are exchange data under ADR-0032. They are untrusted and
adversary-controlled. A policy of never deserializing keeps the bridge out
of the deserialization attack surface.

### Input 3: MapMessage flattening rejected

Flattening a `MapMessage` to a JSON body is rejected for now. JMS map
values are constrained to primitives, `String`, and `byte[]`. Faithful
flattening still forces decisions the bytes-only proto cannot express
today:

- numeric type preservation: the int vs long vs double distinction is lost
  in JSON numbers;
- `byte[]` representation: base64 vs hex, and how it is versioned;
- null-value semantics: JSON null vs an absent key;
- a canonical media type and versioning for the synthesized body.

With no consumer demand, each decision is an unforced compatibility
commitment. Flattening stays revisitable.

## Decision

Forward `ObjectMessage`, `MapMessage`, and `StreamMessage` with an empty
body. Preserve headers. The session runs with `CLIENT_ACKNOWLEDGE`, and the
consumer acknowledges each message only through the completion settlement
path, after the Rust route reports success. Never invoke the body accessors
on these types.

The single exception is the terminal reject-then-log discard. When a message
reaches `JMS_MAX_DELIVERY_ATTEMPTS`, the bridge acknowledges it without route
success and still never invokes a body accessor. This discard is the explicit
exception to at-least-once, applied to every message type, not only the
empty-bodied types. The bridge does not forward to a dead-letter destination.

| JMS type | Body forwarded | Representation | Accessor policy | Rationale |
| --- | --- | --- | --- | --- |
| `BytesMessage` | yes | raw bytes | `readBytes` only | byte-intact |
| `TextMessage` | yes | UTF-8 text. `ContentType` property wins, else `text/plain` | `getText` only | text body |
| `ObjectMessage` | empty | none | never call `getObject` | Java-serialization gadget risk |
| `MapMessage` | empty | none | never call `getMapNames` or `getObject(String)` | no canonical wire representation |
| `StreamMessage` | empty | none | never call `readInt`, `readString`, or `readBytes` | no sequential stream parse |

Content producers must use `BytesMessage` or `TextMessage` to carry a
body.

## Consequences

### No deserialization attack surface

The bridge never calls `getObject()` or any other body accessor. The
Java-serialization gadget risk from Input 2 does not reach the bridge.

### Content producers use Bytes or Text

Producers that need a body must use `BytesMessage` or `TextMessage`.
Object, Map, and Stream messages arrive empty-bodied.

### Flattening is revisitable

MapMessage flattening stays open. A future ADR can revisit it when a
consumer demands it and the proto can express the constraints from Input 3.

## Related policies

- Precedence. The bridge terminal cap applies only when it fires before a
  provider or broker native redelivery or expiry limit. Where the native limit
  fires first, it takes precedence, and the bridge does not claim its terminal
  policy ran. Redelivery is not promised to the source queue. Observed with
  default ActiveMQ Classic configuration: the bridge cap fired first, the
  source queue was empty, and `ActiveMQ.DLQ` `EnqueueCount` was 0. Artemis
  destinations were not observed (`rc-5c6yv`). Not every native-first ordering
  or dead-letter query is verified.
- Operator recovery. On a permanent failure, stop the route and its context.
  Confirm that bridge and journal ownership has ended before you edit any file:
  the last subscription for the affected domain must be released, and the bridge
  process and its file lock must be released. Only then repair or remove the
  corrupt journal or the unkeyable source for the affected domain. Then restart the context. Do not edit a live
  journal file while a route or bridge holds the lock.
- Header forwarding. The producer forwards an outgoing header only when
  `jmsHeaderAllowlist` names it. The allowlist is empty by default, so user
  headers are not forwarded unless allowlisted. This is a breaking change.
  Credentials are denied before the allowlist check.
