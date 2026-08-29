# jms-message-fidelity Specification

## Purpose
TBD - created by archiving change jms-message-fidelity. Update Purpose after archive.
## Requirements
### Requirement: Consumed TextMessage preserves the ContentType property

The JMS bridge consumer SHALL deliver a consumed `TextMessage` with the
`content_type` field set to the message's `ContentType` JMS string property
when that property is present and non-empty, and SHALL fall back to
`text/plain` when absent or empty. The `ContentType` property SHALL remain in
the forwarded headers map.

#### Scenario: ContentType property present

- Given a broker message of type `TextMessage` with property
  `ContentType=application/xml`
- When the consumer converts it to a `JmsMessage`
- Then `content_type` is `application/xml`
- And the headers map still contains `ContentType=application/xml`

#### Scenario: ContentType property absent

- Given a broker message of type `TextMessage` with no `ContentType` property
- When the consumer converts it to a `JmsMessage`
- Then `content_type` is `text/plain`

#### Scenario: ContentType property empty

- Given a broker message of type `TextMessage` with `ContentType=""`
- When the consumer converts it to a `JmsMessage`
- Then `content_type` is `text/plain`

#### Scenario: Non-text ContentType changes the Rust body variant

- Given a broker message of type `TextMessage` with `ContentType=application/xml`
- When a Rust consumer route builds its exchange from the delivered `JmsMessage`
- Then the body surfaces as bytes, not text (the consumer routes `text/*` to
  text bodies; `application/xml` no longer masquerades as `text/plain`)
- And route authors pattern-matching on text bodies for XML JMS payloads must
  match bytes instead — a visible behavior flip of this change

#### Scenario: BytesMessage content type is unaffected

- Given a broker message of type `BytesMessage` with property
  `ContentType=application/xml` and a non-empty body
- When the consumer converts it to a `JmsMessage`
- Then `content_type` is empty (BytesMessage branch sets none)
- And the body carries the bytes and the headers map still contains
  `ContentType=application/xml`

### Requirement: Duplicate subscription IDs are rejected without eviction

The JMS bridge service SHALL reject a second concurrent `subscribe` stream
carrying a `subscription_id` already present in the active-consumer map, with
gRPC status `ALREADY_EXISTS`, before registering any state for the rejected
stream. Subscription cleanup SHALL remove the map entry only when the entry
still belongs to the stream being cleaned up.

#### Scenario: Second stream with an in-use subscription ID

- Given an active subscribe stream with `subscription_id="s1"`
- When a second subscribe stream arrives with `subscription_id="s1"`
- Then the second stream terminates with status `ALREADY_EXISTS`
- And the first stream continues delivering messages

#### Scenario: Cancelling the first of two differently-keyed streams

- Given active streams with `subscription_id="s1"` and `subscription_id="s2"`
- When the `s1` stream is cancelled
- Then only the `s1` map entry is removed
- And the `s2` stream continues delivering messages

#### Scenario: Rejected stream leaks no consumer

- Given an active subscribe stream with `subscription_id="s1"`
- When a second subscribe stream with the same ID is rejected
- Then the consumer instance created for the rejected stream is returned to
  the factory (`destroy` called) and no map entry changed owner

### Requirement: Non-Bytes/Text JMS message types forward empty under a documented policy

The JMS bridge SHALL forward `ObjectMessage`, `MapMessage`, and
`StreamMessage` with an empty body and preserved headers, acknowledging them
via session `AUTO_ACKNOWLEDGE` receipt with no explicit `acknowledge()` call,
and SHALL never invoke body accessors (`getObject` on ObjectMessage;
`getMapNames`/`getObject(String)` on MapMessage; `readInt`/`readString`/
`readBytes` on StreamMessage) on those messages. The
policy SHALL be recorded in an ADR and a per-type table in
`bridges/jms/README.md`.

#### Scenario: ObjectMessage forwarded empty and never deserialized

- Given a broker message of type `ObjectMessage`
- When the consumer forwards it
- Then the `JmsMessage` body is empty, headers are preserved
- And `getObject()` was never invoked
- And the session ran with `AUTO_ACKNOWLEDGE` (receipt acknowledges)
- And no explicit `acknowledge()` call was made

#### Scenario: MapMessage forwarded empty without map access

- Given a broker message of type `MapMessage`
- When the consumer forwards it
- Then the `JmsMessage` body is empty, headers are preserved
- And `getMapNames()` and `getObject(String)` were never invoked
- And the session ran with `AUTO_ACKNOWLEDGE` (receipt acknowledges)
- And no explicit `acknowledge()` call was made

#### Scenario: StreamMessage forwarded empty without stream reads

- Given a broker message of type `StreamMessage`
- When the consumer forwards it
- Then the `JmsMessage` body is empty, headers are preserved
- And `readInt()`, `readString()`, and `readBytes(...)` were never invoked
- And the session ran with `AUTO_ACKNOWLEDGE` (receipt acknowledges)
- And no explicit `acknowledge()` call was made

#### Scenario: Policy is documented

- Given the forwarding policy decision
- When a reader consults `bridges/jms/README.md`
- Then a per-type table states, for Object/Map/Stream, the body
  representation (empty) and the security rationale (no deserialization)
- And an ADR records the decision inputs including the rejected MapMessage
  flattening alternative

