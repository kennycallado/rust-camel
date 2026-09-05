# integration-tier Specification

## Purpose
TBD - created by archiving change integration-tier-contract. Update Purpose after archive.
## Requirements
### Requirement: Pure tier derivation

The system SHALL derive each test document's tier by a pure total function of
its content, with no declaration field, and SHALL never grow the lean
component set {direct, log, mock, seda, timer}. The closure SHALL traverse
every parsed route source (`routeFiles`, `routeFilesFromRoot`, inline
`routes`) recursively through nested steps. Any non-lean literal scheme, any
placeholder in scheme position, or any dynamic-dispatch step SHALL derive
FULL. Only an exact `skipTo` replacement subtracts an intercepted endpoint
from the closure; `divertCopyTo` delivers a copy while the real send
continues, so it subtracts nothing.

#### Scenario: lean document stays lean

- **GIVEN** a document with `inputs` to `direct:` and `expects` on `mock:`
- **WHEN** the tier function runs
- **THEN** the document derives LEAN and boots the lean boot, byte-identical
  registry

#### Scenario: skipTo subtracts from the closure

- **GIVEN** a route referencing `kafka:orders` and an intercept action
  `skipTo: mock:orders`
- **WHEN** the tier function runs
- **THEN** the document derives LEAN because the replaced endpoint is
  removed from the closure

#### Scenario: divertCopyTo does not subtract

- **GIVEN** a route referencing `kafka:orders` with an intercept action
  `divertCopyTo: mock:mirror`
- **WHEN** the tier function runs
- **THEN** the document derives FULL because the real send continues and
  `kafka` stays in the closure

#### Scenario: placeholder in scheme forces full

- **GIVEN** a route with `to: "${env:TARGET_SCHEME}:host"` where the
  placeholder sits before the first colon
- **WHEN** the tier function runs
- **THEN** the document derives FULL

#### Scenario: dynamic dispatch forces full

- **GIVEN** a route containing `recipient_list`, `routingSlip`,
  `dynamic_router`, or a `toD`-style step
- **WHEN** the tier function runs
- **THEN** the document derives FULL regardless of the rest of the closure

#### Scenario: scenario section forces full

- **GIVEN** a document with a `scenario:` section targeting only `direct:` and
  `mock:`
- **WHEN** the tier function runs
- **THEN** the document derives FULL

#### Scenario: inline and root-anchored routes count as sources

- **GIVEN** a lean-closure document whose routes come from inline `routes`
  or `routeFilesFromRoot`
- **WHEN** the tier function runs
- **THEN** those sources participate in the closure identically to
  `routeFiles`

### Requirement: Unified document with vocabulary ban

The system SHALL keep the reserved test-document suffix (`.test.yaml`, with
`.test.yml` as an alias of the same format) as the only test document
format — no integration-specific suffix — and
SHALL reject at load any document mixing `scenario:` with `inputs`,
`expects`, or `intercepts`, reporting `doc-validation` and exit 2.

#### Scenario: mixed vocabulary rejected

- **GIVEN** a document declaring both `scenario:` and `inputs:`
- **WHEN** the document loads
- **THEN** the run reports `doc-validation` and exits 2 before any boot

### Requirement: Symmetric tier filters

The system SHALL provide `--unit` and `--integration` as symmetric filters
over the derived tier, SHALL exclude nonmatching documents found through
directory expansion, SHALL fail with `tier-filter-collision` and exit 2 for a
nonmatching document named explicitly, and SHALL treat both flags together as
misuse with exit 2.

#### Scenario: expanded nonmatching document is excluded

- **GIVEN** a directory holding lean and full documents
- **WHEN** `camel test --unit` runs over the directory
- **THEN** only lean documents run and no failure is reported for the excluded
  full documents

#### Scenario: explicit nonmatching document collides

- **GIVEN** a document that derives FULL
- **WHEN** it is named explicitly under `camel test --unit`
- **THEN** the run reports `tier-filter-collision` and exits 2

#### Scenario: both flags are misuse

- **GIVEN** any document set
- **WHEN** `camel test --unit --integration` runs
- **THEN** the run exits 2 without booting anything

### Requirement: Ordered scenario actions

The system SHALL execute `scenario:` documents as ordered actions with
`send`, `receive` carrying a mandatory deadline, `sleep`, `validate`, and
scenario variables with extraction from received messages. A `send`
action SHALL accept an optional `method` field, normalized to
uppercase and validated as an RFC 7230 token at load. An absent method
SHALL infer `POST` for a body and `GET` for no body.

Scenario variables form one namespace. A harness endpoint reference
with `bindVar` SHALL fill that variable at boot, before the first
action. An `extract` SHALL overwrite on receive. Last writer wins. A
`bindVar` value SHALL be the bound authority, host and port.

Endpoint strings in `send` and `receive`, body string leaves, and
`send` header values SHALL resolve `${name}` placeholders against the
scenario variables. Substitution SHALL be raw, with no
percent-encoding. `$${` SHALL escape to a literal `${`. A variable
unset at resolution time SHALL fail `scenario-var-unresolved`, exit 1,
naming the variable. `${env:}` SHALL NOT resolve in scenario strings.

The initial registered-adapter lookup SHALL use the original declared
endpoint key, before interpolation. The wire target SHALL then interpolate variables
and, for a harness-provisioned reference, replace only the authority
with the selected partner's bound authority, preserving the
interpolated path and query. The declared URI SHALL never reach a
socket connect. After interpolation, an endpoint whose authority
equals a bound partner authority SHALL dispatch to that partner.

Adapter level, a send parks its roundtrip receiver under a generation
counter. A later send on the same endpoint replaces the entry under a
new generation. When the earlier send fails before receiving an HTTP
response, its cleanup SHALL NOT remove the later entry, and a
following receive SHALL consume the later send's roundtrip.

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

