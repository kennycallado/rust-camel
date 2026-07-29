## ADDED Requirements

### Requirement: Non-exec, non-declared startup skips exec validation

The camel-cli `run` command SHALL NOT prevent startup through exec validation
when no discovered route references an `exec:` endpoint AND the operator has
not declared a `[components.exec]` section. In that case the ExecBundle SHALL
NOT be constructed or validated, regardless of whether the `exec` cargo
feature is enabled.

#### Scenario: timer-to-log route starts with exec feature enabled and no exec config

- **GIVEN** the CLI is built with default features (exec enabled) and a
  Camel.toml that contains no `[components.exec]` section
- **WHEN** `camel run` runs a single `timer:tick -> log` route
- **THEN** the CamelContext starts, the route processes exchanges, and on a
  stop signal the process exits with code 0; no "no profiles configured"
  error is raised

#### Scenario: ExecBundle is not constructed when exec is unused and undeclared

- **GIVEN** discovered routes contain no `exec:` endpoint and Camel.toml has
  no `[components.exec]` section
- **WHEN** `camel run` starts
- **THEN** `ExecBundle::from_toml` is never invoked and no fail-closed
  validation runs

#### Scenario: explicit empty exec declaration is still validated

- **GIVEN** Camel.toml contains an explicit `[components.exec]` section with
  zero profiles, and no discovered route references `exec:`
- **WHEN** `camel run` starts
- **THEN** the ExecBundle is registered and startup aborts fail-closed with
  the "no profiles configured" error, because the operator declared exec intent

### Requirement: Exec-using routes remain fail-closed without profiles

The camel-cli `run` command SHALL register and validate the ExecBundle when at
least one discovered route references an `exec:` endpoint, preserving the
ADR-0033 fail-closed property: a config with zero exec profiles SHALL abort
startup.

#### Scenario: exec route with no profiles aborts startup

- **GIVEN** a discovered route containing an `exec:echo` step and a Camel.toml
  with no exec profiles
- **WHEN** `camel run` starts
- **THEN** startup aborts with a fail-closed error stating no profiles are
  configured

#### Scenario: exec route with a matching profile starts

- **GIVEN** a discovered route containing an `exec:echo` step and a Camel.toml
  defining an `echo` profile
- **WHEN** `camel run` starts
- **THEN** the ExecBundle validates and the route starts

#### Scenario: one of several routes using exec triggers registration

- **GIVEN** two discovered routes, where only the second references
  `exec:echo`, and Camel.toml defines an `echo` profile
- **WHEN** `camel run` starts
- **THEN** the ExecBundle is registered and both routes start

### Requirement: Exec usage is detected across all statically declared URIs

The scheme-presence scan SHALL inspect the route from-uri and every
**statically declared** URI reachable through structural step variants
(`To`, `WireTap`, `Enrich`, `PollEnrich`, and the recursive bodies of
`Filter`/`DeclarativeFilter`, `Split`/`DeclarativeSplit`/
`DeclarativeStreamSplit`, `Multicast`, `Throttle`, `LoadBalance`,
`Loop`/`DeclarativeLoop`, `IdempotentConsumer`, `Choice`/`DeclarativeChoice`,
and `DeclarativeDoTry` try/catch/finally). Dynamic-URI steps (routing slip,
recipient list, dynamic router) SHALL be treated as not statically resolvable
and SHALL NOT, on their own, trigger exec registration.

#### Scenario: exec as from-uri triggers registration

- **GIVEN** a route whose from-uri is `exec:echo` (consumer-style) and no
  exec profiles are configured
- **WHEN** `camel run` starts
- **THEN** startup aborts fail-closed, because an `exec:` endpoint is declared

#### Scenario: exec referenced inside a choice branch triggers registration

- **GIVEN** a route whose from-uri is `timer:tick` and whose steps include a
  `choice` with a `when` branch containing `to: exec:echo`
- **WHEN** `camel run` starts with no exec profiles configured
- **THEN** startup aborts fail-closed, because a statically declared `exec:`
  endpoint is reachable

#### Scenario: exec referenced via wiretap triggers registration

- **GIVEN** a route containing a `wiretap: exec:audit` step and no exec
  profiles configured
- **WHEN** `camel run` starts
- **THEN** startup aborts fail-closed, because the `exec:` URI is statically
  declared

#### Scenario: exec referenced via enrich triggers registration

- **GIVEN** a route containing an `enrich: exec:enricher` step and no exec
  profiles configured
- **WHEN** `camel run` starts
- **THEN** startup aborts fail-closed, because the `exec:` URI is statically
  declared

#### Scenario: exec referenced via pollEnrich triggers registration

- **GIVEN** a route containing a `pollEnrich: exec:poller` step and no exec
  profiles configured
- **WHEN** `camel run` starts
- **THEN** startup aborts fail-closed, because the `exec:` URI is statically
  declared

#### Scenario: dynamic-uri-only route does not trigger registration

- **GIVEN** a route whose only exec-like reference is produced by a
  dynamic-URI step (recipient list, dynamic router, or routing slip) from
  exchange data, with no statically declared `exec:` URI and no
  `[components.exec]` section
- **WHEN** `camel run` starts
- **THEN** the ExecBundle is not registered and the route starts (any runtime
  attempt to resolve an `exec:` endpoint fails endpoint resolution, not
  startup validation)

