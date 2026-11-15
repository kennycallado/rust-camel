## ADDED Requirements

### Requirement: Script operand binding (eval and fcall)

For `command=eval` and `command=fcall` endpoints ONLY, keys and args SHALL
bind from the exchange body (`{"keys": [...], "args": [...]}`) instead of the
`CamelRedis.Key`/`CamelRedis.Keys` headers. This is a scoped exception to the
body-only value binding's header-sourced addressing convention: server-side
scripts declare their own multi-key operand layout (`EVAL`/`FCALL` take a
keys vector plus an args vector), which the single-key header convention
cannot express. The exception is limited to script commands: every other
command keeps key addressing in `CamelRedis.Key`/`CamelRedis.Keys`, and batch
entries keep key addressing in their typed header entries. The body-only
value rules themselves are unchanged: values still never come from headers,
and script TEXT is operator config (URI/file), never body or header data.

#### Scenario: script keys and args bind from the body

- **GIVEN** a `command=eval` endpoint with body `{"keys": ["a", "b"],
  "args": ["x"]}`
- **WHEN** the producer builds the script invocation
- **THEN** it passes keys `a`, `b` and arg `x` positionally to `EVALSHA`

#### Scenario: non-script commands keep header-sourced keys

- **GIVEN** a `command=get` endpoint with `CamelRedis.Key=k1`
- **WHEN** the producer resolves the key
- **THEN** the key comes from `CamelRedis.Key`; the body carries only the
  value contract's data
