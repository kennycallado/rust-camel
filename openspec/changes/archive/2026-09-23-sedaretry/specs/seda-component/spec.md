## MODIFIED Requirements

### Requirement: typed SEDA no-active-consumers gate classification

The SEDA crate SHALL classify a pipeline error as the no-active-consumers
gate only when the error carries the crate's typed gate rejection marker —
an endpoint-creation failure whose source chain, within the classifier's
bounded walk depth of 8 source hops, contains the marker type. The marker
type SHALL NOT be constructible or nameable outside the SEDA crate, and
the source handle carrying it SHALL NOT be cloneable or extractable
outside camel-api, so foreign code cannot forge gate provenance. Message
text SHALL play no part in classification. Every endpoint-creation
failure that lacks the gate marker AND the terminal-config marker —
whatever its text — SHALL report `is_no_active_consumers_gate` as false
and `is_direct_startup_race` as true (retryable), except the terminal-config
carrier defined by the typed terminal-config-error classification
requirement. The crate SHALL construct every gate rejection through one
in-crate constructor, and the rejection's outer detail SHALL stay
byte-identical to the historical wording (`SEDA endpoint '<name>' has no
active consumers` single mode, `SEDA endpoint '<name>' has no active
subscribers` fanout mode); the marker's own Display is non-canonical
diagnostic text (it never equals a gate message and never participates
in classification). A marker deeper than 8 source hops SHALL NOT classify
the error as the gate (bounded walk; tested at and beyond the limit).

#### Scenario: genuine single-mode gate fails fast

- **GIVEN** a SEDA single-mode endpoint with no active consumer
- **WHEN** a producer send rejects through the gate constructor with the
  typed marker in the source chain
- **THEN** `is_no_active_consumers_gate` reports true and
  `is_direct_startup_race` reports false (fail fast, no retry)

#### Scenario: genuine fanout-mode gate fails fast

- **GIVEN** a SEDA fanout-mode endpoint with no active subscriber, rejected
  at the pre-enqueue gate or at the subscriber-list check
- **WHEN** the producer send rejects through the gate constructor
- **THEN** `is_no_active_consumers_gate` reports true and
  `is_direct_startup_race` reports false

#### Scenario: gate marker deeper in the source chain still classifies

- **GIVEN** an endpoint-creation failure whose source chain contains an
  intermediate wrapper error that in turn sources the typed gate marker,
  with the marker within 8 source hops
- **WHEN** the classification runs
- **THEN** `is_no_active_consumers_gate` reports true and
  `is_direct_startup_race` reports false

#### Scenario: marker beyond the walk limit stays retryable

- **GIVEN** an endpoint-creation failure whose source chain carries the
  typed gate marker deeper than 8 source hops
- **WHEN** the classification runs
- **THEN** `is_no_active_consumers_gate` reports false and
  `is_direct_startup_race` reports true (bounded walk, boundary tested)

#### Scenario: foreign message containing the consumer wording stays retryable

- **GIVEN** a foreign component failure rendered as a plain
  `EndpointCreationFailed` whose text contains "has no active consumers"
  as a substring
- **WHEN** the classification runs
- **THEN** `is_no_active_consumers_gate` reports false and
  `is_direct_startup_race` reports true (retryable within the bounded
  window)

#### Scenario: foreign message containing the subscriber wording stays retryable

- **GIVEN** a plain `EndpointCreationFailed` whose text contains
  "has no active subscribers" as a substring
- **WHEN** the classification runs
- **THEN** `is_no_active_consumers_gate` reports false and
  `is_direct_startup_race` reports true

#### Scenario: byte-exact imitation of the consumer wording stays retryable

- **GIVEN** a plain `EndpointCreationFailed` whose text is byte-identical
  to the canonical single-mode gate message
  (`SEDA endpoint 'q' has no active consumers`) but carries no typed
  marker
- **WHEN** the classification runs
- **THEN** `is_no_active_consumers_gate` reports false and
  `is_direct_startup_race` reports true

#### Scenario: byte-exact imitation of the subscriber wording stays retryable

- **GIVEN** a plain `EndpointCreationFailed` whose text is byte-identical
  to the canonical fanout-mode gate message
  (`SEDA endpoint 'q' has no active subscribers`) but carries no typed
  marker
- **WHEN** the classification runs
- **THEN** `is_no_active_consumers_gate` reports false and
  `is_direct_startup_race` reports true

#### Scenario: decorated gate wording stays retryable

