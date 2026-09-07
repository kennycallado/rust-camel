## MODIFIED Requirements

### Requirement: Scenario boot shares the camel run composition root

The scenario tier SHALL boot through the same composition root as `camel run`: sealed config load and root-anchored route resolution follow the boot root, which is the nearest ancestor directory (including the document's own) containing a `Camel.toml`. Relative `routeFiles` entries SHALL resolve against the scenario document's own directory. A document with no `Camel.toml` ancestor SHALL fail named with exit 2 before boot.

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

### Requirement: Ordered scenario actions

Scenario actions SHALL execute in document order. A receive SHALL carry a mandatory deadline. A document MAY declare a top-level `sendDeadline` bounding every send action; when absent, the thirty-second default applies. An unparseable `sendDeadline` SHALL fail at load time with exit 2.

#### Scenario: document send deadline bounds every send

- **GIVEN** a document declaring `sendDeadline: 500ms` and a send action whose connection hangs
- **WHEN** the action runs
- **THEN** the send fails apparatus-class at the document deadline, naming the bound

#### Scenario: absent send deadline keeps the thirty-second default

- **GIVEN** a document without `sendDeadline` and a send that completes
- **WHEN** the actions run
- **THEN** send behavior is unchanged from the fixed default

#### Scenario: invalid send deadline is a load error

- **GIVEN** a document declaring `sendDeadline: soon`
- **WHEN** the document loads
- **THEN** the load fails with exit 2 naming `sendDeadline`
#### Scenario: send then receive within deadline

- **GIVEN** a full-tier scenario that sends a body and receives on a partner
  endpoint with a deadline
- **WHEN** the partner returns the body inside the deadline
- **THEN** the scenario validates the body and passes

#### Scenario: missing deadline is a load error

- **GIVEN** a `receive` action without a deadline
- **WHEN** the document loads
- **THEN** the run reports `doc-validation` and exits 2

#### Scenario: variable extraction flows forward

- **GIVEN** a `receive` action that extracts a header into a scenario variable
- **WHEN** a later `validate` references that variable
- **THEN** validation sees the extracted value

#### Scenario: explicit method overrides body inference

- **GIVEN** a `send` action with `method: PUT` and no body, targeting a
  harness partner endpoint whose scripted response matches only
  method `PUT` and serves a known body
- **WHEN** the scenario runs
- **THEN** the request carries method `PUT`, the scripted response matches,
  and the received body validates. Under the legacy inference the
  request would be `GET`, the scripted response would not match, and
  the unmatched status with an empty body would fail validation

#### Scenario: absent method keeps legacy inference

- **GIVEN** a `send` action without a `method` field
- **WHEN** the action carries a body, and again without a body
- **THEN** the requests carry `POST` and `GET` respectively, exactly as
  before the field existed

#### Scenario: invalid method token is a load error

- **GIVEN** a `send` action with `method: "P UT"` (a space is not a
  token character)
- **WHEN** the document loads
- **THEN** the run reports `doc-validation` naming the action index and
  exits 2, with the same behavior whether or not the `http` feature is
  compiled

#### Scenario: lowercase method normalizes to uppercase

- **GIVEN** a `send` action with `method: delete`
- **WHEN** the document loads
- **THEN** the resolved method on the typed action is `DELETE`

#### Scenario: partner-direct send reaches the bound address

- **GIVEN** a `send` whose endpoint reference declares `provisioning:
  harness` on a `:0` URI with `bindVar: PARTNER`, and a `partners:`
  entry keyed by that declared URI that scripts method `PUT` on path
  `/orders` with status 200 and a known body
- **WHEN** the scenario sends `method: PUT` and receives from the same
  reference
- **THEN** the request reaches the partner at its bound address, the
  scripted response validates, and no dial ever targets the declared
  `:0` URI

#### Scenario: extracted variable interpolates into a later send

- **GIVEN** a receive that extracts a response field into `orderId` and a
  later `send` with endpoint
  `http://${PARTNER}/orders/${orderId}` and `method: GET`
- **WHEN** the scenario runs
- **THEN** the second request path carries the extracted value and the
  partner matchers see it

#### Scenario: unset variable at send time fails naming the variable

- **GIVEN** a `send` whose endpoint string contains `${missing}`, and no
  boot-time `bindVar` or earlier extraction sets `missing`
- **WHEN** the send resolves
- **THEN** the run reports `scenario-var-unresolved` naming `missing` and
  exits 1

#### Scenario: bindVar address is interpolable as a string

- **GIVEN** a harness endpoint reference with `bindVar: PARTNER` and a
  later `send` with endpoint `http://${PARTNER}/orders`
- **WHEN** the later send resolves
- **THEN** the endpoint carries the partner bound authority

#### Scenario: dollar dollar escapes a literal

- **GIVEN** a `send` with a body string leaf `$${not_a_var}`
- **WHEN** the send resolves
- **THEN** the wire body carries the literal `${not_a_var}` and no
  variable lookup happens

#### Scenario: a failed send does not remove a later send's lane entry

- **GIVEN** two sends on the same endpoint, the first parking its
  roundtrip receiver, the second replacing the lane entry
- **WHEN** the first send fails before receiving an HTTP response
- **THEN** its cleanup leaves the second entry intact and a following
  receive consumes the second send's roundtrip

### Requirement: Failure taxonomy

Scenario runs SHALL keep the epistemic exit split: 0 = pass, 1 = verdict-class (system under test failed), 2 = apparatus-class (harness failure) and load/misuse errors. Harness-side queue overflow conditions SHALL surface as apparatus-class failures carrying their own name, never as verdict-class receive timeouts.

#### Scenario: arrival lane overflow is an apparatus failure

- **GIVEN** a partner path whose arrival lane dropped arrivals after filling
- **WHEN** a subsequent receive on that path times out
- **THEN** the run fails with exit 2 as an apparatus arrival-lane-overflow naming the dropped count, not as a verdict receive-timeout

#### Scenario: client lane FIFO overflow is an apparatus failure

- **GIVEN** more same-key in-flight sends than the client lane FIFO bound
- **WHEN** the overflowing send launches
- **THEN** the send fails apparatus-class naming the lane key and the bound
#### Scenario: receive timeout is a verdict failure

- **GIVEN** a `receive` with a deadline whose partner never sends
- **WHEN** the deadline elapses
- **THEN** the run reports `receive-timeout` and exits 1

#### Scenario: infra absence fails named, never hangs

- **GIVEN** a scenario requiring a broker that is absent
- **WHEN** the runner starts
- **THEN** the run reports `infra-unavailable` naming the requirement and
  exits 2 within its startup deadline

#### Scenario: shutdown failure does not mask the verdict

- **GIVEN** a scenario that passed its assertions
- **WHEN** teardown times out during shutdown
- **THEN** the verdict stays recorded and the run reports `shutdown-failure`
  with exit 2

### Requirement: Wire-fidelity lane key and mismatch diagnostics

The strict wire-bytes lane key SHALL keep its fidelity contract. Multiple in-flight sends under one lane key SHALL park their responses in a bounded FIFO ordered by wire arrival; receives under that key SHALL consume the oldest parked response first.

#### Scenario: same-key sends park in arrival order

- **GIVEN** three sends to one lane key with no intervening receives
- **WHEN** three receives on that key follow
- **THEN** each receive resolves the oldest parked response first, in wire order

#### Scenario: Query-bearing receive matches after wire fidelity

- **Given** a scenario declaring an HTTP partner `from: http://0.0.0.0:PORT/api?flag=a&x=1` and a route sending to a matching target with the same authored query bytes
- **When** the partner `receive` action runs
- **Then** the request is recorded into the matching lane and the receive asserts against it (arrival key equals the declared endpoint's wire `path_and_query`)

#### Scenario: Receive-timeout lists arrived wire paths

- **Given** a partner `receive` that matches no arrival lane
- **When** the receive times out
- **Then** the error message lists every wire path (with query) that arrived in the lane group, so byte divergence is diagnosed without external packet capture

#### Scenario: Count mismatch lists recorded paths

- **Given** a partner `receive` whose count assertion fails (expected N, recorded M ≠ N)
- **When** the mismatch is reported
- **Then** the failure detail prints the recorded request paths, not only the counts

#### Scenario: Empty harness path is an apparatus error

- **Given** a harness HTTP target declaration whose path is empty or absent
- **When** the harness parses the target
- **Then** parsing fails with an apparatus-class error naming the declaration — no silent `/` fallback lane

#### Scenario: Diagnostics redact sensitive query values

- **Given** a receive-timeout or count-mismatch diagnostic whose recorded wire path contains a query value classified sensitive under ADR-0051
- **When** the harness renders the diagnostic message
- **Then** the sensitive value is redacted in the printed path — wire-path visibility never becomes credential disclosure

## ADDED Requirements

### Requirement: Load-time document validation

The scenario document loader SHALL reject statically-detectable document defects at load time with exit 2, before partner binding or boot, following the reserved-provisioning gate pattern.

#### Scenario: inline routes rejected at load

- **GIVEN** a scenario document whose route source is inline `routes:`
- **WHEN** the document loads
- **THEN** the load fails with exit 2 directing the author to `routeFiles`, before any partner binds

#### Scenario: harness provisioning without bound authority rejected at load

- **GIVEN** a `provisioning: harness` entry with a `direct:` or `fake:` ref that also declares a `bindVar`
- **WHEN** the document loads
- **THEN** the load fails with exit 2 naming the entry and the missing bound authority

#### Scenario: expectReply on a partner send is a load error

- **GIVEN** a send to an `http` partner ref or a `fake:` ref declaring `expectReply`
- **WHEN** the document loads
- **THEN** the load fails with exit 2 naming the action index and the `expectReply` field

### Requirement: Direct reply assertion

The scenario tier SHALL assert the synchronous reply of a `direct:` send through an `expectReply` field parsed into the shared matcher algebra (`camel-matchers::Expectation`, the same verbs as `validate`). `expectReply` on any non-`direct:` target SHALL fail at load time (the fake adapter records sends and produces no synchronous reply). An `expectReply` mismatch SHALL fail verdict-class naming the expectation and the actual reply body.

#### Scenario: expectReply matches the direct reply body

- **GIVEN** a request/reply route behind `direct:reply` and a send declaring `expectReply: {contains: "ack"}`
- **WHEN** the send completes
- **THEN** the reply body satisfies the expectation and the action passes

#### Scenario: expectReply mismatch is a verdict failure

- **GIVEN** a send declaring `expectReply` whose reply body violates the expectation
- **WHEN** the send completes
- **THEN** the run fails verdict-class naming the expectation and the actual reply body

### Requirement: Inbound listener provisioning

The scenario harness SHALL provision inbound listeners on an ephemeral port and expose the bound address to route URIs through variable interpolation, reusing staged listener consumption. Documents pinning literal ports SHALL keep working unchanged.

#### Scenario: inbound listener binds port zero

- **GIVEN** a document declaring an inbound listener with a `bindVar`
- **WHEN** the harness provisions it
- **THEN** the listener binds `127.0.0.1:0` and the variable carries the bound `http://host:port` address

#### Scenario: route consumer serves on the staged listener

- **GIVEN** a route URI interpolating the inbound `bindVar`
- **WHEN** the document boots and a request targets the bound address
- **THEN** the route consumer serves from the staged listener without a second bind

#### Scenario: fixed-port inbound documents stay valid

- **GIVEN** an inbound document pinning a literal port in the route URI (the pre-change shape)
- **WHEN** it loads and boots
- **THEN** the behavior is unchanged
