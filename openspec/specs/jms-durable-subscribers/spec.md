# jms-durable-subscribers Specification

## Purpose
TBD - created by archiving change jms-bridge-tls-durable-topics. Update Purpose after archive.
## Requirements
### Requirement: Durable topic subscription identity rides the subscribe request

The JMS component SHALL accept `clientId` (alias `client_id`) and `subscriptionName`
(alias `subscription_name`) as known URI parameters on the endpoint URI, collected by
the known-key table (`KNOWN_PARAM_NAMES` as the single naming authority, both alias
forms). Both values SHALL be carried on every `SubscribeRequest` proto call in the
new `client_id` and `subscription_name` fields, so every subscribe attempt — first
attach and every reconnect — presents the identical durable identity (`client_id`
and `subscription_name` stable across reconnects; the per-stream `subscription_id`
keeps its existing fresh-per-stream behavior and is NOT part of the durable
identity). Validation SHALL
fail loud at URI-parse time: the two parameters are accepted only together
(either alone is a permanent config error naming the missing pair), both must be
non-blank after trim, either parameter on a `queue:` destination is a permanent
config error (durability is a topic-only contract), and a durable endpoint SHALL
reject `concurrentConsumers` greater than `1` (one client id serves exactly one
dedicated connection). No environment variable SHALL
carry these options.

#### Scenario: durable params parse and reach the subscribe request

- **GIVEN** the URI `jms:topic:prices?clientId=app-1&subscriptionName=snapshots`
- **WHEN** the endpoint config parses and the consumer subscribes
- **THEN** the Subscribe call carries `client_id` = `app-1` and
  `subscription_name` = `snapshots` on every attempt

#### Scenario: snake_case aliases resolve to the same values

- **GIVEN** the URI `jms:topic:prices?client_id=app-1&subscription_name=snapshots`
- **WHEN** the endpoint config parses
- **THEN** the stored values match the camelCase form exactly

#### Scenario: one parameter alone is a permanent config error

- **GIVEN** the URI `jms:topic:prices?subscriptionName=snapshots`
- **WHEN** the endpoint config parses
- **THEN** parsing fails permanently with an error naming `clientId` as required
  together with `subscriptionName`

#### Scenario: durable params on a queue destination are rejected

- **GIVEN** the URI `jms:queue:orders?clientId=app-1&subscriptionName=snapshots`
- **WHEN** the endpoint config parses
- **THEN** parsing fails permanently with an error stating durable subscription
  parameters require a topic destination

#### Scenario: blank values are rejected

- **GIVEN** the URI `jms:topic:prices?clientId=&subscriptionName=snapshots`
- **WHEN** the endpoint config parses
- **THEN** parsing fails permanently with an error naming the blank parameter

#### Scenario: durable identity with concurrent consumers is rejected

- **GIVEN** the URI
  `jms:topic:prices?clientId=app-1&subscriptionName=snapshots&concurrentConsumers=2`
- **WHEN** the endpoint config parses
- **THEN** parsing fails permanently with an error stating a durable endpoint
  requires `concurrentConsumers` of exactly 1 (two connections cannot share one
  client id)

#### Scenario: unknown-key diagnostics still cover misspellings

- **GIVEN** the URI `jms:topic:prices?durableSubscribtion=news`
- **WHEN** the endpoint config parses
- **THEN** the key is reported as unknown (collected with a closest-known-key
  suggestion), consistent with the endpoint-config unknown-key contract

### Requirement: Bridge creates durable topic consumers

The bridge SHALL create a durable topic subscription when the Subscribe request
carries a non-empty `client_id` and `subscription_name` for a topic destination:
the connection SHALL have the configured client id set before `connection.start()`
and the consumer SHALL be created with
`createDurableConsumer(topic, subscriptionName, selector, noLocal=false)`, keeping
the request's message selector behavior unchanged. A request carrying only one of
the two fields, or either field on a queue destination, SHALL fail the subscribe
permanently (INVALID_ARGUMENT class) with an error that never echoes credential
values. A durable consumer uses the dedicated (non-pooled) connection path exactly
as volatile consumers do.

#### Scenario: durable consumer construction on a capable bridge

- **GIVEN** a Subscribe request with `client_id=app-1`, `subscription_name=snapshots`,
  and a `topic:` destination
- **WHEN** the bridge subscribes
- **THEN** the dedicated connection's client id is `app-1` before start and the
  consumer is a durable consumer named `snapshots` with `noLocal=false`

#### Scenario: selector and durable combine

- **GIVEN** a durable Subscribe request whose `message_selector` is non-empty
- **WHEN** the bridge subscribes
- **THEN** the durable consumer is created with the selector passed through
  verbatim to the provider

#### Scenario: half-specified durable identity fails permanently

- **GIVEN** a Subscribe request with `client_id` set but `subscription_name` empty
- **WHEN** the bridge subscribes
- **THEN** the subscribe fails permanently with INVALID_ARGUMENT and no consumer
  thread starts

### Requirement: Durable subscriptions survive reconnect

A durable topic subscription SHALL survive disconnect and reconnect: when the
consumer's connection is lost (broker blip or bridge restart) and the Rust
consumer loop re-subscribes, the bridge SHALL re-attach
to the same broker subscription (same client id + subscription name — the durable
identity; each replacement stream keeps a fresh `subscription_id`) and messages
published to the topic while disconnected SHALL be delivered after reconnect.
Client-id identity SHALL come from route configuration (stable across reconnects),
never from per-process generated state.

#### Scenario: messages published while disconnected are received after reconnect

