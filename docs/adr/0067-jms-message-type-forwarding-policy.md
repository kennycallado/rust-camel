# ADR-0067: JMS Message-Type Forwarding Policy

**Date:** 2026-08-29
**Status:** Accepted
**Origin:** bd rc-41h3 (epic rc-41h3), Phase 3
**Cross-refs:** ADR-0032

## Context

The JMS bridge forwards broker messages to a bytes-only proto. The proto
carries a body and headers. It has no branch for the JMS message types
`ObjectMessage`, `MapMessage`, or `StreamMessage`. Today
`JmsConsumer.convertMessage` falls through for these types and produces an
empty body. This ADR pins that behavior as policy.

## Decision inputs

### Input 1: current empty-body behavior

The fall-through path produces an empty body with headers preserved. Under
`AUTO_ACKNOWLEDGE`, receipt acknowledges the message
(`JmsConsumer.java:132`). No body accessor is invoked (property accessors still run for headers and content-type).

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
body. Preserve headers. Receipt acknowledges under `AUTO_ACKNOWLEDGE`.
Never invoke the body accessors on these types.

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
