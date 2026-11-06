## MODIFIED Requirements

### Requirement: JMS producer forwards only allowlisted non-reserved headers

The Rust JMS producer SHALL forward an exchange header only when the endpoint
`jmsHeaderAllowlist` (alias `jms_header_allowlist`) contains its normalized
name. Normalization SHALL casefold and remove `-`, `_`, and `.`. The allowlist
SHALL default to empty. Before the allowlist check the producer SHALL deny, in
normalized form, a reserved JMS name, the `jms` provider-control namespace
(`JMS`, `JMSX`, and `JMS_`), a framework-internal name in the `camel` or
`xcamel` namespace, and any name that contains a credential substring. The
credential substring set SHALL reuse the existing
`BROKER_SENSITIVE_KEY_SUBSTRINGS` constant and SHALL add `apikey`,
`authorization`, `bearer`, and `principal`. Denial SHALL match
case-insensitively and SHALL catch separator variants such as `api-key`,
`api_key`, and `API_KEY`. Producer QoS headers SHALL remain set after header
extraction, so the allowlist does not regress them. `Content-Type` SHALL remain
a dedicated protocol field and inbound mapping SHALL stay unchanged. No denied
header SHALL appear in the IPC send payload or as a JMS property. The Java
bridge SHALL apply the same normalized rules in a `HeaderPolicy` helper,
verified by a parity test.

When the broker or its JMS client rejects the write of a forwarded, non-denied
property, the Java bridge SHALL fail the send with a typed
`InvalidPropertyException` whose message names every offending property name
and carries no property value, no credential material, and no host or
connection detail. The bridge SHALL NOT sanitize, rename, or drop the
offending property, SHALL NOT downgrade the failure to a log entry, and SHALL
attempt the write of every remaining non-denied property so that legal-name
forwarding does not depend on position in the header map. The
`JmsBridgeService.send` handler SHALL map `InvalidPropertyException` to gRPC
`INVALID_ARGUMENT`, keeping the generic `INTERNAL` mapping for all other send
failures. The rejection SHALL NOT change broker-health state, restart
semantics, or the durable-subscriber contract. Legal-name forwarding SHALL
remain byte-identical to the pre-change behaviour. The legality set of
property names is broker-dependent (the JMS identifier rule excludes `-`;
Classic tolerates hyphenated names and Artemis rejects them) and the bridge
SHALL document this difference instead of encoding it.

#### Scenario: Empty allowlist forwards no user headers

- **GIVEN** no `jmsHeaderAllowlist` is configured
- **WHEN** a route sends an exchange with user headers
- **THEN** the IPC send payload SHALL carry no user header
- **AND** the bridge SHALL set no JMS property for them

#### Scenario: A listed business header is forwarded

- **GIVEN** `jmsHeaderAllowlist=customerId,region`
- **WHEN** a route sends `customerId=42` and `region=eu`
- **THEN** both headers SHALL reach the bridge and become JMS properties

#### Scenario: Reserved and framework-internal names are denied even when listed

- **GIVEN** `jmsHeaderAllowlist=JMSMessageID,JMSXDeliveryCount,CamelStop,x-camel-leader-epoch`
- **WHEN** a route sends headers with those names
- **THEN** the IPC payload SHALL omit all of them
- **AND** the bridge SHALL set no JMS property for them

#### Scenario: Credential variants are denied

- **GIVEN** `jmsHeaderAllowlist=api-key,API_KEY,api_key,Password,Authorization`
- **WHEN** a route sends headers with those names
- **THEN** the IPC payload SHALL omit all of them
- **AND** the credential set SHALL reuse `BROKER_SENSITIVE_KEY_SUBSTRINGS`

#### Scenario: Credentials never reach the wire

- **GIVEN** a route that sets `password`, `Authorization`, `api-key`, and a benign `traceId`, with a non-empty allowlist that names them
- **WHEN** the exchange is sent
- **THEN** a positive test SHALL find `traceId` on the wire
- **AND** a negative test SHALL find no credential name or value in the IPC payload or in the broker JMS properties

#### Scenario: Java applies the same rules

- **GIVEN** a shared corpus of header names
- **WHEN** the Rust producer and the Java `HeaderPolicy` classify them
- **THEN** both SHALL deny the same names and allow the same names

#### Scenario: A rejected property write fails the send with a name-only typed error

- **GIVEN** a non-empty allowlist that names `x-crash-proof` and `traceId`
- **WHEN** the broker client rejects the `x-crash-proof` property write while accepting `traceId`
- **THEN** the send SHALL fail with `InvalidPropertyException`
- **AND** the error message SHALL contain `x-crash-proof`
- **AND** the error message SHALL NOT contain the property value, any credential material, or any host or connection detail

#### Scenario: Multiple offending names are reported in one send

- **GIVEN** a non-empty allowlist that names two broker-rejected properties and one legal property
- **WHEN** the send is attempted
- **THEN** the error message SHALL name both rejected properties
- **AND** the legal property write SHALL still have been attempted

#### Scenario: A rejected property write is classified as a sender error, not a broker failure

- **GIVEN** a send whose property write the broker client rejects
- **WHEN** the `JmsBridgeService.send` handler observes the `InvalidPropertyException`
- **THEN** the gRPC status SHALL be `INVALID_ARGUMENT`
- **AND** the bridge SHALL NOT record a broker-health regression, restart the bridge, or attach a `BrokerFailureClass` to the rejection

#### Scenario: Legal-name forwarding is byte-identical

- **GIVEN** an allowlist of legal names such as `traceId` and `orderId`
- **WHEN** the message is sent to any broker
- **THEN** the broker-visible properties for those names SHALL be identical to the pre-change behaviour
- **AND** the send SHALL report success with the JMS message id

#### Scenario: An illegal name on Artemis dead-letters with a name-only error

- **GIVEN** the containerized Artemis fixture and an allowlist that names `x-crash-proof`
- **WHEN** a route sends an exchange carrying `x-crash-proof`
- **THEN** the send SHALL fail and the route error SHALL name `x-crash-proof`
- **AND** the error SHALL carry no property value
- **AND** a control route carrying a legal name SHALL deliver the message with that property intact
