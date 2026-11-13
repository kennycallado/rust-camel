## MODIFIED Requirements

### Requirement: XADD appends the exchange body as exactly one body field

The producer SHALL accept `command=XADD` and append one entry to the stream
named by the URI `key=` (falling back to the exchange header `CamelRedis.Key`
when the URI declares no key). The effective key SHALL be published on
`CamelRedis.Key` — the URI key when the URI declares one — overwriting any
stale upstream value before dispatch. The entry SHALL
carry exactly one field named `body`; its value SHALL be
`exchange.body.try_into_text()` — text as-is, compact JSON for structured
bodies, UTF-8 bytes decoded as text. `Body::Empty`, `Body::Stream`, and
non-UTF-8 byte bodies SHALL fail with a named processor error before any
command is sent. The entry ID SHALL always be `*` (server-assigned). The
exchange body SHALL be kept unchanged (never replaced by the XADD reply);
the producer SHALL set `CamelRedis.StreamId` to the returned entry ID and
`CamelRedis.Stream` to the stream key, overwriting any upstream values.
Round-trip contract: text `S` written by XADD SHALL be read by the
`XREADGROUP` consumer as `Body::Text(S)` byte for byte.

#### Scenario: wire frame and postconditions

- **GIVEN** a producer URI `redis://127.0.0.1:6379?command=XADD&key=s1`
  and an exchange with body text `hello`
- **WHEN** the producer executes against a fake RESP server that answers
  XADD with `5-0`
- **THEN** the recorded frame is `XADD s1 * body hello`, the exchange body
  is still `hello`, and the headers are `CamelRedis.StreamId=5-0` and
  `CamelRedis.Stream=s1`

<!-- openspec-scenario-ops
renamed: header key overrides the URI key fallback -> URI key overrides the exchange header key
-->

#### Scenario: URI key overrides the exchange header key

- **GIVEN** the URI `redis://127.0.0.1:6379?command=XADD&key=s1` and an
  exchange with header `CamelRedis.Key=s2` and body text `hello`
- **WHEN** the producer executes
- **THEN** the recorded frame is `XADD s1 * body hello`,
  `CamelRedis.Stream=s1`, and the header `CamelRedis.Key` carries the
  effective key `s1`

#### Scenario: header key used when the URI declares no key

- **GIVEN** the URI `redis://127.0.0.1:6379?command=XADD` (no `key=`) and
  an exchange with header `CamelRedis.Key=s2` and body text `hello`
- **WHEN** the producer executes against a fake RESP server that answers
  XADD with `5-0`
- **THEN** the recorded frame is `XADD s2 * body hello` and
  `CamelRedis.Stream=s2`

#### Scenario: prohibited body forms fail before sending

- **GIVEN** an `XADD` producer against a fake RESP server and three
  exchanges: one with `Body::Empty`, one with `Body::Stream` carrying
  valid UTF-8 content, and one with `Body::Bytes` carrying non-UTF-8 bytes
- **WHEN** the producer executes each exchange
- **THEN** each exchange fails with a processor error naming the body
  requirement (`try_into_text` rejected the empty body, the stream body,
  and the non-UTF-8 bytes respectively), and no XADD frame was ever sent
  for any of the three

#### Scenario: text round-trips byte for byte through the consumer

- **GIVEN** text `S = "María & café ☕"` written with `command=XADD`
- **WHEN** the existing `XREADGROUP` consumer reads the entry
- **THEN** the delivered Exchange body is `Body::Text(S)` byte for byte

### Requirement: XLEN returns the entry count as the body

The producer SHALL accept `command=XLEN` and replace the exchange body with
the number of entries in the stream named by the URI `key=` (falling back
to the exchange header `CamelRedis.Key` when the URI declares no key). The
effective key SHALL be published on `CamelRedis.Key`. `XLEN` SHALL classify
as idempotent (a read).

#### Scenario: count is delivered as the body

- **GIVEN** a fake RESP server that answers `XLEN s1` with `7`
- **WHEN** an `XADD`-prepared exchange with header `CamelRedis.Key=s1` is
  executed by a `command=XLEN` producer
- **THEN** the recorded frame is `XLEN s1` and the exchange body is `7`

## ADDED Requirements

### Requirement: Rename, RENAMENX and MOVE are non-idempotent and never retried

`Rename`, `Renamenx`, and `Move` SHALL NOT be listed by
`is_idempotent_command`. The component SHALL NOT retry a failed `RENAME`,
`RENAMENX`, or `MOVE`: a retry after an unconfirmed success turns success
into a visible failure (`RENAME` answers `ERR no such key`; `RENAMENX` and
`MOVE` answer `0`). An ambiguous transport failure SHALL return `Err` after
exactly one attempt.

#### Scenario: classification and no retry

- **GIVEN** a fake executor that fails the first command with a transient
  transport error, and `is_idempotent_command` classifying
  `RedisCommand::Rename`, `RedisCommand::Renamenx`, and
  `RedisCommand::Move`
- **WHEN** the producer executes an exchange for each of the three
  commands
- **THEN** each classification is `false`, the executor was called exactly
  once per exchange (no retry, no reconnect), and the producer returned
  `Err`
