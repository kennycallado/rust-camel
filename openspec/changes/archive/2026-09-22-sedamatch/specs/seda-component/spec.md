## ADDED Requirements

### Requirement: typed SEDA no-active-consumers gate classification

The SEDA crate SHALL classify a pipeline error as the no-active-consumers
gate only when the error carries the crate's typed gate rejection marker —
an endpoint-creation failure whose source chain, within the classifier's
bounded walk depth of 8 source hops, contains the marker type. The marker
type SHALL NOT be constructible or nameable outside the SEDA crate, and
the source handle carrying it SHALL NOT be cloneable or extractable
outside camel-api, so foreign code cannot forge gate provenance. Message
text SHALL play no part in classification. Every
`EndpointCreationFailed` that lacks the marker — whatever its text — SHALL
report `is_no_active_consumers_gate` as false and `is_direct_startup_race`
as true (retryable). The crate SHALL construct every gate rejection
through one in-crate constructor, and the rejection's outer detail SHALL
stay byte-identical to the historical wording
(`SEDA endpoint '<name>' has no active consumers` single mode,
`SEDA endpoint '<name>' has no active subscribers` fanout mode); the
marker's own Display is non-canonical diagnostic text (it never equals a
gate message and never participates in classification). A marker deeper
than 8 source hops SHALL NOT classify the error as the gate (bounded
walk; tested at and beyond the limit).

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
  source-preserving endpoint failure)
- **WHEN** the classification runs
- **THEN** `is_no_active_consumers_gate` reports false and
  `is_direct_startup_race` reports true

#### Scenario: other SEDA endpoint-creation failures stay retryable

- **GIVEN** a SEDA endpoint-creation failure with any other wording
  (queue-full, enqueue or fanout timeout, multipleConsumers configuration
  rejection, or a passthrough creation failure)
- **WHEN** the classification runs
- **THEN** `is_no_active_consumers_gate` reports false and
  `is_direct_startup_race` reports true (documented residual, unchanged)

#### Scenario: canonical wording and aliases are preserved

- **GIVEN** a gate rejection produced at any of the three gate sites
- **WHEN** the error renders and is inspected
- **THEN** the outer detail is byte-identical to the historical wording,
  `variant_name()` reports `EndpointCreationFailed`, and `classify()`
  reports `endpoint`
