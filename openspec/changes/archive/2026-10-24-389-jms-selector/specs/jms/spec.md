## ADDED Requirements

### Requirement: JMS message selector reaches the broker subscription

The system SHALL pass a configured `messageSelector` endpoint option end to end: the Rust
consumer SHALL include it as the `message_selector` field of the gRPC `SubscribeRequest` (both
proto copies byte-identical), the Java bridge SHALL create the JMS consumer with the selector
overload (`session.createConsumer(destination, selector)`), and the producer path SHALL NOT
carry or gate a selector. A subscribe request without a configured selector SHALL be
byte-compatible with the pre-change request (field absent/empty).

#### Scenario: selector configured reaches the subscribe request

- **GIVEN** a route with `jms:queue:orders?message_selector=priority%20%3E%205`
- **WHEN** the consumer subscribes against the in-process bridge mock
- **THEN** the captured `SubscribeRequest` carries `message_selector = "priority > 5"`
  with `destination` and `subscription_id` unchanged

#### Scenario: no selector leaves the request unchanged

- **GIVEN** a route with no `messageSelector` option
- **WHEN** the consumer subscribes
- **THEN** the captured `SubscribeRequest` has an empty `message_selector` field and the
  destination/subscription_id values of the pre-change request

#### Scenario: absent selector means no selector

- **GIVEN** a route with no `messageSelector` option
- **WHEN** the endpoint URI is parsed
- **THEN** `message_selector` is `None` and the subscription proceeds with no selector

#### Scenario: empty option value means no selector

- **GIVEN** a route with `message_selector=` (empty after percent-decoding)
- **WHEN** the endpoint URI is parsed
- **THEN** `message_selector` is `None` (preserved pre-change behavior) and the subscribe
  request carries an empty selector field

#### Scenario: whitespace-only selector is rejected at URI validation

- **GIVEN** a route with `message_selector=%20%20` (whitespace-only after percent-decoding)
- **WHEN** the endpoint URI is parsed
- **THEN** validation fails with an error naming `messageSelector` (no silent fallback to
  no-selector, no empty selector sent on the wire)

#### Scenario: Java consumer uses the selector overload

- **GIVEN** a `SubscribeRequest` carrying a non-empty `message_selector`
- **WHEN** the Java `JmsConsumer` creates its JMS consumer
- **THEN** `session.createConsumer(destination, selector)` is invoked with the request's
  selector, not the one-argument overload

### Requirement: JMS selector support fails closed against bridges without the capability

The bridge SHALL advertise `message-selector-v1` in the `capabilities` of its Health response.
When a selector is configured and the connected bridge's Health evidence does not advertise that
capability, the system SHALL fail closed: the slot SHALL be parked permanently at startup with an
error naming the missing capability, the required bridge version (jms bridge >= 0.11.0), and the
pinned bridge version the component was built for, and zero Subscribe calls SHALL be sent.
Runtime capability loss (bridge replaced by an older build) SHALL park permanently and stop
resubscription, following the `broker-tls-v1` gate pattern. Health statuses
`STALE`/`UNKNOWN`/`SKIPPED`/`UNSPECIFIED` SHALL neither bypass nor weaken the gate; a
never-completing Health RPC follows the existing bounded startup-failure policy.

#### Scenario: startup fail-closed with no silent start

- **GIVEN** a selector-configured consumer and a bridge whose Health response reports an empty
  capabilities list
- **WHEN** the slot starts up
- **THEN** the slot parks permanently with an error containing `message-selector-v1` and the
  required bridge version, and the bridge mock records zero Subscribe calls

#### Scenario: selector-configured consumer on a capable bridge subscribes

- **GIVEN** a selector-configured consumer and a bridge advertising `message-selector-v1`
- **WHEN** the slot starts up
- **THEN** the startup gate passes and the Subscribe call carries the selector

#### Scenario: no-selector consumer on an old bridge is unaffected

- **GIVEN** a consumer with no selector and a bridge without `message-selector-v1`
- **WHEN** the slot starts up
- **THEN** the gate passes trivially and the subscription proceeds

#### Scenario: runtime capability loss parks permanently

- **GIVEN** a running selector-configured consumer whose bridge advertised
  `message-selector-v1` at startup
- **WHEN** a later Health response from the replaced bridge lacks the capability
- **THEN** the slot parks permanently with the missing-capability error and does not resubscribe

#### Scenario: capability evidence is authoritative regardless of health status

- **GIVEN** a selector-configured consumer and a bridge whose Health response reports broker
  status `STALE` (or `UNKNOWN`/`SKIPPED`) but advertises `message-selector-v1`
- **WHEN** the startup gate evaluates
- **THEN** the gate passes on the capability alone (broker health status neither bypasses nor
  weakens the capability check)

#### Scenario: replacement bridge cannot inherit previous evidence

- **GIVEN** a daemon restart (or slot replacement) where the previous bridge instance advertised
  `message-selector-v1` but the new instance does not
- **WHEN** the startup gate evaluates for the replacement
- **THEN** capability evidence re-collected from the current instance governs and the gate
  fails closed (previous-instance evidence does not authorize the replacement)

### Requirement: JMS selector values are broker-validated, not parsed locally

The system SHALL NOT implement a selector-expression parser. Validation is limited to shape
(non-blank after trim). Selector semantics (matching, syntax errors such as an invalid SQL-92
expression) SHALL be owned by the broker; a broker-rejected selector surfaces through the
existing failure classification.

#### Scenario: invalid selector syntax is a broker-owned failure

- **GIVEN** a configured selector the broker rejects (invalid expression)
- **WHEN** the bridge subscribes
- **THEN** the resulting error is classified by the existing broker-failure classifier and no
  Rust-side selector parsing or rewriting occurs

### Requirement: JMS selector documentation states broker-side filtering and bridge requirement

The endpoint option table and the JMS guide SHALL state that the selector is applied at the
broker (non-matching messages are withheld before delivery), that selector syntax is
provider-specific, that jms bridge >= 0.11.0 advertising `message-selector-v1` is required, and
what the startup error looks like when the bridge does not support it. The docs SHALL state that
JVM unit tests prove plumbing only and selector semantics are verified by the CI-owned
live-broker suite.

#### Scenario: docs name the version and the error shape

- **GIVEN** the README option table and the JMS guide
- **WHEN** a reader looks up `messageSelector`
- **THEN** the docs state broker-side filtering, required bridge version 0.11.0, and the
  missing-capability startup error text naming `message-selector-v1`

### Requirement: JMS selector filtering is verified against a live broker in CI

Selector semantics (a matching message is delivered, a non-matching message is withheld) SHALL
be verified by a CI-owned container/live-broker test marked as requiring broker infrastructure.
JVM and Rust unit tests SHALL be documented as proving plumbing only.

#### Scenario: live broker withholds non-matching messages

- **GIVEN** a route with a selector consuming from a queue (and, covered by the same suite, a
  topic) against a live broker in the CI-owned container suite
- **WHEN** one matching and one non-matching message are published
- **THEN** the route receives the matching message and the non-matching message is withheld by
  the broker within the test's bounded wait
