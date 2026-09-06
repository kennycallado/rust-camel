## ADDED Requirements

### Requirement: Scenario boot shares the camel run composition root

The FULL-tier scenario boot SHALL boot through the same composition-root
wiring as `camel run` (ADR-0069 §4, §10), in the same order: sealed
config load, context preparation, security compile-context build,
bind-acknowledgement install, component-bundle cascade, route discovery,
SQL startup checks from the discovered definitions, route registration,
context start. It SHALL build the security compile context from the
sealed `CamelConfig` through the shared builder, install `[binds]`
public-exposure acknowledgements through the shared installer, register
ADR-0033 fail-closed SQL startup checks from the discovered routes, and
load the document's route source through camel-dsl route discovery
(two-pass template materialization included) with the config's
`stream_caching.threshold` and the built security context. All
`${env:NAME}` resolution SHALL go exclusively through the scenario layered
environment injected as the discovery lookup — the scenario boot SHALL NOT
resolve placeholders through the process or ambient environment.
Keycloak/oidc security configuration SHALL be rejected before any builder
call with `CamelError::AuthProviderUnavailable`; the scenario runner
SHALL classify that variant as the `infra-unavailable` document-error
class (classification by variant, never message text) — the tier runs
offline (no network), and only the native provider is supported in v1.
Wasm `security.policies`/`security.permissions` SHALL be rejected
fail-closed with a configuration error naming the v1 tier limitation.

#### Scenario: security_policy route boots in the tier

- **GIVEN** a scenario project whose Camel.toml declares native security
  and whose route file declares a route with a `security_policy`
- **WHEN** the scenario boot runs
- **THEN** the route boots successfully where the hand-rolled boot
  previously failed with the default security compile context

#### Scenario: keycloak config rejected offline

- **GIVEN** a scenario project whose Camel.toml declares keycloak/oidc
  security
- **WHEN** the scenario boot runs
- **THEN** the boot fails with `CamelError::AuthProviderUnavailable`, no
  network access is attempted, and the runner reports the
  `infra-unavailable` document-error class for this rejection (not
  `full-boot-failure`)

#### Scenario: wasm security policies rejected in v1

- **GIVEN** a scenario project whose Camel.toml declares wasm
  `security.policies` or `security.permissions`
- **WHEN** the scenario boot runs
- **THEN** the boot fails closed with a configuration error naming the v1
  tier limitation, before any route compiles

#### Scenario: non-loopback Public bind fails closed in the tier

- **GIVEN** a scenario project whose Camel.toml declares a non-loopback
  bind serving a `Public` route without `allow_public_exposure`
- **WHEN** the scenario boot starts the context
- **THEN** startup fails closed with the ADR-0061 acknowledgement error,
  identical to `camel run` behavior

#### Scenario: stream-cache threshold from config applies

- **GIVEN** a scenario project whose Camel.toml sets a non-default
  `[stream_caching]` threshold and whose route uses the stream_cache step
- **WHEN** the scenario boot loads routes through discovery
- **THEN** route compilation receives the configured threshold through the
  same wiring `camel run` uses

#### Scenario: templated route file materializes

- **GIVEN** a scenario project whose declared `routeFiles` contain a
  template and two templated routes
- **WHEN** the scenario boot loads the route source through discovery
- **THEN** both templated routes exist in the booted context, where the
  per-file parse previously yielded zero routes

#### Scenario: sql dynamic-query route fails closed at scenario startup

- **GIVEN** a scenario project whose route file contains a `sql:` endpoint
  with a dynamic query that ADR-0033 rejects at startup
- **WHEN** the scenario boot starts the context
- **THEN** startup fails with the ADR-0033 fail-closed check error,
  identical to `camel run` behavior

#### Scenario: hermetic env resolution

- **GIVEN** a route file containing `${env:X}` where `X` is defined only
  in the process environment and in no layer of the scenario environment
- **WHEN** the scenario boot loads routes
- **THEN** the boot fails with the unresolved-placeholder error and the
  process-environment value is never applied

#### Scenario: camel run unchanged

- **GIVEN** the repository's existing `camel run` test suite
- **WHEN** the shared-wiring delegation lands
- **THEN** every existing `camel run` test passes with assertions
  unchanged — mechanical call-site updates that swap local wiring
  helpers for the shared ones (same arrange, same assertions) do not
  count as modification
