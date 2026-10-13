# jms-message-fidelity (delta for change `jms-bridge-080`)

Delta for change `jms-bridge-080`. The settlement protocol moves the consumer
session from `AUTO_ACKNOWLEDGE` receipt to `CLIENT_ACKNOWLEDGE` with
completion-driven acknowledgement. One requirement that named the old receipt
behavior is restated. The other requirements in the canonical
`jms-message-fidelity` spec are unchanged.

## MODIFIED Requirements

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
