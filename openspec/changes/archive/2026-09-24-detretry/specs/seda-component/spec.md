## MODIFIED Requirements

### Requirement: typed SEDA terminal-config-error classification

The SEDA crate SHALL classify a pipeline error as a terminal
configuration error only when the error carries the crate's typed
terminal-config marker — an `EndpointCreationFailedWithSource` whose
source chain, within the classifier's bounded walk depth of 8 source
hops, contains the marker type. The marker type SHALL NOT be
constructible or nameable outside the SEDA crate. Message text SHALL
play no part in classification. The crate SHALL construct every
terminal-config rejection through in-crate constructors, and each
rejection's outer detail SHALL stay byte-identical to the historical
wording: the multipleConsumers+wait rejection
(`multipleConsumers=true with waitForTaskToComplete != Never is not
supported — a single request cannot have N valid replies without
aggregator semantics`) and the endpoint-config-conflict rejection
(the `is_compatible_with` incompatibility detail, `endpoint '<name>'
already exists with different config: <diffs>`, produced when an
endpoint is requested with a config incompatible with the existing
endpoint state of the same name). The marker SHALL cover both terminal
variants — the multipleConsumers+wait conflict and the endpoint config
conflict. The marker's own Display SHALL be non-canonical diagnostic
text that never equals a canonical rejection message.
`is_seda_terminal_config_error` SHALL report true for an error carrying
either marker variant within the walk limit, and
`is_direct_startup_race` SHALL report false for the same (both
combinations are deterministic — no consumer timing and no retry can
ever satisfy them), and true when the marker sits deeper than 8 source
hops (bounded walk; boundary tested at the limit and beyond). The plain
`EndpointCreationFailed` variant SHALL never classify as a
terminal-config error.

#### Scenario: genuine multipleConsumers+wait reject fails fast

- **GIVEN** a SEDA fanout endpoint (`multipleConsumers=true`) with an
  active consumer and a producer whose `waitForTaskToComplete` is not
  `Never`
- **WHEN** the producer send rejects through the terminal-config
  constructor
- **THEN** `is_seda_terminal_config_error` reports true and
  `is_direct_startup_race` reports false (terminal, no retry)

#### Scenario: genuine endpoint config conflict fails fast

- **GIVEN** a SEDA endpoint state registered for name `<n>` with one
  config, and an endpoint creation request for the same name with an
  incompatible config (for example a different `size`)
- **WHEN** `create_endpoint` rejects through the config-conflict
  constructor
- **THEN** `is_seda_terminal_config_error` reports true and
  `is_direct_startup_race` reports false (deterministic conflict, no
  retry on any arm)

#### Scenario: config-conflict rejection detail stays byte-identical

- **GIVEN** an endpoint request whose config conflicts with the existing
  endpoint state (`size: 10 vs 5` diff)
- **WHEN** the rejection renders
- **THEN** the outer detail reads exactly `endpoint '<name>' already
  exists with different config: size: 10 vs 5` (the historical
  `is_compatible_with` wording, unchanged by the typed carrier)

#### Scenario: foreign imitation of the config wording stays retryable

- **GIVEN** a plain `EndpointCreationFailed` whose text byte-matches the
  canonical config-conflict wording
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
