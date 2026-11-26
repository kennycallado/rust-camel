## ADDED Requirements

### Requirement: MCP catalog visibility follows caller authentication

The MCP server SHALL filter every `tools/list` and `resources/list` reply by
the caller's authentication. The filter SHALL use the same per-route kernel
authentication path the server uses to admit `tools/call` and
`resources/read`: the route's compiled security plan, credential extraction
from the normalized inbound headers, and `kernel_authenticate`. A list path
SHALL NOT read scopes or roles itself, and no second interpretation of the
route's security policy SHALL exist. This discovery rule is separate from the
route-dispatch authentication rule and SHALL NOT reinstate the bind-presence
gate superseded by ADR-0061.

The server SHALL determine readiness and access classification from the
catalog snapshot. An entry registered without a plan is public by
construction. Removal of a plan after the snapshot SHALL NOT change the
captured classification.

A ready entry with no registered security plan, or with a `Public` access
mode, SHALL be listed to every caller. A ready entry whose plan is not
`Public` SHALL be listed only when the request's credential authenticates
against that entry's own plan. When the plan carries an authorization policy,
the entry SHALL be listed only when that same policy grants the authenticated
principal **and** the policy declares itself independent of request content
(`SecurityPolicy::is_request_independent`); the default declaration SHALL be
`false`. A denial, an evaluation error, or a policy that is not
request-independent SHALL omit the entry (fail closed). `RolePolicy` and
`ScopePolicy` SHALL declare request independence; permission, WASM, `Ref`,
and custom policies SHALL remain hidden from discovery until their author
declares otherwise.

An unauthenticated caller on a policy-protected bind SHALL therefore see only
public entries, or an empty list when none is public. A bind with no
registered plans SHALL list every ready entry regardless of credentials.
Filtering SHALL NOT alter readiness gating, cache metadata, or the shape of a
listed entry. Omission from a list SHALL NOT weaken dispatch: `tools/call`
and `resources/read` keep their existing kernel denial and authorization
behavior, and on a request those checks deny, the route SHALL NOT run.

#### Scenario: unauthenticated list shows only public tools

- **GIVEN** a bind whose catalog contains a tool whose route plan is `Public`
  and a tool whose route plan requires authentication, with no credential on
  the request
- **WHEN** a host issues `tools/list`
- **THEN** the result contains the public tool and does not contain the
  protected tool

#### Scenario: scoped tool is hidden from an authenticated caller without the scope

- **GIVEN** a tool whose route plan authorizes a required scope, and a caller
  whose valid credential lacks that scope
- **WHEN** the host issues `tools/list`
- **THEN** the result does not contain the scoped tool

#### Scenario: authorized caller sees the scoped tool

- **GIVEN** a tool whose route plan authorizes a required scope, and a caller
  whose valid credential carries that scope
- **WHEN** the host issues `tools/list`
- **THEN** the result contains the scoped tool

#### Scenario: content-dependent policy is hidden even from an authorized caller

- **GIVEN** a tool whose route plan carries a policy that grants on an empty
  Exchange but denies real dispatch content, and a caller whose credential is
  valid
- **WHEN** the host issues `tools/list`
- **THEN** the result does not contain the tool, and a later `tools/call`
  reaches the unchanged dispatch denial path

#### Scenario: policy-less bind lists everything

- **GIVEN** a bind with no registered route security plans
- **WHEN** a caller with no credential issues `tools/list`
- **THEN** every ready tool is listed, and the same result holds when a
  credential is sent

#### Scenario: list omission does not weaken call denial

- **GIVEN** a protected tool that `tools/list` omitted for an unauthenticated
  caller
- **WHEN** the same caller issues `tools/call` for that tool with schema-valid
  arguments
- **THEN** the call returns the existing kernel denial and the tool's route
  receives no invocation

#### Scenario: resources/list applies the same filter

- **GIVEN** a bind whose catalog contains a public resource and a resource
  whose route plan requires authentication, with no credential on the request
- **WHEN** a host issues `resources/list`
- **THEN** the result contains the public resource and does not contain the
  protected resource

#### Scenario: authorized caller sees the scoped resource

- **GIVEN** a resource whose route plan authorizes a required scope, and a
  caller whose valid credential carries that scope
- **WHEN** the host issues `resources/list`
- **THEN** the result contains the resource; without the scope it does not

#### Scenario: policy-less bind lists every resource

- **GIVEN** a bind with no registered route security plans
- **WHEN** a caller with no credential issues `resources/list`
- **THEN** every ready resource is listed

#### Scenario: resource list omission does not weaken read denial

- **GIVEN** a protected resource that `resources/list` omitted for an
  unauthenticated caller
- **WHEN** the same caller issues `resources/read` for it
- **THEN** the read returns the existing kernel denial and the resource route
  receives no read
