# route-templates Specification

## Purpose
TBD - created by archiving change template-compile-parity. Update Purpose after archive.
## Requirements
### Requirement: Security context parity for templated routes

The system SHALL compile routes materialized from templates with the same
`SecurityCompileContext` as direct routes in the same discovery run, and
SHALL fail closed when no authenticator is available for a templated route's
`security_policy`.

#### Scenario: secured templated route compiles with real context

- **GIVEN** a template whose route declares `security_policy` with roles and a `templated_routes` instantiation, and a discovery run with a `SecurityCompileContext` containing the authenticator from Camel.toml `[security.native]`
- **WHEN** discovery materializes and compiles the template
- **THEN** the route compiles and exposes a security authenticator

#### Scenario: secured templated route fails closed without context

- **GIVEN** the same template instantiated without any configured authenticator
- **WHEN** discovery materializes and compiles the template
- **THEN** compilation fails with the authenticator-required error

#### Scenario: mixed direct and templated secured routes in one file

- **GIVEN** a file containing a direct secured route and a templated secured route, and a discovery run with a real security context
- **WHEN** discovery completes
- **THEN** both routes are materialized and compiled

### Requirement: Stream-cache threshold parity for templated routes

The system SHALL compile materialized routes with the configured
`stream_cache_threshold`, identical to direct routes.

#### Scenario: configured threshold reaches materialized route steps

- **GIVEN** a template whose route contains a stream-cache step and a discovery run with a non-default threshold
- **WHEN** the template is materialized and compiled
- **THEN** the stream-cache step observes the configured threshold, not the default

### Requirement: Uniform context threading for threshold-less discovery

The system SHALL route threshold-less discovery through the same
security-aware compile path as threshold-carrying discovery, so direct and
templated routes share one default-context semantics.

#### Scenario: threshold-less direct route uses security-aware path

- **GIVEN** a discovery run without an explicit stream-cache threshold
- **WHEN** routes are parsed and compiled
- **THEN** they compile through the security-aware entrypoint with the default threshold and the provided or empty security context

### Requirement: Complete fail-closed template diagnostics

The system SHALL keep startup fail-closed when any templated spec fails
materialization, and SHALL report every materialization failure in the same
run with its error class, template id, and file location preserved.
Parse-level failures of template definitions or templated specs during
template collection (Pass 1) remain first-abort.

#### Scenario: two failing specs both reported

- **GIVEN** a file with two templated specs, both invalid for different reasons
- **WHEN** discovery runs
- **THEN** the run fails and the error output names both specs with their distinct error causes

#### Scenario: distinct error classes for security and body failures

- **GIVEN** one templated spec that fails materialization because its `security_policy` requires an authenticator and none is configured, and another templated spec in the same run whose materialized body is malformed
- **WHEN** discovery reports both failures
- **THEN** the two failures carry distinct error classes (security-required vs invalid-body) instead of both being flattened to invalid-body

### Requirement: Typed template parameters

The system SHALL support declared parameter types (`string`, `number`,
`boolean`, default `string`) where a placeholder occupying a whole scalar
node substitutes with the declared type, and SHALL reject non-coercible
parameter values at resolution time.

#### Scenario: numeric parameter populates numeric field

- **GIVEN** a template parameter declared `type: number` with value `5000`, used as the whole value of a numeric DSL field
- **WHEN** the template is materialized
- **THEN** the field deserializes as the number `5000`

#### Scenario: embedded placeholder remains string-typed

- **GIVEN** a placeholder embedded in a longer string inside the template body
- **WHEN** the template is materialized with a string parameter
- **THEN** the result is textual interpolation

#### Scenario: non-coercible value rejected loudly

- **GIVEN** a parameter declared `type: number` whose provided value is `abc`
- **WHEN** parameters are resolved
- **THEN** resolution fails with an error naming the parameter and the declared type

#### Scenario: typed parameter used whole-node and embedded in the same template

- **GIVEN** a `type: number` parameter used as the entire string value `"{{p}}"` of one field and embedded as `"x{{p}}"` in another string field of the same template
- **WHEN** the template is materialized
- **THEN** the whole-node field deserializes as a JSON number and the embedded field yields a string containing the parameter's textual form

### Requirement: Per-instance route identity

The system SHALL reject a `route_id` override on a multi-route template, and
SHALL allow distinct overrides per instantiation of a single-route template.

#### Scenario: multi-route template with override fails

- **GIVEN** a template producing two routes and an instantiation with `route_id` set
- **WHEN** the template is materialized
- **THEN** materialization fails with a configuration error directing the author to per-route ids

#### Scenario: single-route template instantiated N times

- **GIVEN** a single-route template instantiated three times with distinct `route_id` overrides
- **WHEN** discovery completes
- **THEN** three routes exist with the three distinct ids

### Requirement: Parameter-sensitive source hash

The system SHALL compute the source hash of a materialized route from the
template body, the resolved parameter map, and the effective route id (the
id after any `route_id` override is applied), so reload detection observes
parameter-value and identity changes per instance.

#### Scenario: parameter change triggers reload

- **GIVEN** a running templated route whose instantiation parameter value changes on disk
- **WHEN** the reload diff runs
- **THEN** the instance is detected as changed and reloaded

#### Scenario: unrelated instance untouched

- **GIVEN** two instantiations of the same template where only one parameter value changes
- **WHEN** the reload diff runs
- **THEN** only the affected instance reloads

#### Scenario: override-only instances hash distinctly

- **GIVEN** a single-route template instantiated three times with byte-identical parameters and distinct `route_id` overrides
- **WHEN** the source hash is computed for each materialized route
- **THEN** all three hashes are distinct

