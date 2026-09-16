# jms Specification

## Purpose
TBD - created by archiving change jmsmutants. Update Purpose after archive.
## Requirements
### Requirement: JMS reconnect defaults remain explicit

The JMS configuration unit suite SHALL pin every field overridden by `jms_reconnect_default`.

#### Scenario: Reconnect policy contains documented defaults

- **GIVEN** the component reconnect default
- **WHEN** `jms_reconnect_default()` is evaluated
- **THEN** `max_attempts` is 0, `initial_delay` is 5 seconds, `multiplier` is 2.0, `max_delay` is 30 seconds, and `jitter_factor` is 0.0; `multiplier` and `max_delay` are also recorded as equivalent to their `NetworkRetryPolicy::default()` values

### Requirement: JMS endpoint URI validation rejects ambiguous input

The JMS configuration unit suite SHALL verify endpoint URI scheme, destination-name, shorthand, and boundary validation.

#### Scenario: Unsupported scheme is rejected

- **GIVEN** a URI without the `jms`, `activemq`, or `artemis` scheme
- **WHEN** `JmsEndpointConfig::from_uri` parses it
- **THEN** parsing fails with the expected scheme error

#### Scenario: Empty destination name is rejected

- **GIVEN** `jms:queue:`, `jms:topic:`, or bare `jms:` with no destination name
- **WHEN** `JmsEndpointConfig::from_uri` parses it
- **THEN** parsing fails with the destination-format error rather than the `jms:` shorthand ambiguity error

#### Scenario: JMS shorthand is rejected as ambiguous

- **GIVEN** a non-empty `jms:<name>` URI
- **WHEN** `JmsEndpointConfig::from_uri` parses it
- **THEN** parsing fails with guidance to use an explicit queue or topic prefix

#### Scenario: Validation boundary has the documented polarity

- **GIVEN** a priority query value of 9 or 10
- **WHEN** `JmsEndpointConfig::from_uri` parses them
- **THEN** priority 9 is accepted and priority 10 is rejected

### Requirement: JMS bridge cache default remains concrete

The JMS configuration unit suite SHALL verify that `default_bridge_cache_dir` delegates to the concrete bridge download cache default.

#### Scenario: Bridge cache default is not an empty path

- **GIVEN** the process environment used by the bridge cache helper
- **WHEN** `default_bridge_cache_dir()` is evaluated
- **THEN** it equals the concrete helper result and differs from `PathBuf::default()`

### Requirement: Broker URL redaction keeps benign query keys and masks credential-shaped values

The camel-jms broker URL redaction path (`redact_broker_url`, backed by
the canonical `camel_api::redact` query-allowlist variant) SHALL mask
userinfo in every authority window, SHALL redact any query pair whose
key — single-pass percent-decoded (`%HH`, hex case-insensitive) and
lowercased — contains a sensitive substring (`password`, `passwd`,
`secret`, `credential`, `token`, `username`, `user`) as
`key=<redacted>` (the rendered key keeps its original encoded bytes),
and SHALL keep all other pairs visible — ActiveMQ failover URIs encode
non-secret transport policy in query parameters, the sole diagnostic
value of the broker URL in Debug output (documented exception to
whole-query drop). The fragment SHALL never echo: everything from the
first `#` is dropped and replaced with the `#[redacted]` sentinel. The
result SHALL cap at 256 bytes on a UTF-8 character boundary with the
sentinel budget reserved before the cap.

A kept (benign-keyed) pair SHALL render as `<redacted>` when its
single-pass minimal decode — `%40` to `@`, `%3a` to `:`, `%2f` to `/`,
case-insensitive, applied to the whole pair — contains `@` AND (`:` OR
`//`). This closes percent-encoded credential smuggling under benign
keys (bd rc-r7v8s). A value whose decode carries `@` but neither `:` nor
`//` — an email address such as `contact=admin%40corp.example` — SHALL
stay visible; over-masking a `user@host:port`-shaped value is accepted
(ADR-0051: over-masking is safe, under-masking is not).

#### Scenario: percent-encoded credentials under a benign key are masked

- **GIVEN** `tcp://h:61616?redirect=http%3A%2F%2Fuser%3Asecret%40host`
  and its fully-lowercase variant
  `tcp://h:61616?redirect=http%3a%2f%2fuser%3asecret%40host`
- **WHEN** each passes through the broker URL redaction path
- **THEN** neither `secret` nor `user%3Asecret%40` renders — the
  `redirect` pair renders as `<redacted>`, and the broker host and port
  stay visible for failover diagnosis

#### Scenario: literal credential shapes under a benign key are masked

- **GIVEN** `tcp://h:61616?next=%2F%2Fuser:pass@host` (encoded slashes,
  literal `@`) and `tcp://h:61616?next=user:pass@host` (fully literal)
- **WHEN** each passes through the broker URL redaction path
- **THEN** neither `pass` nor `user:pass` renders — each `next` pair
  renders as `<redacted>`

#### Scenario: lone percent-encoded email value stays visible

- **GIVEN** `tcp://h:61616?contact=admin%40corp.example`
- **WHEN** it passes through the broker URL redaction path
- **THEN** the pair stays visible — `@` without `:` or `//` is not
  credential-shaped

#### Scenario: sensitive keys redact regardless of value shape

- **GIVEN** `tcp://host:61616?password=p&user=u&keepAlive=true` and
  `tcp://host:61616?pass%77ord=shortsecret` (percent-encoded key that
  decodes to `password`)
- **WHEN** each passes through the broker URL redaction path
- **THEN** the first output carries `password=<redacted>` and
  `user=<redacted>` while `keepAlive=true` stays visible; the second
  output carries `pass%77ord=<redacted>` — the decoded key matches, the
  secret value never renders

#### Scenario: credential-shaped kept value is masked

- **GIVEN** `tcp://h:61616?next=user%40host%3Aport` — a benign key
  whose value decodes to `user@host:port` (`@` plus `:`)
- **WHEN** it passes through the broker URL redaction path
- **THEN** the pair renders as `<redacted>` — over-masking is accepted
  per ADR-0051 (over-masking is safe, under-masking is not)

#### Scenario: fragment never echoes and the cap holds

- **GIVEN** a broker URL with a fragment and a kept query longer than
  256 bytes combined
- **WHEN** it passes through the broker URL redaction path
- **THEN** the output carries `#[redacted]` (never fragment bytes), is
  at most 256 bytes, cuts on a UTF-8 character boundary, and the
  sentinel renders complete