- **GIVEN** a plain `EndpointCreationFailed` whose text is the canonical
  gate message with extra prefix, suffix, or case modification
- **WHEN** the classification runs
- **THEN** `is_no_active_consumers_gate` reports false and
  `is_direct_startup_race` reports true

#### Scenario: typed endpoint failure without the gate marker stays retryable

- **GIVEN** an endpoint-creation failure variant carrying a source chain
  that does not contain the gate marker (a foreign or future
  source-preserving endpoint failure, including one whose source is a
  terminal-config marker of a FOREIGN crate)
- **WHEN** the classification runs
- **THEN** `is_no_active_consumers_gate` reports false and
  `is_direct_startup_race` reports true

#### Scenario: other SEDA endpoint-creation failures stay retryable

- **GIVEN** a SEDA endpoint-creation failure with any other wording
  (queue-full, enqueue or fanout timeout, or a passthrough creation
  failure)
- **WHEN** the classification runs
- **THEN** `is_no_active_consumers_gate` reports false and
  `is_direct_startup_race` reports true (documented residual, unchanged;
  the multipleConsumers configuration rejection moved to the terminal-config
  classification — see the typed terminal-config-error requirement)

#### Scenario: canonical wording and aliases are preserved

- **GIVEN** a gate rejection produced at any of the three gate sites
- **WHEN** the error renders and is inspected
- **THEN** the outer detail is byte-identical to the historical wording,
  `variant_name()` reports `EndpointCreationFailed`, and `classify()`
  reports `endpoint`

## ADDED Requirements

### Requirement: typed SEDA terminal-config-error classification

The SEDA crate SHALL classify a pipeline error as a terminal
configuration error only when the error carries the crate's typed
terminal-config marker — an `EndpointCreationFailedWithSource` whose
source chain, within the classifier's bounded walk depth of 8 source
hops, contains the marker type. The marker type SHALL NOT be
constructible or nameable outside the SEDA crate. Message text SHALL
play no part in classification. The crate SHALL construct every
terminal-config rejection through one in-crate constructor, and the
multipleConsumers+wait rejection's outer detail SHALL stay byte-identical
to the historical wording (`multipleConsumers=true with
waitForTaskToComplete != Never is not supported — a single request
cannot have N valid replies without aggregator semantics`). The marker's
own Display SHALL be non-canonical diagnostic text that never equals a
canonical rejection message. `is_direct_startup_race` SHALL report false
for an error carrying the marker within the walk limit (the combination
is deterministic — a retry can never succeed), and true when the marker
sits deeper than 8 source hops (bounded walk; boundary tested at the
limit and beyond). The plain `EndpointCreationFailed` variant SHALL
never classify as a terminal-config error.

#### Scenario: genuine multipleConsumers+wait reject fails fast

- **GIVEN** a SEDA fanout endpoint (`multipleConsumers=true`) with an
  active consumer and a producer whose `waitForTaskToComplete` is not
  `Never`
- **WHEN** the producer send rejects through the terminal-config
  constructor
- **THEN** `is_seda_terminal_config_error` reports true and
  `is_direct_startup_race` reports false (terminal, no retry)

#### Scenario: foreign imitation of the config wording stays retryable

- **GIVEN** a plain `EndpointCreationFailed` whose text byte-matches the
  canonical multipleConsumers+wait wording
- **WHEN** the classification runs
- **THEN** `is_seda_terminal_config_error` reports false and
  `is_direct_startup_race` reports true (typed provenance only)

#### Scenario: typed source without the terminal-config marker stays retryable

- **GIVEN** an `EndpointCreationFailedWithSource` whose source chain
  carries a foreign error type instead of the marker
- **WHEN** the classification runs
- **THEN** `is_seda_terminal_config_error` reports false and
  `is_direct_startup_race` reports true

#### Scenario: terminal-config marker at exactly the walk limit classifies

- **GIVEN** an endpoint-creation failure whose source chain carries the
  typed terminal-config marker at exactly 8 source hops
- **WHEN** the classification runs
- **THEN** `is_seda_terminal_config_error` reports true and
  `is_direct_startup_race` reports false (inclusive limit)

#### Scenario: terminal-config marker beyond the walk limit stays retryable

- **GIVEN** an endpoint-creation failure whose source chain carries the
  typed terminal-config marker deeper than 8 source hops
- **WHEN** the classification runs
- **THEN** `is_seda_terminal_config_error` reports false and
  `is_direct_startup_race` reports true (bounded walk, boundary tested)
