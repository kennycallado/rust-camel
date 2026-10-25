# jms-endpoint-config Specification

## Purpose
TBD - created by archiving change 409-jms-unknown-params. Update Purpose after archive.
## Requirements
### Requirement: Unknown query parameters are collected, not dropped

The JMS endpoint URI parser (`JmsEndpointConfig::from_uri`) SHALL collect
every unrecognized query-parameter key into `unknown_params: Vec<String>` on
the parsed config, in source order, preserving duplicate occurrences, and
SHALL NOT store or echo any parameter value. Before an unknown key is
stored, it SHALL be tested with the existing camel-api credential-shape
predicate (`redact::is_credential_shaped_key`); a credential-shaped key —
including percent-encoded shapes — SHALL be stored as `<redacted>` so the
raw key never enters the struct, its derived `Debug`, or any event. Bare
query segments without `=` SHALL keep today's skip behavior. Known keys —
including all camelCase and snake_case aliases — SHALL match exactly as
today with unchanged semantics, defaults, and error behavior.

#### Scenario: Typo'd parameter key is collected

- **GIVEN** the URI `jms:queue:orders?durableSubscribtion=news`
- **WHEN** `from_uri` parses it
- **THEN** parsing succeeds, all defaults apply, and `unknown_params` equals
  `["durableSubscribtion"]`; the value `"news"` is not stored anywhere

#### Scenario: Secret-shaped value is never echoed

- **GIVEN** an endpoint URI with `?mysteryBox=hunter2`
- **WHEN** the unknown key is collected and the warning is emitted
- **THEN** `mysteryBox` is stored and logged and `hunter2` appears in no
  log event and in no `Debug` output of the parsed config

#### Scenario: Credential-shaped key never echoes

- **GIVEN** an endpoint URI whose query contains
  `user%3Asecret%40host=1` (percent-encoded `user:secret@host`)
- **WHEN** the unknown key is collected and the warning is emitted
- **THEN** the stored key and the logged key are `<redacted>`; neither the
  raw key nor `secret` appears anywhere

#### Scenario: Known alias produces no unknown

- **GIVEN** the URI `jms:queue:orders?concurrent_consumers=3`
- **WHEN** `from_uri` parses it
- **THEN** parsing succeeds, `concurrent_consumers` is `3`, and
  `unknown_params` is empty

#### Scenario: Several unknowns are collected in source order

- **GIVEN** the URI
  `jms:queue:orders?zeta=1&alpha=2&concurrentConsumers=2&beta=3`
- **WHEN** `from_uri` parses it
- **THEN** `unknown_params` equals `["zeta", "alpha", "beta"]` and
  `concurrent_consumers` is `2`

### Requirement: Known-key table is the single source of truth

The set of recognized query-parameter keys SHALL be defined exactly once, in
a constant name table mapping each recognized key literal to a dispatch
enum variant, adjacent to the parser, with the canonical (camelCase) form
first for each alias group. Recognition and dispatch SHALL both derive from
this table: the parser looks the key up in the table and matches on the
resulting variant, and the match SHALL be exhaustive over the enum so that
an enum variant without a handler arm is a compile error, while a handler
arm cannot exist without a variant. A table row pointing at a stub arm
SHALL be caught by a completeness test that parses every table key with a
non-default sample value and asserts the corresponding config field changed
(the struct has no `Default`/`PartialEq`; per-field assertions). The
canonical form used for suggestions SHALL come from the table via an
exhaustive per-variant method, so a new variant cannot skip it.

#### Scenario: Enum variant without handler arm is caught

- **GIVEN** a dispatch enum variant with a name-table row but no handler
  arm
- **WHEN** the crate is compiled
- **THEN** the build fails because the dispatch match is non-exhaustive

#### Scenario: Handler arm without a table row is caught

- **GIVEN** a handler arm is added for a dispatch enum variant that has no
  name-table row
- **WHEN** a URI containing that key is parsed
- **THEN** the table lookup returns "unknown", the key is collected as
  unknown, and the parameter's own feature test fails

#### Scenario: Stub arm is caught by the completeness test

- **GIVEN** a dispatch arm that silences the compiler without mutating any
  config field
- **WHEN** the completeness test parses a URI containing that key with a
  non-default sample value
- **THEN** the test fails because the parsed config field kept its default

### Requirement: Closest-known-key suggestion

The parser SHALL offer a suggestion for each unknown key: the canonical form
of the closest known key group when the normalized edit distance (casefold,
separator-stripped, via the existing header normalization; hand-rolled
Levenshtein, no new dependency) is at most 3 and at most half the normalized
unknown key's length, with keys capped at 64 characters for bounded work and
ties broken by first group in table order. Otherwise the suggestion SHALL
report "no close match". Normalization SHALL be used for suggestions only,
never for accepting a key.

#### Scenario: True near-miss suggests the canonical key

- **GIVEN** the unknown key `concurrentConsumerss`
- **WHEN** a suggestion is computed
- **THEN** the suggestion is `concurrentConsumers`

#### Scenario: Distant unknown reports no close match

- **GIVEN** the unknown key `durableSubscribtion` (no durable parameters
  exist yet; rc-c4fbo M2 owns them)
- **WHEN** a suggestion is computed
- **THEN** the result is "no close match"

### Requirement: One actionable warning per endpoint with unknowns

At endpoint creation (`create_endpoint`), when the parsed config has
unknown parameters, the component SHALL emit exactly one `tracing::warn!`
for that endpoint listing every unknown key in source order, each with its
suggestion or "no close match", and the hint that the parameter was
ignored. The event SHALL carry keys and suggestions only — never a
parameter value, never the URI. Endpoints without unknowns SHALL emit no
warning. Direct `from_uri` calls outside endpoint creation SHALL stay
silent. Unknown parameters SHALL NOT fail the boot and SHALL NOT be logged
at `error`.

#### Scenario: One warning names keys and ignored hint

- **GIVEN** a route creating an endpoint from
  `jms:queue:orders?durableSubscribtion=news&concurrentConsumerss=2`
- **WHEN** the endpoint is created
- **THEN** exactly one warn-level event is emitted containing both keys,
  `no close match` for the first, the `concurrentConsumers` suggestion for
  the second, and the ignored hint; the values `news` and `2` appear
  nowhere in the event

#### Scenario: Secret-shaped value is never echoed

- **GIVEN** an endpoint URI with `?mysteryBox=hunter2`
- **WHEN** the unknown key is collected and the warning is emitted
- **THEN** `mysteryBox` is stored and logged; `hunter2` appears in no log
  event and in no `Debug` output of the parsed config

#### Scenario: Clean endpoint is silent

- **GIVEN** a route creating an endpoint from
  `jms:queue:orders?concurrent_consumers=3`
- **WHEN** the endpoint is created
- **THEN** no unknown-parameter warning is emitted

