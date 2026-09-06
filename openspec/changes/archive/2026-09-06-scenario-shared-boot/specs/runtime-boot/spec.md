## ADDED Requirements

### Requirement: Shared security and startup wiring

camel-bundles SHALL own the composition-root wiring that `camel run`
performs before route loading: (a) building the security compile context
from `CamelConfig` (authenticator resolution, provider registration,
keycloak/UMA evaluator wiring, and — when the wasm feature is enabled —
wasm security-policy and permission registries) behind a `security`
feature gate that adds the camel-auth, camel-component-keycloak, and
camel-dsl dependencies, and (b), always available without any feature
gate, installing `[binds]` public-exposure acknowledgements into the
context-level gate and — under the existing mcp/wasm component features —
the MCP registry gate and the wasm source-bind gate (ADR-0061 Rule 4,
fail-closed), and (c), always available without any feature gate,
registering ADR-0033 fail-closed SQL startup checks derived from the
discovered route definitions. `camel run` SHALL delegate to these
helpers, SHALL enable the security feature in its default feature set so
default-build behavior is unchanged, and its externally observable
behavior SHALL NOT change.

#### Scenario: camel run delegates without behavior change

- **GIVEN** a project with security, binds, and sql configuration
- **WHEN** `camel run` boots through the camel-bundles helpers
- **THEN** the booted context matches the pre-change behavior on every
  existing run test (security policies resolve, bind acks gate identically,
  SQL startup checks reject identically)

#### Scenario: both callers boot a security_policy route

- **GIVEN** one CamelConfig with native security whose route declares a
  `security_policy`
- **WHEN** `camel run` boots it and the integration harness boots the same
  project through the shared helpers
- **THEN** both boots succeed in starting the route, and in both boots a
  request with valid native credentials passes the policy while a request
  without credentials is refused

#### Scenario: both callers refuse an unacknowledged public bind

- **GIVEN** one CamelConfig declaring a non-loopback bind serving a
  `Public` route without `allow_public_exposure`
- **WHEN** `camel run` boots it and the integration harness boots the same
  project through the shared installer
- **THEN** both boots fail closed at context start with the ADR-0061
  acknowledgement error

#### Scenario: security feature absent fails closed

- **GIVEN** a `CamelConfig` declaring `[security]` sections and a build
  where the camel-bundles security feature is disabled
- **WHEN** a caller runs the ungated `ensure_security_supported` check
  before booting
- **THEN** the check returns a configuration error naming the required
  feature, before any route compiles

#### Scenario: ungated helpers build without the security feature

- **GIVEN** a build of camel-bundles with the security feature disabled
- **WHEN** a caller uses the bind-ack installer or the SQL startup-check
  installer
- **THEN** both compile and install their wiring (the helpers use only
  camel-bundles' existing hard dependencies)

#### Scenario: feature forwarding with default enablement

- **GIVEN** camel-cli built with its default feature set
- **WHEN** it depends on camel-bundles
- **THEN** the camel-bundles security feature is enabled (the forwarding
  feature is part of the CLI defaults) and the shared helpers are
  available to the CLI boot
