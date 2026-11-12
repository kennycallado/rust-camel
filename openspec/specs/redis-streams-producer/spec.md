# redis-streams-producer Specification

## Purpose
TBD - created by archiving change redisstreams. Update Purpose after archive.
## Requirements
### Requirement: XADD appends the exchange body as exactly one body field

The producer SHALL accept `command=XADD` and append one entry to the stream
named by `CamelRedis.Key` (falling back to the URI `key=`). The entry SHALL
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

#### Scenario: header key overrides the URI key fallback

- **GIVEN** the URI `redis://127.0.0.1:6379?command=XADD&key=s1` and an
  exchange with header `CamelRedis.Key=s2` and body text `hello`
- **WHEN** the producer executes
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
the number of entries in the stream named by `CamelRedis.Key` (falling back
to the URI `key=`). `XLEN` SHALL classify as idempotent (a read).

#### Scenario: count is delivered as the body

- **GIVEN** a fake RESP server that answers `XLEN s1` with `7`
- **WHEN** an `XADD`-prepared exchange with header `CamelRedis.Key=s1` is
  executed by a `command=XLEN` producer
- **THEN** the recorded frame is `XLEN s1` and the exchange body is `7`

### Requirement: XADD is non-idempotent and never retried by the component

`Xadd` SHALL NOT be listed by `is_idempotent_command`. The component SHALL
NOT retry a failed XADD. An ambiguous transport failure (the reply may or
may not have been applied) SHALL return `Err` with an unknown-outcome
processor error. `CamelRedis.StreamId` SHALL NOT be used as a dedup key;
duplicates under route redelivery are accepted (at-least-once) and the
Idempotent Consumer with `RedisIdempotentRepository` is the mitigation.

#### Scenario: classification and no retry

- **GIVEN** a fake RESP server that fails the first XADD with a transient
  transport error
- **WHEN** `is_idempotent_command` classifies `RedisCommand::Xadd`, and the
  producer executes the exchange
- **THEN** the classification is `false`, exactly one XADD frame was sent
  (no retry), and the producer returned `Err`

