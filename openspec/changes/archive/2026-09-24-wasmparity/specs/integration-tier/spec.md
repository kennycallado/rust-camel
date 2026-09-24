## MODIFIED Requirements

### Requirement: Scenario boot shares the camel run composition root

The scenario tier SHALL boot through the same composition root as `camel run`: sealed config load and root-anchored route resolution follow the boot root, which is the nearest ancestor directory (including the document's own) containing a `Camel.toml`. Relative `routeFiles` entries SHALL resolve against the scenario document's own directory. A document with no `Camel.toml` ancestor SHALL fail named with exit 2 before boot. The resolved boot root SHALL be a usable, non-empty directory: an empty resolved root — a document named as a bare relative filename from its project directory — SHALL normalize to the working directory (`.`), the same empty-parent rule `camel run` applies to its project root, so every root-derived consumer (sealed config load, root-anchored route resolution, and the wasm bundle base dir) receives semantics identical to `camel run` booting the same project.

#### Scenario: boot root is the nearest Camel.toml ancestor

- **GIVEN** a scenario document at `tests/integration/layers/map/static.test.yaml` and a `Camel.toml` at `tests/integration/`
- **WHEN** the tier boots the document
- **THEN** the sealed config loads from `tests/integration/Camel.toml` and the boot succeeds

#### Scenario: no Camel.toml ancestor fails named

- **GIVEN** a scenario document with no `Camel.toml` in any ancestor directory
- **WHEN** the tier loads it
- **THEN** the run fails with exit 2 naming the missing project root

#### Scenario: relative routeFiles stay document-anchored

- **GIVEN** a nested scenario document declaring `routeFiles: [routes.yaml]` with `routes.yaml` colocated next to the document
- **WHEN** the tier boots from the ancestor root
- **THEN** `routeFiles` resolve against the document directory and the boot succeeds

#### Scenario: routeFilesFromRoot follows the resolved root

- **GIVEN** a nested scenario document declaring `routeFilesFromRoot: [routes/x.yaml]` present under the ancestor root's route space
- **WHEN** the tier boots from the ancestor root
- **THEN** the root-anchored paths resolve against the resolved boot root, not the document directory

#### Scenario: wasm route boots with the boot root as base dir

- **GIVEN** a scenario project whose route file declares a `wasm:` step and whose guest module sits under the project root
- **WHEN** the tier boots the document named as a bare relative filename from the project root directory
- **THEN** the wasm base dir resolves to the normalized boot root (never the empty path), the guest loads, and the boot succeeds — identical to `camel run` on the same project

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
