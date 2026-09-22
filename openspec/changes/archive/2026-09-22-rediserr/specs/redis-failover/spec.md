## ADDED Requirements

### Requirement: Structural transient classification of redis errors

The camel-redis component SHALL classify retry-transience of a
`CamelError` structurally — by `CamelError` variant, typed marker sources,
and `redis::RedisError::kind()` downcast from the preserved source chain —
not by lowercase substring matching on `err.to_string()`. The verdict table
SHALL be behaviorally identical to the legacy substring table for every
realizable redis-transport input. Conversion boundaries on the
classification paths (component `commands/*.rs`, `topology.rs` resolve and
connect, `executor.rs` connect, `queue.rs`/`pubsub.rs` reconnect wraps,
`retry.rs` budget exhaustion) SHALL preserve the structured error in the
`CamelError` source chain (`ProcessorErrorWithSource`) while keeping the
operator-visible message text unchanged.

Classification precedence SHALL be: (1) `Config`/`ConfigValidation` → not
transient (ADR-0012 boundary); (2) `CamelError::Io` → transient; (3) a
typed marker source (`TransientRetryBudgetExhausted`, `TransportTimeout`,
`TransientByProse`) → transient; (4) the first `redis::RedisError` in the
source chain whose kind is `Server(ReadOnly)`, `ClusterConnectionNotFound`,
or `Io` carrying an `io::Error` source of kind `ConnectionRefused`,
`ConnectionReset`, `ConnectionAborted`, `BrokenPipe`, or `TimedOut` →
transient; (5) a preserved `redis::RedisError` matched by none of those
enumerated shapes → an explicit documented legacy-substring fallback on
THAT RedisError's own Display; (6) anything else (including plain
`ProcessorError` text with no redis error or marker in its chain) → not
transient. Verdict identity SHALL be proven per conversion site by a
static-prose audit: a wrap site whose static prose contains a legacy
classifier word SHALL produce an always-transient structure (marker), and
a site whose prose contains no classifier word SHALL have its legacy
verdict reproduced by rules 4–5 on the preserved error.

#### Scenario: refused connection classified transient through preserved structure

- **GIVEN** a `redis::RedisError` of kind `Io` wrapping an `io::Error` of
  kind `ConnectionRefused`, converted at a command boundary via the shared
  helper
- **WHEN** `is_transient_redis_error` classifies the resulting
  `CamelError`
- **THEN** the verdict is transient, and the operator-visible message is
  byte-identical to the legacy `ProcessorError` wrap

#### Scenario: read-only role error classified transient by server code

- **GIVEN** a `redis::RedisError` server error `READONLY You can't write
  against a read only replica.`
- **WHEN** classified after boundary conversion
- **THEN** the verdict is transient (failover to replica-in-promotion),
  with no substring test on the message

#### Scenario: business error not transient despite preserved structure

- **GIVEN** a `redis::RedisError` server error `WRONGTYPE Operation
  against a key holding the wrong kind of value`
- **WHEN** classified after boundary conversion
- **THEN** the verdict is not transient

#### Scenario: budget exhaustion classified transient by marker

- **GIVEN** `retry.rs` exhausted the `NetworkRetryPolicy` budget and built
  its terminal error with the `TransientRetryBudgetExhausted` marker source
- **WHEN** the consumer's Err-branch classifies it (ADR-0012 metric
  routing: `e:redis:message-transient-budget`)
- **THEN** the verdict is transient, and the message text still names the
  stage and attempt budget

#### Scenario: repo transport error classified transient by variant

- **GIVEN** the repository executor's `CamelError::Io` (get_conn remap,
  `to_camel_error`, or the local response backstop)
- **WHEN** classified
- **THEN** the verdict is transient (as under the legacy table, where the
  `IO error:` Display prefix always matched)

#### Scenario: plain processor text no longer sniffs transient

- **GIVEN** a plain `CamelError::ProcessorError("connection refused")`
  with no redis error or marker in its source chain
- **WHEN** classified
- **THEN** the verdict is not transient — the stringly false-positive
  class is removed

#### Scenario: documented fallback preserves unenumerated spellings

- **GIVEN** a preserved `redis::RedisError` matched by no enumerated kind
  shape — e.g. a server-controlled message embedding a classifier word
  (`ERR connection lost while …`), a TLS inner error whose text contains
  one, or a redis-rs static detail like `SSL Handshake error`
- **WHEN** classified
- **THEN** the verdict matches the legacy substring table exactly (the
  rule-5 fallback is the only place substring matching remains, it tests
  only the RedisError's own Display, and it is documented at the match
  site)

#### Scenario: prose-transient site stays always-transient via marker

- **GIVEN** a conversion site whose static wrap prose contains a legacy
  classifier word (e.g. `failed to build Redis connection info: …`),
  wrapping an inner error whose own kind is fatal
- **WHEN** the reconnect loop classifies the resulting `CamelError`
- **THEN** the verdict is transient, identical to the legacy behavior
  where the prose word matched — the site carries a `TransientByProse`
  marker, and any fatal-inner-kind concern at such sites is filed as a
  follow-up, not silently flipped

#### Scenario: config errors with transient-looking text stay fatal

- **GIVEN** a `CamelError::Config` whose message embeds a classifier word
  (e.g. a CA path containing `readonly`)
- **WHEN** classified
- **THEN** the verdict is not transient (rc-ezi0f early return preserved)
