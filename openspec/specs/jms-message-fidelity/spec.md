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
`StreamMessage` with an empty body and preserved headers. It SHALL open the
consumer session with `CLIENT_ACKNOWLEDGE` and SHALL acknowledge each such
message only through the completion settlement path, after the Rust route
reports success. The single exception SHALL be the terminal reject-then-log
discard: when the message reaches `JMS_MAX_DELIVERY_ATTEMPTS`, the bridge SHALL
acknowledge it without route success, and SHALL still never invoke a body
accessor. It SHALL never invoke body accessors (`getObject` on
ObjectMessage; `getMapNames`/`getObject(String)` on MapMessage;
`readInt`/`readString`/`readBytes` on StreamMessage) on those messages. The
policy SHALL be recorded in an ADR and a per-type table in
`bridges/jms/README.md`, including the terminal-discard exception.

#### Scenario: ObjectMessage forwarded empty and never deserialized

- **GIVEN** a broker message of type `ObjectMessage`
- **WHEN** the consumer forwards it
- **THEN** the `JmsMessage` body SHALL be empty and headers SHALL be preserved
- **AND** `getObject()` SHALL never be invoked
- **AND** the session SHALL run with `CLIENT_ACKNOWLEDGE`
- **AND** no `acknowledge()` call SHALL be made before the route reports success, except the terminal reject-then-log discard

#### Scenario: MapMessage forwarded empty without map access

- **GIVEN** a broker message of type `MapMessage`
- **WHEN** the consumer forwards it
- **THEN** the `JmsMessage` body SHALL be empty and headers SHALL be preserved
- **AND** `getMapNames()` and `getObject(String)` SHALL never be invoked
- **AND** the session SHALL run with `CLIENT_ACKNOWLEDGE`
- **AND** no `acknowledge()` call SHALL be made before the route reports success, except the terminal reject-then-log discard

#### Scenario: StreamMessage forwarded empty without stream reads

- **GIVEN** a broker message of type `StreamMessage`
- **WHEN** the consumer forwards it
- **THEN** the `JmsMessage` body SHALL be empty and headers SHALL be preserved
- **AND** `readInt()`, `readString()`, and `readBytes(...)` SHALL never be invoked
- **AND** the session SHALL run with `CLIENT_ACKNOWLEDGE`
- **AND** no `acknowledge()` call SHALL be made before the route reports success, except the terminal reject-then-log discard

#### Scenario: Policy is documented

- **GIVEN** the forwarding policy decision
- **WHEN** a reader consults `bridges/jms/README.md`
- **THEN** a per-type table SHALL state, for Object, Map, and Stream, the body representation (empty), the security rationale (no deserialization), and the terminal reject-then-log discard exception
- **AND** an ADR SHALL record the decision inputs including the rejected MapMessage flattening alternative

