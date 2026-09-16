## ADDED Requirements

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
