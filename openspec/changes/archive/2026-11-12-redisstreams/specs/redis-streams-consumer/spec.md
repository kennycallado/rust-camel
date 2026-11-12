## MODIFIED Requirements

### Requirement: XREADGROUP activates Streams consumer mode without fallback

The Redis consumer SHALL accept `command=XREADGROUP` on `redis://` and
`redis-sentinel://` consumer endpoints and resolve it to a Streams
consumer mode. The mode SHALL require a non-empty `group`, a non-empty
`consumer`, and a `key`; any of them missing or empty SHALL fail at
consumer construction with an invalid-URI error (fail closed, REDIS-003).
`XREADGROUP` mode SHALL NOT fall back to BLPOP/BRPOP or any other
consume path. Optional parameters: `count` (positive integer, absent =
no COUNT argument) and `block` (milliseconds, default 10000, `0` = block
indefinitely); invalid values SHALL fail at construction. Additional
optional parameters: `createGroup` (boolean, default `false`) and
`groupStart` (`latest` or `earliest`). The pair SHALL be validated at URI
parse time: `createGroup=true` without an explicit `groupStart` SHALL fail
at parse, and `groupStart` present without `createGroup=true` SHALL fail
at parse. There is no default start position.

#### Scenario: valid XREADGROUP URI resolves to Streams mode

- **GIVEN** the URI `redis://127.0.0.1:6379?command=XREADGROUP&key=s1&group=g&consumer=c`
- **WHEN** `RedisConsumer::new` builds the consumer
- **THEN** the consumer is in Streams mode with group `g`, consumer `c`,
  no count, and block 10000 ms

#### Scenario: missing group fails closed

- **GIVEN** the URI `redis://127.0.0.1:6379?command=XREADGROUP&key=s1&consumer=c`
- **WHEN** `RedisConsumer::new` builds the consumer
- **THEN** construction fails with an invalid-URI error naming `group`,
  and no connection is attempted

#### Scenario: empty consumer name fails closed

- **GIVEN** the URI `redis://127.0.0.1:6379?command=XREADGROUP&key=s1&group=g&consumer=`
- **WHEN** `RedisConsumer::new` builds the consumer
- **THEN** construction fails with an invalid-URI error naming `consumer`

#### Scenario: invalid block value fails closed

- **GIVEN** the URI `redis://127.0.0.1:6379?command=XREADGROUP&key=s1&group=g&consumer=c&block=abc`
- **WHEN** `RedisConsumer::new` builds the consumer
- **THEN** construction fails with an invalid-URI error naming `block`

#### Scenario: no fallback to BLPOP

- **GIVEN** a Streams consumer whose session hits a transient transport
  error on every connect attempt within budget
- **WHEN** the retry budget is exhausted
- **THEN** the session returns `Err` for supervision and no BLPOP/BRPOP
  command was ever issued (verified by the recorded command log)

#### Scenario: groupStart without createGroup fails at parse time

- **GIVEN** the URI `redis://127.0.0.1:6379?command=XREADGROUP&key=s1&group=g&consumer=c&groupStart=latest`
- **WHEN** the URI is parsed
- **THEN** parsing fails with an error naming `groupStart`, and no
  connection is attempted

#### Scenario: createGroup without groupStart fails at parse time

- **GIVEN** the URI `redis://127.0.0.1:6379?command=XREADGROUP&key=s1&group=g&consumer=c&createGroup=true`
- **WHEN** the URI is parsed
- **THEN** parsing fails with an error naming `groupStart` — there is no
  default start position — and no connection is attempted

## ADDED Requirements

### Requirement: Group creation is explicit, operator-owned, and readied before the first read

With `createGroup=true`, the Streams session SHALL send
`XGROUP CREATE <key> <group> <start> MKSTREAM` exactly once per session,
on the read connection, before the first `XREADGROUP`; `groupStart=latest`
maps to `$` and `groupStart=earliest` maps to `0`. Readiness
(`mark_ready`) SHALL be signaled only after the group-creation command
succeeds or is answered with `BUSYGROUP`. A `BUSYGROUP` reply — matched by
`RedisError::code()`, never by message text — SHALL be treated as success
with an `info!` log: the existing group wins and `XGROUP SETID` is never
sent. With `createGroup=false` (the default), the session SHALL NOT send
`XGROUP CREATE`; a missing group keeps failing closed with `NOGROUP`.
`NOPERM` and `WRONGTYPE` SHALL classify as non-transient and surface to
supervision.

#### Scenario: opt-in creation sends MKSTREAM before the first read

- **GIVEN** the URI `...?command=XREADGROUP&key=s1&group=g&consumer=c&createGroup=true&groupStart=earliest`
  and a fake RESP server
- **WHEN** the session starts
- **THEN** the first frame after connect is `XGROUP CREATE s1 g 0
  MKSTREAM`, the XREADGROUP frames follow it, and readiness fires only
  after the create reply

#### Scenario: BUSYGROUP is success with an info log

- **GIVEN** a fake RESP server that answers the XGROUP CREATE frame with
  the server error `BUSYGROUP Consumer Group name already exists`
- **WHEN** the session starts with `createGroup=true`
- **THEN** the session treats the reply as success (code-matched), logs at
  `info!`, signals readiness, and no `XGROUP SETID` frame is ever sent

#### Scenario: NOPERM goes to supervision as non-transient

- **GIVEN** a fake RESP server that answers the XGROUP CREATE frame with
  `NOPERM` for the session user
- **WHEN** the session starts with `createGroup=true`
- **THEN** the classification is non-transient and the session returns
  `Err` for supervision instead of retrying group creation

#### Scenario: default keeps the operator-owned NOGROUP failure

- **GIVEN** the URI `...?command=XREADGROUP&key=s1&group=g&consumer=c`
  (no `createGroup`) against a server where group `g` does not exist
- **WHEN** the session starts
- **THEN** no XGROUP CREATE frame is sent and the first read fails with
  `NOGROUP`, classified non-transient for supervision
