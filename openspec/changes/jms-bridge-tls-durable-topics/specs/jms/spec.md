# jms — delta (durable reservation scope updated for durable topic subscribers)

## MODIFIED Requirements

### Requirement: Durable reservation accounting scope and identity

The bridge SHALL account each delivery as a durable reservation, not as a
completed handoff. A reservation SHALL be recorded before the message leaves
the bridge and SHALL include conversion failures and crashes between the record
and the handoff. The accounting domain SHALL be stable across reconnects and
SHALL be scoped to the broker and the destination. The bridge SHALL key a
reservation by the provider `JMSMessageID` and SHALL NOT key it by an exchange
header or an ephemeral subscription identifier.

Rust SHALL derive the broker key in one place as a stable hash of the broker
name, the broker type, and the redacted broker URL. Rust SHALL be the single
source of the default resolution of the journal directory, verified by a
repository source scan: `$XDG_STATE_HOME/rust-camel/jms-bridge` when
`XDG_STATE_HOME` is set, else `$HOME/.local/state/rust-camel/jms-bridge`. When
neither variable resolves, Rust SHALL fail at startup. Rust SHALL pass the
resolved directory (`JMS_ATTEMPT_JOURNAL_DIR`) and the broker key
(`JMS_BRIDGE_BROKER_KEY`) to the bridge through named environment variables on
the initial launch and on every restart launch. Java SHALL NOT re-derive the
default. Neither a raw broker URL nor a raw message ID SHALL be used as a file
path. The journal path SHALL be
`<JMS_ATTEMPT_JOURNAL_DIR>/<broker-key>/<destination-key>.journal`.

The journal domain SHALL be the tuple (`JMS_ATTEMPT_JOURNAL_DIR`, broker key,
destination key) with exactly one live owner at a time. Budgets SHALL be shared
concurrently only within the owning bridge process. A sequential owner that
acquires the domain through lock handoff SHALL inherit the retained counts. A
reservation SHALL be one durable delivery-attempt record; the
`JMS_ATTEMPT_JOURNAL_*` variables and `JMS_MAX_DELIVERY_ATTEMPTS` SHALL count
reservations. Cross-process or cross-host duplicates SHALL be documented as
bounded at N times `JMS_MAX_DELIVERY_ATTEMPTS`, where N SHALL be the number of
distinct journal domains — `(directory, broker key, destination key)` tuples —
consuming the queue within one retention window.

Topic delivery attempts that are NOT durable subscriptions SHALL keep the
subscription-scoped volatile counter, and SHALL document that the topic poison
cap is not crash-stable. Durable topic subscriptions (both `clientId` and
`subscriptionName` configured) SHALL be broker-side durable subscriptions:
their redelivery across reconnects is owned by the broker, and their attempts
SHALL NOT be recorded in the attempt journal or take its lock.

The directory and its files SHALL be owner-private (mode 0700 and 0600 on
POSIX). Producer-only bridges and topic consumption SHALL NOT touch the journal
or take its lock. When `JMS_ATTEMPT_JOURNAL_DIR` is unset or
`JMS_BRIDGE_BROKER_KEY` is unset, the bridge SHALL report readiness and SHALL
serve producers and topic consumption; a queue subscribe SHALL then fail with
gRPC `FAILED_PRECONDITION` naming `JMS_ATTEMPT_JOURNAL_DIR` or
`JMS_BRIDGE_BROKER_KEY`. When either variable is set but invalid
(non-resolvable, not owner-private, or a symlink escape), the bridge SHALL treat
it as a startup permanent failure.

#### Scenario: Consumers of one bridge process share one budget

- **GIVEN** two consumers on one queue served by one bridge process
- **WHEN** one message is redelivered between them
- **THEN** the reservation count SHALL be shared
- **AND** the cap SHALL apply to the queue domain

#### Scenario: Cross-process duplicates are bounded

- **GIVEN** two bridge processes with distinct journal directories consuming one queue
- **WHEN** the same message is redelivered
- **THEN** each directory SHALL keep an independent budget
- **AND** the total SHALL be bounded at N times the configured maximum, where N is the number of distinct journal domains (directory, broker key, destination key) consuming the queue within one retention window

#### Scenario: Unset env defers the failure to queue subscribe

- **GIVEN** a bridge started with `JMS_ATTEMPT_JOURNAL_DIR` unset or `JMS_BRIDGE_BROKER_KEY` unset
- **WHEN** it starts and a producer or topic path runs
- **THEN** readiness SHALL succeed and those paths SHALL work
- **AND** a queue subscribe SHALL fail with `FAILED_PRECONDITION` naming the missing variable

#### Scenario: Invalid env is a startup permanent failure

- **GIVEN** a journal directory that is non-resolvable, not owner-private, or a symlink escape
- **WHEN** the bridge starts
- **THEN** it SHALL signal a startup permanent failure
- **AND** it SHALL NOT report readiness

#### Scenario: Sequential owners inherit counts

- **GIVEN** one journal domain released by a previous owner
- **WHEN** a new owner acquires the domain through lock handoff
- **THEN** it SHALL inherit the retained reservation counts

#### Scenario: Rust owns the key and the default directory

- **GIVEN** a broker name, type, and redacted URL
- **WHEN** Rust launches the bridge
- **THEN** Rust SHALL pass the derived broker key and the resolved directory as environment variables
- **AND** Java SHALL NOT derive a default directory

#### Scenario: Broker and destination namespaces stay separate

- **GIVEN** two brokers and two queues
- **WHEN** each records a reservation
- **THEN** each SHALL use its own broker and destination namespace

#### Scenario: No raw identifiers in paths

- **GIVEN** a credential-bearing broker URL and a raw message ID
- **WHEN** the bridge derives the journal path
- **THEN** neither value SHALL appear in a file path

#### Scenario: Producer and topic paths never lock

- **GIVEN** a producer-only bridge or a topic subscription
- **WHEN** it runs
- **THEN** it SHALL NOT create, lock, or replay a queue journal

#### Scenario: Topic scope is explicit

- **GIVEN** a topic destination
- **WHEN** the bridge consumes it
- **THEN** a non-durable subscription SHALL use a subscription-scoped volatile counter
- **AND** it SHALL document that the cap is not crash-stable
- **AND** a durable subscription SHALL NOT record attempts in the attempt journal (broker-owned redelivery)
