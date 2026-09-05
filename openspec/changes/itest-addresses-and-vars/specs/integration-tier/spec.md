## MODIFIED Requirements

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

## ADDED Requirements

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
