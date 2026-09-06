## ADDED Requirements

### Requirement: Env-lookup-injected route discovery

Route discovery SHALL offer an entry that resolves `${env:NAME}`
placeholders in route files through a caller-injected lookup closure
instead of the process environment, while preserving the full discovery
contract of the process-environment entries: glob pattern handling, the
reserved test-suffix gate, JSON explicit-pattern gating, file size caps,
two-pass template materialization, stream-cache threshold threading, and
security compile-context threading. The existing process-environment
entries SHALL keep their behavior unchanged.

#### Scenario: injected lookup resolves placeholders

- **GIVEN** a route file containing `from: direct:${env:TIER_ONLY}` and an
  injected lookup that maps `TIER_ONLY` to `start`
- **WHEN** discovery runs through the env-injected entry
- **THEN** the route compiles with the `direct:start` endpoint

#### Scenario: process environment is not consulted

- **GIVEN** a route file containing `${env:PROC_ONLY}`, a process
  environment that defines `PROC_ONLY`, and an injected lookup that
  returns `None` for `PROC_ONLY`
- **WHEN** discovery runs through the env-injected entry
- **THEN** discovery fails with the environment error naming `PROC_ONLY`
  and the file path, without reading the process environment

#### Scenario: templates materialize through the injected entry

- **GIVEN** a route file declaring one template and two templated routes,
  and an injected lookup resolving every placeholder the file uses
- **WHEN** discovery runs through the env-injected entry with a threshold
  and a security compile context
- **THEN** both templated routes materialize and compile with the given
  threshold and security context, identical to the process-environment
  entry over the same file after equivalent interpolation