### Requirement: Partner-side normative proof

The system SHALL place normative integration assertions at the partner side
of the transport: a harness-owned listener for outbound routes and a harness
client against a real consumer for inbound routes. Mock expectations and
route-internal interception SHALL NOT produce a green integration result.

#### Scenario: outbound wire validation

- **GIVEN** a route whose HTTP producer targets a harness listener bound on
  `127.0.0.1:0`
- **WHEN** the route sends a request with corrupted headers
- **THEN** the scenario fails on the wire validation at the partner, not on
  any in-route assertion

#### Scenario: inbound readiness is honest

- **GIVEN** a full boot with an HTTP consumer on an explicit loopback port
- **WHEN** the harness client connects after boot completes
- **THEN** the connection succeeds without fixed sleeps, because boot waits
  for the bind through the operator readiness signal

#### Scenario: inbound response validated on the wire

- **GIVEN** a full boot whose consumer route answers with a status, headers,
  and body
- **WHEN** the harness client receives the response within the receive
  deadline
- **THEN** the scenario validates status, headers, and body at the wire and
  reports a verdict failure for any mismatch

### Requirement: Layered hermetic environment

The system SHALL resolve placeholders in scenario documents through a layered
environment source passed explicitly to the loaders: document `env` first,
allowlisted ambient variables second, defaults third, otherwise unresolved.
Ambient inheritance SHALL be off by default, `CAMEL_PROFILE` SHALL be pinned
per document, and the harness SHALL NOT mutate the process environment.

#### Scenario: document value wins

- **GIVEN** a document `env` fixing `HTTP_PORT=18080` and an ambient
  `HTTP_PORT=8080` not in the allowlist
- **WHEN** a route URI references `${env:HTTP_PORT}`
- **THEN** resolution yields 18080

#### Scenario: unset and unallowlisted variable fails named

- **GIVEN** a route referencing `${env:NOPE}` with no document value and no
  allowlist entry
- **WHEN** the document loads
- **THEN** the failure names the variable and the document exits before boot

### Requirement: Failure taxonomy

The system SHALL inherit exit codes 0, 1, 2 with an epistemic split: verdict
failures (`receive-timeout`, `validation-mismatch`, runtime
`scenario-var-unresolved`) exit 1; apparatus failures (`doc-validation`,
`tier-filter-collision`, `partner-bind-failure`, `partner-startup-failure`,
`action-transport-failure`, `infra-unavailable`, `full-boot-failure`,
`shutdown-failure`) exit 2. Every adapter operation SHALL carry a deadline.

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

### Requirement: Demand-gated activation and CI isolation

The system SHALL gate adapter activation behind the `http` Cargo feature for
v1, SHALL reserve `testcontainer` and `user-provided` partner provisioning
values as grammar that the v1 runner rejects as unsupported, and SHALL keep
the default test suite unchanged in runtime and composition. The
`integration-http` CI job SHALL run loopback scenarios on relevant pull
request paths; loopback scenarios SHALL carry no `#[ignore]` marker.

#### Scenario: http scenarios isolated behind their feature

- **GIVEN** a workspace built without the `http` adapter feature
- **WHEN** the integration-tier tests compile
- **THEN** no HTTP partner code is compiled in and no http scenario runs

#### Scenario: reserved provisioning value rejected

- **GIVEN** a scenario endpoint declaring the `testcontainer` or
  `user-provided` provisioning source
- **WHEN** the document loads in v1
- **THEN** the run reports the source as unsupported and exits 2

#### Scenario: default suite untouched

- **GIVEN** the default test suite before and after this change
- **WHEN** both run in CI
- **THEN** their runtime and executed set are identical, and only the
  opt-in `integration-http` job exercises scenario documents

### Requirement: Scripted partner declarations

The document SHALL accept a top-level `partners:` section. It SHALL
be a map from the exact declared endpoint string, the `:0` URI as
written in the endpoint reference, to a sequence of script entries.
Each entry SHALL be a map of optional `method` and `path` matchers
plus a `response` map of optional `status`, `headers`, and `body`,
with parity to the Rust `ScriptedResponse`. Partners SHALL bind before
route boot. When the section is present and no script matches, the
partner SHALL return status 500 with an empty body. An absent
`partners:` section SHALL keep the `permissive(200)` default. Unknown
keys SHALL fail `doc-validation` at load.

#### Scenario: scripted response serves the matching request

- **GIVEN** a `partners:` entry scripting method `POST` on `/orders`
  with status 201 and a body
- **WHEN** the scenario sends `method: POST` to that path
- **THEN** the response carries status 201 and the scripted body, and
  the scenario validates both

#### Scenario: unmatched request serves the unmatched status

- **GIVEN** a `partners:` entry scripting only method `POST` on
  `/orders`
- **WHEN** the scenario sends `method: DELETE` to the same path
- **THEN** the response carries status 500 with an empty body, and
  validation of any scripted body fails

#### Scenario: absent partners section keeps permissive behavior

- **GIVEN** a scenario with a harness endpoint reference and no
  `partners:` section
- **WHEN** any request reaches the partner
- **THEN** the response is status 200 with an empty body, exactly as
  before this section existed

#### Scenario: unknown partner entry field is a load error

- **GIVEN** a `partners:` entry with a field `responsez`
- **WHEN** the document loads
- **THEN** the run reports `doc-validation` naming the field and exits 2

#### Scenario: partners key with no matching declared ref is a load error

- **GIVEN** a declared harness reference `http://127.0.0.1:0/orders`
  and a `partners:` key `http://127.0.0.1:0/order` (a typo of the
  declared URI)
- **WHEN** the document loads
- **THEN** the run reports `doc-validation` naming the unmatched key
  and exits 2