- **GIVEN** a live durable subscription on a real broker
- **WHEN** the subscription disconnects, a message is published to the topic, and
  the consumer reconnects with the same `clientId`/`subscriptionName`
- **THEN** the published message is delivered to the reconnected consumer

#### Scenario: reconnect identity is stable

- **GIVEN** a durable route whose subscribe stream fails and retries
- **WHEN** each retry reaches the bridge
- **THEN** every Subscribe request carries the same `client_id` and
  `subscription_name` as the first attach, while each replacement stream carries a
  fresh `subscription_id` (existing behavior unchanged)

#### Scenario: colliding client id fails loud as CLIENT_ID_IN_USE

- **GIVEN** a second consumer dialing the same broker with a client id already
  bound to another connection
- **WHEN** the broker rejects the connection with `InvalidClientIDException`
- **THEN** the failure is classified `CLIENT_ID_IN_USE` by the bridge classifier
  (new mapping added by this change; the existing `IllegalStateException` mapping
  is unchanged) and the error carries no credential or remote-text echo

### Requirement: Durable subscriber support fails closed against bridges without the capability

The bridge SHALL advertise `durable-subscribers-v1` in the `capabilities` of its
Health response. When a durable route (both `clientId` and `subscriptionName`
configured) connects to a bridge whose Health evidence does not advertise that
capability, the system SHALL fail closed following the `broker-tls-v1` /
`message-selector-v1` gate pattern: permanent park naming the missing capability
and the required bridge version (jms bridge >= 0.9.0) plus the pinned bridge
version the component was built for, zero Subscribe calls sent,
runtime capability loss parking permanently and stopping resubscription, and
capability evidence re-collected per bridge instance (no cross-instance
inheritance). Health statuses `STALE`/`UNKNOWN`/`SKIPPED`/`UNSPECIFIED` SHALL
neither bypass nor weaken the gate; a never-completing Health RPC follows the
existing bounded startup-failure policy. Non-durable routes SHALL be unaffected
by the gate.

#### Scenario: startup fail-closed with no silent downgrade

- **GIVEN** a durable route and a bridge whose Health response reports an empty
  capabilities list
- **WHEN** the slot starts up
- **THEN** the slot parks permanently with an error containing
  `durable-subscribers-v1` and zero Subscribe calls are sent (proto3 unknown-field
  tolerance must never silently downgrade durability)

#### Scenario: durable route on a capable bridge subscribes

- **GIVEN** a durable route and a bridge advertising `durable-subscribers-v1`
- **WHEN** the slot starts up
- **THEN** the gate passes and the Subscribe call carries the durable identity

#### Scenario: non-durable route on an old bridge is unaffected

- **GIVEN** a consumer with no durable parameters and a bridge without
  `durable-subscribers-v1`
- **WHEN** the slot starts up
- **THEN** the gate passes trivially and the subscription proceeds

#### Scenario: runtime capability loss parks permanently

- **GIVEN** a running durable route whose bridge advertised
  `durable-subscribers-v1` at startup
- **WHEN** a later Health response from the replaced bridge lacks the capability
- **THEN** the slot parks permanently with the missing-capability error and does
  not resubscribe

#### Scenario: replacement bridge cannot inherit previous evidence

- **GIVEN** a daemon restart (or slot replacement) where the previous bridge
  instance advertised `durable-subscribers-v1` but the new instance does not
- **WHEN** the startup gate evaluates for the replacement
- **THEN** capability evidence re-collected from the current instance governs and
  the gate fails closed

#### Scenario: capability evidence is authoritative regardless of health status

- **GIVEN** a durable route and a bridge whose Health response reports broker
  status `STALE` (or `UNKNOWN`/`SKIPPED`) but advertises `durable-subscribers-v1`
- **WHEN** the startup gate evaluates
- **THEN** the gate passes on the capability alone (broker health status neither
  bypasses nor weakens the capability check)

#### Scenario: failed or timed-out Health verification follows the bounded startup policy

- **GIVEN** a durable route whose bridge Health RPC never completes
- **WHEN** the startup gate waits for capability evidence
- **THEN** the existing bounded startup-failure policy applies (no indefinite
  wait, no bypass of the capability check)

### Requirement: Explicit client id overrides clientIDPrefix autogeneration

The bridge SHALL guarantee that an explicit durable client id wins over the
ActiveMQ Classic `clientIDPrefix` autogeneration: a dedicated connection created
for a durable subscription SHALL report exactly the configured client id after
start. The process-unique prefix machinery (rc-trxge) SHALL keep governing
generated identities of non-durable connections unchanged.

#### Scenario: dedicated durable connection reports the configured client id

- **GIVEN** a durable Subscribe request with `client_id=app-1` against the
  production factory (clientIDPrefix always set)
- **WHEN** the dedicated connection is created, configured, and started
- **THEN** `getClientID()` returns exactly `app-1`

### Requirement: Durable subscriber documentation states persistence semantics

The bridge documentation SHALL document durable topic subscribers with a working
example: URI parameters, the `durable-subscribers-v1` bridge requirement, the
client-id uniqueness duty of the operator (colliding subscribers fail loud),
persistence across route cancel and bridge restart, that the subscription is NOT
deleted on cancel (broker-side cleanup), and one `ssl://` TLS broker example for
the transport feature.

#### Scenario: README carries the durable example and semantics

- **GIVEN** the bridge README
- **WHEN** an operator reads the durable subscriber section
- **THEN** it contains the URI example, capability name, uniqueness duty,
  persistence semantics, and the no-unsubscribe-on-cancel note; release notes
  generated from Conventional Commits record both features

