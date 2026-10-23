# health-probe-bodies Specification

## Purpose
TBD - created by archiving change 381-health-probe-detail. Update Purpose after archive.
## Requirements
### Requirement: Probe bodies carry per-service detail when not Healthy

The system SHALL NOT return an empty `services` array from `/healthz`,
`/readyz`, or `/startupz` when the aggregate status is `Degraded` or
`Unhealthy` and the source is detail-capable (it overrides the report
methods); the body SHALL name each offending service and its message using
the `ServiceHealth` shape (`name`, `status`, `message`). Sources relying on
the defaulted report methods preserve the status-only shape — an empty
`services` array is permitted for any aggregate status. Service completeness
is scoped to the evaluated probe domain: when a probe short-circuits before
consulting downstream checks, the body names exactly the offenders found in
the evaluated domain.

#### Scenario: Degraded /readyz names the degraded service

- **GIVEN** a `HealthSource` whose readiness evaluation is `Degraded` because
  a named service is degraded with a message
- **WHEN** a client GETs `/readyz`
- **THEN** the response is HTTP 200 with `status` `Degraded` and a
  `services` array containing an entry with that service's `name`,
  `status`, and `message`

#### Scenario: Unhealthy /healthz names the failed service

- **GIVEN** a `HealthSource` whose liveness evaluation is `Unhealthy` because
  a named lifecycle service is failed
- **WHEN** a client GETs `/healthz`
- **THEN** the response is HTTP 503 with `status` `Unhealthy` and a
  `services` array containing that service's `name`, `status`, and `message`

#### Scenario: Liveness does not inherit readiness-only failures

- **GIVEN** a source whose readiness is `Unhealthy` from a registry check but
  whose lifecycle services are all non-failed
- **WHEN** a client GETs `/healthz`
- **THEN** the liveness aggregate status and body are unaffected by the
  readiness-only failure

#### Scenario: Lifecycle short-circuit preserves precedence without registry consult

- **GIVEN** a source with a failed lifecycle entry and a readiness evaluation
  that short-circuits before consulting the registry, where the registry
  records whether it is consulted
- **WHEN** a client GETs `/readyz`
- **THEN** the response is HTTP 503 naming the failed lifecycle entry with a
  non-empty `message`, the registry is not consulted, and a stopped lifecycle
  entry does not change the readiness aggregate status

### Requirement: Probe timeout synthesizes a probe-timeout service entry

The system SHALL fail closed on handler timeout at `/healthz`, `/readyz`, and
`/startupz` with HTTP 503, aggregate status `Unhealthy`, and a
`services` array containing one `ServiceHealth` entry named `probe-timeout`
with status `Failed` and a message naming the probe that exceeded the handler
timeout, exactly as `/health` already does. The synthetic `probe-timeout`
entry SHALL appear only on handler timeout; ordinary `Unhealthy` responses
carry the source's own service entries and never a synthetic one.

#### Scenario: Hanging source produces probe-timeout entry

- **GIVEN** a `HealthSource` whose readiness report never completes within
  the handler timeout
- **WHEN** a client GETs `/readyz`
- **THEN** the response is HTTP 503 with `status` `Unhealthy` and a
  `services` array whose single entry is named `probe-timeout` with status
  `Failed`

#### Scenario: Timeout entry synthesized on all three probes

- **GIVEN** a `HealthSource` whose report methods never complete within the
  handler timeout
- **WHEN** a client GETs `/healthz`, then `/readyz`, then `/startupz`
- **THEN** each response is HTTP 503 with `status` `Unhealthy` and a
  `services` array whose single entry is named `probe-timeout`, each message
  naming its own probe

#### Scenario: Ordinary Unhealthy response has no synthetic entry

- **GIVEN** a detail-capable source whose readiness report completes with
  `Unhealthy` and a named failed service
- **WHEN** a client GETs `/readyz`
- **THEN** the response contains that named service entry and no
  `probe-timeout` entry

### Requirement: Healthy probe body shape is explicit and stable

The system SHALL serve, on aggregate status `Healthy`, exactly the report the
source produced for that probe domain: detail-capable sources list all
relevant services even when Healthy (a detail-capable source returns an empty
`services` array on Healthy only when its evaluation produced no entries),
and sources using the trait defaults return an empty `services` array for
every aggregate status. This shape SHALL be documented on the health docs
page and tested for both source kinds.

#### Scenario: Detail-capable source Healthy body lists services

- **GIVEN** a detail-capable source that is fully Healthy and produces
  service entries
- **WHEN** a client GETs `/readyz`
- **THEN** the response is HTTP 200 with `status` `Healthy` and the
  `services` array exactly as the source produced it

#### Scenario: Status-only source Healthy body has empty services

- **GIVEN** a `HealthSource` that overrides only the bare status methods
- **WHEN** a client GETs `/healthz`
- **THEN** the response is HTTP 200 with `status` `Healthy` and an empty
  `services` array

#### Scenario: Status-only source non-Healthy body preserves status-only shape

- **GIVEN** a `HealthSource` that overrides only the bare status methods and
  returns `Unhealthy` from `readiness()`
- **WHEN** a client GETs `/readyz`
- **THEN** the response is HTTP 503 with `status` `Unhealthy` and an empty
  `services` array, per the defaulted report methods

### Requirement: Probe wire contract is unchanged

The system SHALL keep the probe JSON field names (`status`, `services`,
`timestamp`, and per entry `name`, `status`, `message`), the status-code
mapping (200 for `Healthy` and `Degraded`, 503 for `Unhealthy`), and the
`/health` endpoint behavior unchanged; the additive `HealthSource` report
methods SHALL be defaulted so existing implementors compile unchanged.

#### Scenario: Existing implementor compiles via defaults

- **GIVEN** a `HealthSource` implementor that defines only `liveness()` and
  `readiness()`
- **WHEN** the new report methods are called on it
- **THEN** they return `HealthReport` with the bare probe status and an
  empty `services` array, and no implementor change is required

#### Scenario: Exact serialized keys are stable

- **GIVEN** a probe response from a detail-capable source with a degraded
  service carrying a message
- **WHEN** the raw JSON body is inspected
- **THEN** the top-level keys are exactly `status`, `services`, `timestamp`
  and each entry's keys are exactly `name`, `status`, `message` (including
  `"message": null` when absent), and the HTTP status follows the 200/503
  mapping exactly as before this change

