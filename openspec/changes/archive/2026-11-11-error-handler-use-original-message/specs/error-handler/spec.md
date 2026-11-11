## ADDED Requirements

### Requirement: opt-in original-message restoration before dead-letter dispatch

When a route's `error_handler` sets `use_original_message: true`, the
system SHALL snapshot the inbound message before route processing and
restore that original message before dispatching the failed exchange to
the dead-letter channel (or a matched `on_exceptions` clause's
`handled_by` delegate). The YAML DSL SHALL carry the flag from the
document through `DeclarativeErrorHandler` into the compiled
`ErrorHandlerConfig`, so the restoration the runtime already implements
becomes reachable from YAML routes. When the flag is unset the behavior
SHALL stay opt-out: the dead-letter channel receives the exchange as
mutated by route processing (no snapshot, no restoration). The flag
SHALL survive every `error_handler` config shape: `dead_letter_channel`
only, top-level `retry` only, and `on_exceptions` clauses. Exception:
when a route step replaces the Exchange object wholesale, the stashed
snapshot is not carried and restoration cannot occur (a documented,
test-enforced limitation of the runtime snapshot mechanism); for that
exchange the flag behaves as if unset.

#### Scenario: YAML use_original_message true routes the original body to the DLC

- **GIVEN** a YAML route where a producer route sends `ORIGINAL-EVENT`
  to a consumer route whose `error_handler` sets
  `dead_letter_channel: "mock:dlc"` and `use_original_message: true`,
  and whose steps set the body to `MUTATED` and then fail
- **WHEN** the input is processed and the step failure is handled
- **THEN** `mock:dlc` receives exactly one exchange whose body is
  `ORIGINAL-EVENT` (the pre-mutation message, not the mutated one)

#### Scenario: default (flag unset) routes the mutated body to the DLC

- **GIVEN** the same YAML route but WITHOUT `use_original_message`
- **WHEN** the step failure is handled
- **THEN** `mock:dlc` receives exactly one exchange whose body is
  `MUTATED` (opt-out behavior is preserved)

#### Scenario: the flag survives the dead-letter-channel-only shape

- **GIVEN** a `DeclarativeErrorHandler` with only `dead_letter_channel`
  set and `use_original_message: true`
- **WHEN** the handler is compiled
- **THEN** the resulting `ErrorHandlerConfig` carries
  `use_original_message == true`

#### Scenario: the flag survives the retry-only shape

- **GIVEN** a `DeclarativeErrorHandler` with only a top-level `retry`
  policy and `use_original_message: true`
- **WHEN** the handler is compiled
- **THEN** the resulting `ErrorHandlerConfig` carries
  `use_original_message == true` together with the retry policy

#### Scenario: the flag survives the on_exceptions shape

- **GIVEN** a `DeclarativeErrorHandler` with `on_exceptions` clauses and
  `use_original_message: true`
- **WHEN** the handler is compiled
- **THEN** the resulting `ErrorHandlerConfig` carries
  `use_original_message == true` together with the compiled exception
  policies

#### Scenario: the default is false when the flag is absent

- **GIVEN** any `DeclarativeErrorHandler` shape without the flag
- **WHEN** the handler is compiled
- **THEN** the resulting `ErrorHandlerConfig` carries
  `use_original_message == false`
