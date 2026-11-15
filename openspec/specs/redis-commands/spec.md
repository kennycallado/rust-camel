# redis-commands Specification

## Purpose
TBD - created by archiving change rediscmds. Update Purpose after archive.
## Requirements
### Requirement: SET options

The system SHALL accept `SET` option headers (`CamelRedis.SetCondition`,
`CamelRedis.KeepTtl`, `CamelRedis.ReturnOldValue`, `CamelRedis.ExpireUnit`)
and translate them into the Redis `SET key value [NX|XX] [EX s|PX ms|
EXAT s|PXAT ms|KEEPTTL] [GET]` argument list. The value stays the exchange
body. Conflicting options, invalid enum values, malformed booleans, and
out-of-domain numbers SHALL fail closed before send with a named error that
names the header. `SET` is classified not idempotent because its option
forms lose replay data (design.md D3).

#### Scenario: SET with NX condition

- **GIVEN** `command=SET`, `CamelRedis.Key=k`, `CamelRedis.SetCondition=NX`, and a text body
- **WHEN** the producer sends the command
- **THEN** the wire frame is `SET k <body> NX`

#### Scenario: SET with XX condition

- **GIVEN** `CamelRedis.SetCondition=XX`
- **WHEN** the producer sends the command
- **THEN** the wire frame is `SET k <body> XX`

#### Scenario: SET with EX in default unit

- **GIVEN** `CamelRedis.Timeout=15` and no `CamelRedis.ExpireUnit`
- **WHEN** the producer sends the command
- **THEN** the wire frame is `SET k <body> EX 15`

#### Scenario: SET with PX via the unit discriminator

- **GIVEN** `CamelRedis.Timeout=500` and `CamelRedis.ExpireUnit=MILLISECONDS`
- **WHEN** the producer sends the command
- **THEN** the wire frame is `SET k <body> PX 500`

#### Scenario: SET with EXAT

- **GIVEN** `CamelRedis.Timestamp=1731000000` (unit default `SECONDS`)
- **WHEN** the producer sends the command
- **THEN** the wire frame is `SET k <body> EXAT 1731000000`

#### Scenario: SET with PXAT

- **GIVEN** `CamelRedis.Timestamp=1731000000000` and `CamelRedis.ExpireUnit=MILLISECONDS`
- **WHEN** the producer sends the command
- **THEN** the wire frame is `SET k <body> PXAT 1731000000000`

#### Scenario: SET with KEEPTTL

- **GIVEN** `CamelRedis.KeepTtl=true` and no expiry headers
- **WHEN** the producer sends the command
- **THEN** the wire frame is `SET k <body> KEEPTTL`

#### Scenario: SET with GET asserts the old value on the wire

- **GIVEN** `CamelRedis.ReturnOldValue=true`
- **WHEN** the producer sends the command
- **THEN** the wire frame is `SET k <body> GET`; a bulk-string reply body carries the old value bytes binary-safe (same reply path as `GET`)

#### Scenario: KEEPTTL with an expiry is a named error

- **GIVEN** `CamelRedis.KeepTtl=true` and `CamelRedis.Timeout=15`
- **WHEN** the producer resolves the options
- **THEN** it fails before send; the error names `CamelRedis.KeepTtl` and the expiry header

#### Scenario: Timeout and Timestamp together are a named error

- **GIVEN** `CamelRedis.Timeout=15` and `CamelRedis.Timestamp=1731000000`
- **WHEN** the producer resolves the options
- **THEN** it fails before send; the error names both headers

#### Scenario: invalid SetCondition is a named error

- **GIVEN** `CamelRedis.SetCondition=MAYBE`
- **WHEN** the producer resolves the options
- **THEN** it fails before send; the error names `CamelRedis.SetCondition` and the valid values `NX`/`XX`

#### Scenario: invalid ExpireUnit is a named error

- **GIVEN** `CamelRedis.Timeout=5` and `CamelRedis.ExpireUnit=HOURS`
- **WHEN** the producer resolves the options
- **THEN** it fails before send; the error names `CamelRedis.ExpireUnit` and the valid values

#### Scenario: ExpireUnit without an expiry is a named error

- **GIVEN** `CamelRedis.ExpireUnit=MILLISECONDS` and no `CamelRedis.Timeout`/`CamelRedis.Timestamp`
- **WHEN** the producer resolves the options
- **THEN** it fails before send; the error names the unit header and the missing expiry

#### Scenario: malformed KeepTtl is a named error

- **GIVEN** `CamelRedis.KeepTtl="yes-please"` (present but not a boolean)
- **WHEN** the producer resolves the options
- **THEN** it fails before send; the error names `CamelRedis.KeepTtl` and the received value

#### Scenario: SET expiry timeout must be positive

- **GIVEN** `CamelRedis.Timeout=0`
- **WHEN** the producer resolves the options
- **THEN** it fails before send; the error names `CamelRedis.Timeout` and the positive requirement (Redis rejects `EX 0`)

### Requirement: GETDEL and GETEX

The system SHALL support `GETDEL` (delete and return the value) and `GETEX`
(return the value with an optional expiry change: `Timeout`/`Timestamp` under
`CamelRedis.ExpireUnit`, or `CamelRedis.Persist`). Value replies SHALL be
binary-safe via the `GET` reply path. Conflicting `GETEX` options, invalid
units, and malformed booleans SHALL fail closed with a named error. Both
commands are classified not idempotent: `GETDEL` consumes the key so a retry
reports nil; `GETEX` re-arms expiry so the first reply is not reproducible.

#### Scenario: GETDEL wire form and reply

- **GIVEN** `command=GETDEL`, `CamelRedis.Key=k`
- **WHEN** the producer sends the command
- **THEN** the wire frame is exactly `GETDEL k` and the reply body carries the value bytes binary-safe (nil encodes as the `GET` miss body)

#### Scenario: plain GETEX sends no option arguments

- **GIVEN** no expiry headers on `command=GETEX`
- **WHEN** the producer sends the command
- **THEN** the wire frame is exactly `GETEX k` and the reply is binary-safe

#### Scenario: GETEX re-arms the expiry in seconds

- **GIVEN** `CamelRedis.Timeout=100` (unit default `SECONDS`)
- **WHEN** the producer sends `GETEX`
- **THEN** the wire frame is `GETEX k EX 100` and the reply is binary-safe

#### Scenario: GETEX with PX

- **GIVEN** `CamelRedis.Timeout=500` and `CamelRedis.ExpireUnit=MILLISECONDS`
- **WHEN** the producer sends `GETEX`
- **THEN** the wire frame is `GETEX k PX 500`

#### Scenario: GETEX with EXAT

- **GIVEN** `CamelRedis.Timestamp=1731000000`
- **WHEN** the producer sends `GETEX`
- **THEN** the wire frame is `GETEX k EXAT 1731000000`

#### Scenario: GETEX with PXAT

- **GIVEN** `CamelRedis.Timestamp=1731000000000` and `CamelRedis.ExpireUnit=MILLISECONDS`
- **WHEN** the producer sends `GETEX`
- **THEN** the wire frame is `GETEX k PXAT 1731000000000`

#### Scenario: GETEX with PERSIST

- **GIVEN** `CamelRedis.Persist=true`
- **WHEN** the producer sends `GETEX`
- **THEN** the wire frame is `GETEX k PERSIST` and the reply is binary-safe

#### Scenario: GETEX PERSIST with an expiry is a named error

- **GIVEN** `CamelRedis.Persist=true` and `CamelRedis.Timeout=10`
- **WHEN** the producer resolves the options
- **THEN** it fails before send; the error names `CamelRedis.Persist` and the expiry header

#### Scenario: GETEX Timeout and Timestamp together are a named error

- **GIVEN** both `CamelRedis.Timeout` and `CamelRedis.Timestamp`
- **WHEN** the producer resolves the options
- **THEN** it fails before send; the error names both headers

#### Scenario: malformed GETEX Persist is a named error

- **GIVEN** `CamelRedis.Persist="always"`
- **WHEN** the producer resolves the options
- **THEN** it fails before send; the error names `CamelRedis.Persist` and the received value

#### Scenario: GETEX ExpireUnit without an expiry is a named error

- **GIVEN** `CamelRedis.ExpireUnit=MILLISECONDS` with no `CamelRedis.Timeout`/`CamelRedis.Timestamp` (with or without `CamelRedis.Persist=true`)
- **WHEN** the producer resolves the options
- **THEN** it fails before send; the error names the unit header and the missing expiry (same rule as `SET`)

### Requirement: SETRANGE and GETRANGE

The system SHALL support `SETRANGE` (write the body at `CamelRedis.Offset`,
reply is the new length) and `GETRANGE` (read `CamelRedis.Start`..
`CamelRedis.End`, binary-safe reply). `CamelRedis.Offset` MUST be a
non-negative integer: offset 0 is legal, a negative offset is a named error.

#### Scenario: SETRANGE wire form

- **GIVEN** `CamelRedis.Key=k`, `CamelRedis.Offset=6`, and body `world`
- **WHEN** the producer sends the command
- **THEN** the wire frame is `SETRANGE k 6 world` and the integer reply is the exchange body as JSON

#### Scenario: SETRANGE accepts offset zero

- **GIVEN** `CamelRedis.Offset=0`
- **WHEN** the producer sends the command
- **THEN** the wire frame is `SETRANGE k 0 <body>`

#### Scenario: SETRANGE with a negative offset is a named error

- **GIVEN** `CamelRedis.Offset=-1`
- **WHEN** the producer resolves the arguments
- **THEN** it fails before send; the error names `CamelRedis.Offset` and the non-negative requirement

#### Scenario: SETRANGE without Offset is a named error

- **GIVEN** no `CamelRedis.Offset`
- **WHEN** the producer resolves the arguments
- **THEN** it fails before send; the error names `CamelRedis.Offset`

#### Scenario: GETRANGE wire form and binary-safe reply

- **GIVEN** `CamelRedis.Start=0`, `CamelRedis.End=3` (existing range headers)
- **WHEN** the producer sends `GETRANGE`
- **THEN** the wire frame is `GETRANGE k 0 3` and the reply body carries the slice bytes binary-safe

### Requirement: Multi-key source validation

Every multi-key command (`LMPOP`, `BLMPOP`, `ZMPOP`, `BZMPOP`, `SINTERCARD`,
`ZDIFF`, `ZINTER`, `ZUNION`, `TOUCH`) SHALL read `CamelRedis.Keys` through a
strict resolver: the header MUST be a JSON array of strings, non-empty, with
every entry valid. A non-array header, an empty array, or a malformed entry
SHALL fail closed before send with a named error naming the header and, for a
bad entry, its index. Malformed entries are never silently dropped.

#### Scenario: empty Keys array is a named error

- **GIVEN** `CamelRedis.Keys=[]` on any multi-key command
- **WHEN** the producer resolves the arguments
- **THEN** it fails before send; the error names `CamelRedis.Keys`

#### Scenario: non-array Keys is a named error

- **GIVEN** `CamelRedis.Keys="a"` (a bare string) on `SINTERCARD`
- **WHEN** the producer resolves the arguments
- **THEN** it fails before send; the error names `CamelRedis.Keys`

#### Scenario: malformed Keys entry is a named error with the index

- **GIVEN** `CamelRedis.Keys=["a", 42]` on `LMPOP`
- **WHEN** the producer resolves the arguments
- **THEN** it fails before send; the error names `CamelRedis.Keys`, the offending index, and the value; nothing is dropped

### Requirement: List commands

The system SHALL support `LMPOP`, `BLMPOP`, `LMOVE`, `BLMOVE`, and `LPOS`.
`LMPOP`/`BLMPOP` pop by list end (`CamelRedis.Where`: `LEFT`/`RIGHT`,
required). The `LPOS` search element is a payload and rides the exchange body
through `body_value_bytes` (body-only contract); `Rank`/`Count` stay headers.
Element replies follow the `MGET` binary rule: all-UTF-8 replies map to JSON
arrays, any binary element switches the whole reply to the RESP-encoded
`Bytes` body.

#### Scenario: LMPOP argument mapping

- **GIVEN** `CamelRedis.Keys=["a","b"]`, `CamelRedis.Where=LEFT`, `CamelRedis.Count=2`
- **WHEN** the producer sends `LMPOP`
- **THEN** the wire frame is `LMPOP 2 a b LEFT COUNT 2`; without `Count` it is `LMPOP 2 a b LEFT`; the nil-or-`[key, [elements]]` reply maps to a JSON null or array body per the binary rule

#### Scenario: LMPOP with an invalid direction is a named error

- **GIVEN** `CamelRedis.Where=UP`
- **WHEN** the producer resolves the arguments
- **THEN** it fails before send; the error names `CamelRedis.Where` and the valid values `LEFT`/`RIGHT`

#### Scenario: LMPOP Count must be positive

- **GIVEN** `CamelRedis.Count=0` (or negative) on `LMPOP`
- **WHEN** the producer resolves the arguments
- **THEN** it fails before send; the error names `CamelRedis.Count` and the positive requirement

#### Scenario: BLMPOP adds the blocking timeout

- **GIVEN** `CamelRedis.Timeout=5` and `CamelRedis.Count=2`
- **WHEN** the producer sends `BLMPOP`
- **THEN** the wire frame is `BLMPOP 5 2 a b LEFT COUNT 2`; a missing or zero `Timeout` sends `0` (block forever, `BLPOP` convention)

#### Scenario: LMOVE argument mapping

- **GIVEN** `CamelRedis.Key=src`, `CamelRedis.Destination=dst`, `CamelRedis.WhereFrom=LEFT`, `CamelRedis.WhereTo=RIGHT`
- **WHEN** the producer sends `LMOVE`
- **THEN** the wire frame is `LMOVE src dst LEFT RIGHT` and the popped element reply follows the `GET` reply rule (`Text` for UTF-8, `Bytes` binary-safe, JSON null on a nil reply)

#### Scenario: LMOVE without directions is a named error

- **GIVEN** missing `CamelRedis.WhereFrom` or `CamelRedis.WhereTo`
- **WHEN** the producer resolves the arguments
- **THEN** it fails before send; the error names the missing header

#### Scenario: LMOVE with an invalid direction is a named error

- **GIVEN** `CamelRedis.WhereFrom=CENTER`
- **WHEN** the producer resolves the arguments
- **THEN** it fails before send; the error names `CamelRedis.WhereFrom` and the valid values `LEFT`/`RIGHT`

#### Scenario: BLMOVE argument mapping

- **GIVEN** the `LMOVE` headers plus `CamelRedis.Timeout=1`
- **WHEN** the producer sends `BLMOVE`
- **THEN** the wire frame is `BLMOVE src dst LEFT RIGHT 1`

#### Scenario: LPOS reads the element from the body

- **GIVEN** `CamelRedis.Key=k`, a text body `x`, `CamelRedis.Rank=1`, `CamelRedis.Count=3`
- **WHEN** the producer sends `LPOS`
- **THEN** the wire frame is `LPOS k x RANK 1 COUNT 3`; without `Rank`/`Count` it is `LPOS k x`; no element header exists

#### Scenario: LPOS accepts empty-string and structured bodies

- **GIVEN** a `Body::Text("")` body, or a JSON object/array body
- **WHEN** the producer resolves the arguments
- **THEN** the element is encoded through the shared value codec (empty string stays an empty bulk argument; objects/arrays go through compact serialization), matching `body_value_bytes`

#### Scenario: LPOS rejects body forms that cannot carry a payload

- **GIVEN** `Body::Empty`, `Body::Stream(..)`, or `Body::Json(null)`
- **WHEN** the producer resolves the arguments
- **THEN** it fails before send; the error names `LPOS` and the payload body rule

#### Scenario: LPOS Count of zero means all matches

- **GIVEN** `CamelRedis.Count=0`
- **WHEN** the producer sends `LPOS`
- **THEN** the wire frame is `LPOS k x COUNT 0` (Redis semantics: 0 returns all matches)

#### Scenario: LPOS Rank of zero is a named error

- **GIVEN** `CamelRedis.Rank=0`
- **WHEN** the producer resolves the arguments
- **THEN** it fails before send; the error names `CamelRedis.Rank` and the non-zero requirement (any positive or negative rank is valid)

### Requirement: Blocking deadline bypass

The system SHALL treat `BLMPOP`, `BZMPOP`, and `BLMOVE` as blocking
endpoints like `BLPOP`/`BRPOP`: no driver response deadline is applied
(`Deadline::Free`, `set_response_timeout(None)`), so a reply slower than the
redis-rs default 500 ms response timeout completes on the same connection.

#### Scenario: BLMPOP survives a delayed reply

- **GIVEN** a fake server that replies to `BLMPOP` after more than 500 ms
- **WHEN** the producer waits for the reply
- **THEN** the reply arrives, exactly one connection is opened, and no reconnect occurs

#### Scenario: BZMPOP survives a delayed reply

- **GIVEN** a fake server that replies to `BZMPOP` after more than 500 ms
- **WHEN** the producer waits for the reply
- **THEN** the reply arrives on one connection

#### Scenario: BLMOVE survives a delayed reply

- **GIVEN** a fake server that replies to `BLMOVE` after more than 500 ms
- **WHEN** the producer waits for the reply
- **THEN** the reply arrives on one connection

### Requirement: Set commands

The system SHALL support `SMISMEMBER` and `SINTERCARD`.

#### Scenario: SMISMEMBER argument mapping

- **GIVEN** `CamelRedis.Key=k`, `CamelRedis.Members=["a","b"]`
- **WHEN** the producer sends `SMISMEMBER`
- **THEN** the wire frame is `SMISMEMBER k a b` and the 1/0 reply array maps to a JSON boolean array

#### Scenario: SINTERCARD argument mapping

- **GIVEN** `CamelRedis.Keys=["a","b"]` and optional `CamelRedis.Limit=10`
- **WHEN** the producer sends `SINTERCARD`
- **THEN** the wire frame is `SINTERCARD 2 a b LIMIT 10`; without `Limit` it is `SINTERCARD 2 a b`; the integer reply is the JSON body

#### Scenario: SINTERCARD Limit must be non-negative

- **GIVEN** `CamelRedis.Limit=-1`
- **WHEN** the producer resolves the arguments
- **THEN** it fails before send; the error names `CamelRedis.Limit` and the non-negative requirement

### Requirement: Sorted set commands

The system SHALL support `ZMPOP`, `BZMPOP`, `ZRANDMEMBER`, `ZDIFF`,
`ZINTER`, `ZUNION`, and `ZRANGEBYLEX`. `ZMPOP`/`BZMPOP` pop by score order
(`CamelRedis.OrderBy`: `MIN`/`MAX`, required) — sorted-set pops have no list
ends. `WEIGHTS`/`AGGREGATE` are out of scope for this change (design.md D4).
Member collection replies follow the `MGET` binary rule.

#### Scenario: ZMPOP argument mapping

- **GIVEN** `CamelRedis.Keys=["a","b"]`, `CamelRedis.OrderBy=MAX`, `CamelRedis.Count=2`
- **WHEN** the producer sends `ZMPOP`
- **THEN** the wire frame is `ZMPOP 2 a b MAX COUNT 2`; the nil-or-`[key, [[member, score], ...]]` reply maps to a JSON null or array body per the binary rule

#### Scenario: ZMPOP with an invalid order is a named error

- **GIVEN** `CamelRedis.OrderBy=LEFT`
- **WHEN** the producer resolves the arguments
- **THEN** it fails before send; the error names `CamelRedis.OrderBy` and the valid values `MIN`/`MAX`

#### Scenario: BZMPOP adds the blocking timeout with an explicit count

- **GIVEN** `CamelRedis.Keys=["a","b"]`, `CamelRedis.OrderBy=MIN`, `CamelRedis.Timeout=0`, `CamelRedis.Count=2`
- **WHEN** the producer sends `BZMPOP`
- **THEN** the wire frame is `BZMPOP 0 2 a b MIN COUNT 2` (0 blocks forever); without `Count` the frame ends `MIN COUNT 1`

#### Scenario: ZRANDMEMBER with a negative count allows repeats

- **GIVEN** `CamelRedis.Key=k` and `CamelRedis.Count=-5`
- **WHEN** the producer sends `ZRANDMEMBER`
- **THEN** the wire frame is `ZRANDMEMBER k -5` (negative count is valid Redis semantics: repeats allowed); without `Count` the wire frame is `ZRANDMEMBER k` and the single-member reply follows the `GET` reply rule; with `Count` the array reply follows the `MGET` binary rule

#### Scenario: ZDIFF argument mapping

- **GIVEN** `CamelRedis.Keys=["a","b"]`
- **WHEN** the producer sends `ZDIFF`
- **THEN** the wire frame is `ZDIFF 2 a b` and the member array reply maps to a JSON array per the binary rule

#### Scenario: ZINTER argument mapping

- **GIVEN** `CamelRedis.Keys=["a","b","c"]`
- **WHEN** the producer sends `ZINTER`
- **THEN** the wire frame is `ZINTER 3 a b c`

#### Scenario: ZUNION argument mapping

- **GIVEN** `CamelRedis.Keys=["a","b"]`
- **WHEN** the producer sends `ZUNION`
- **THEN** the wire frame is `ZUNION 2 a b`

#### Scenario: ZRANGEBYLEX argument mapping

- **GIVEN** `CamelRedis.Key=k`, `CamelRedis.Min=[a`, `CamelRedis.Max=+`
- **WHEN** the producer sends `ZRANGEBYLEX`
- **THEN** the wire frame is `ZRANGEBYLEX k [a +`; a missing bound fails before send with a named error

### Requirement: Hash commands

The system SHALL support `HRANDFIELD` and `HINCRBYFLOAT`. Without `Count` the
single `HRANDFIELD` member reply follows the `GET` reply rule; with `Count`
the member array follows the `MGET` binary rule.

#### Scenario: HRANDFIELD with a negative count allows repeats

- **GIVEN** `CamelRedis.Key=k` and `CamelRedis.Count=3` (or any i64, negative = repeats)
- **WHEN** the producer sends `HRANDFIELD`
- **THEN** the wire frame is `HRANDFIELD k 3`; without `Count` it is `HRANDFIELD k` and the single-member reply follows the `GET` reply rule

#### Scenario: HINCRBYFLOAT argument mapping

- **GIVEN** `CamelRedis.Key=k`, `CamelRedis.Field=balance`, `CamelRedis.Increment=1.5`
- **WHEN** the producer sends `HINCRBYFLOAT`
- **THEN** the wire frame is `HINCRBYFLOAT k balance 1.5` and the bulk-string float reply is the JSON body

#### Scenario: HINCRBYFLOAT with a malformed increment is a named error

- **GIVEN** `CamelRedis.Increment=abc`
- **WHEN** the producer resolves the arguments
- **THEN** it fails before send; the error names `CamelRedis.Increment` and the received value

### Requirement: Generic key commands

The system SHALL support `UNLINK`, `COPY` (with `REPLACE` and `DB`), and
`TOUCH`. `CamelRedis.Db` on `COPY` is optional: absent means the current
database (no `DB` argument sent), present means the given database.

#### Scenario: UNLINK argument mapping

- **GIVEN** `CamelRedis.Key=k`
- **WHEN** the producer sends `UNLINK`
- **THEN** the wire frame is `UNLINK k` (mirrors `DEL`) and the integer reply is the JSON body

#### Scenario: COPY with REPLACE and DB

- **GIVEN** `CamelRedis.Key=src`, `CamelRedis.Destination=dst`, `CamelRedis.Replace=true`, `CamelRedis.Db=3`
- **WHEN** the producer sends `COPY`
- **THEN** the wire frame is `COPY src dst DB 3 REPLACE` and the 0/1 reply maps to a JSON boolean

#### Scenario: COPY without optionals targets the current database

- **GIVEN** `CamelRedis.Key=src`, `CamelRedis.Destination=dst`, no `CamelRedis.Db`, no `CamelRedis.Replace`
- **WHEN** the producer sends `COPY`
- **THEN** the wire frame is exactly `COPY src dst` (no `DB` argument; the copy targets the current database)

#### Scenario: COPY without a destination is a named error

- **GIVEN** `CamelRedis.Key=src` and no `CamelRedis.Destination`
- **WHEN** the producer resolves the arguments
- **THEN** it fails before send; the error names `CamelRedis.Destination`

#### Scenario: TOUCH argument mapping

- **GIVEN** `CamelRedis.Keys=["a","b"]`
- **WHEN** the producer sends `TOUCH`
- **THEN** the wire frame is `TOUCH a b` and the integer reply is the JSON body

### Requirement: Retry classification

The system SHALL classify retry safety per command in `is_idempotent_command`:
retry-safe iff a retry after an unconfirmed success converges server state and
does not lose data the first attempt reported. Count-style replies may differ
across attempts (`DEL`/`UNLINK` 1 then 0) and random-sample reads may return a
fresh draw (`SRANDMEMBER` precedent). The classifier is option-blind, so the
worst-case option form governs: `SET` classifies not idempotent because
`ReturnOldValue` and `NX` lose replay data, and the classifier cannot be
overridden per route (the producer always consults it); routes needing
plain-`SET` retry semantics use route-level redelivery, which re-executes the
whole exchange and may duplicate other side effects. Reads (`GETRANGE`,
`SMISMEMBER`, `SINTERCARD`, `ZDIFF`, `ZINTER`, `ZUNION`, `ZRANGEBYLEX`,
`ZRANDMEMBER`, `HRANDFIELD`, `LPOS`, `TOUCH`) and converging writes
(`SETRANGE`, `UNLINK`) are retry-safe. The pop/move family (`LMPOP`, `BLMPOP`,
`ZMPOP`, `BZMPOP`, `LMOVE`, `BLMOVE`), `SET`, `GETDEL`, `GETEX`, `COPY`, and
`HINCRBYFLOAT` are not. Batch requests SHALL be idempotent iff every entry is
idempotent. `EVAL`/`FCALL` SHALL classify not idempotent unless the endpoint
sets the `idempotent` opt-in. For single-command dispatch the command is fixed
by the endpoint URI and SHALL never be overridable per exchange (ADR-0032);
batch entry selection is permitted only through the validated typed allowlist
under `command=multi`.

#### Scenario: pop family is not retried after an unconfirmed success

- **GIVEN** a transport error after `LMPOP` (or `ZMPOP`/`LMOVE`/`BLMOVE`) was
  flushed
- **WHEN** the retry classifier evaluates the command
- **THEN** the command is classified not idempotent, so the driver does not
  re-issue it (a re-issued pop reports empty and silently loses the popped
  item)

#### Scenario: SET with option forms is not retried after an unconfirmed success

- **GIVEN** a transport error after `SET` (with `NX` or `ReturnOldValue`) was
  flushed
- **WHEN** the retry classifier evaluates the command
- **THEN** the command is classified not idempotent: the classifier cannot
  see options, and a retry would report the wrong old value or a false NX
  failure

#### Scenario: reads are retried safely

- **GIVEN** a transport error after `SMISMEMBER` (or any listed read) was
  flushed
- **WHEN** the retry classifier evaluates the command
- **THEN** the command is classified idempotent and may be retried

#### Scenario: batch idempotence aggregates entries

- **GIVEN** a `multi` batch mixing `GET` (idempotent) and `GETDEL` (not)
- **WHEN** the retry classifier evaluates the batch
- **THEN** the batch is classified not idempotent and is not retried after an
  unconfirmed success

### Requirement: Script execution (eval and fcall)

The system SHALL run server-side scripts through `command=eval` (Lua via
`EVALSHA`) and `command=fcall` (Redis functions via `FCALL`). The script source
SHALL come only from the endpoint URI: parameter `script` (inline source) or
`scriptFile` (path), mutually exclusive; script text arriving in any exchange
header SHALL be rejected with a named error (ADR-0032: exchange data is
untrusted; executable text is an interpretable sink). `scriptFile` SHALL
resolve relative paths against the route-file directory the same way the DSL
resolves `source_file`, with no multi-directory guessing; when no route file
context exists (programmatic endpoint construction), relative paths SHALL
resolve against the process working directory (the documented fallback) and
all failures SHALL name the resolved path. Keys and args SHALL come from the
exchange body as a JSON object `{"keys": [<string>...], "args": [<string|number>...]}`;
this is the documented scoped exception to header-sourced key addressing
(see "Script operand binding"). `fcall` SHALL take the invoked function name
from endpoint parameter `function` (required). `EVALSHA` invocations SHALL
recover from `NOSCRIPT` by reloading the script once and re-invoking;
`NOSCRIPT` outside that path stays a non-transient business error. For
`fcall`, the function library SHALL be loaded lazily on first use with
`FUNCTION LOAD REPLACE` (repeat exchanges reuse the loaded library); a
"function not found" reply SHALL trigger at most one reload-and-reinvoke per
attempt. A failed reload, a still-failing re-invocation, or malformed body
operands SHALL surface named errors; these are terminal, never transient.
Missing `script`/`scriptFile`, both set at once, or `fcall` without
`function` SHALL fail with named errors before send. Replies SHALL map per the
body reply contract: nil to JSON null, integers to JSON numbers, status and
UTF-8 bulk strings to text, non-UTF-8 bulk strings to bytes, nested arrays
recursively.

#### Scenario: eval runs an inline script with keys and args from the body

- **GIVEN** an endpoint `command=eval` with `script` set to inline Lua and a
  body `{"keys": ["k1"], "args": ["v1"]}`
- **WHEN** the producer executes
- **THEN** the client invokes `EVALSHA` with those keys and args and the reply
  is returned as the exchange body per the reply mapping

#### Scenario: NOSCRIPT reloads once and re-invokes

- **GIVEN** a `command=eval` endpoint whose `EVALSHA` receives a `NOSCRIPT`
  reply (server restarted or script evicted)
- **WHEN** the script call fails
- **THEN** the client reloads the script and re-invokes once; the second
  result is the exchange result; a second `NOSCRIPT` surfaces a named error

#### Scenario: fcall loads its library and invokes the function

- **GIVEN** a `command=fcall` endpoint with `function=lock` and
  `scriptFile=functions/lock.lua` (a library defining `lock`)
- **WHEN** the producer executes its first exchange
- **THEN** the library is loaded with `FUNCTION LOAD REPLACE` and
  `FCALL lock <n> <keys> <args>` runs; subsequent exchanges reuse the loaded
  library without reloading

#### Scenario: fcall recovers from a missing function with one reload

- **GIVEN** a `command=fcall` endpoint whose `FCALL` returns "function not
  found" (server restarted or library evicted)
- **WHEN** the call fails
- **THEN** the client reloads the library and re-invokes once; a repeated
  failure surfaces a named error and is not retried

#### Scenario: script text in a header is rejected

- **GIVEN** any exchange carrying script text in a header (for example
  `CamelRedis.Script`) on an `eval`/`fcall` endpoint
- **WHEN** the producer resolves the request
- **THEN** it fails before send with a named error; the header content is
  never interpreted

#### Scenario: scriptFile resolves against the route-file directory

- **GIVEN** a route file in directory `D` with `scriptFile=scripts/lock.lua`
- **WHEN** the endpoint is created and executes
- **THEN** the script is read from `D/scripts/lock.lua`; a missing file fails
  with a named error naming the resolved path

#### Scenario: malformed body operands are a named error

- **GIVEN** a `command=eval` endpoint and a body that is not a JSON object
  with `keys` and `args` arrays (for example `{"keys": "k1"}`)
- **WHEN** the producer resolves the request
- **THEN** it fails before send with a named error naming the body shape

#### Scenario: eval and fcall are non-idempotent without opt-in

- **GIVEN** a `command=eval` endpoint (no `idempotent` opt-in) and an I/O
  failure after the request was flushed
- **WHEN** the retry classifier evaluates the outcome
- **THEN** the request is classified outcome-unknown and is NOT retried

#### Scenario: idempotent opt-in allows outcome-unknown retry

- **GIVEN** a `command=fcall` endpoint with `idempotent=true` and an I/O
  failure after flush
- **WHEN** the retry classifier evaluates the outcome
- **THEN** the request is retried under the retry budget

### Requirement: Atomic batch execution (multi)

The system SHALL execute a batch of commands from header `CamelRedis.Batch`
through `command=multi`. The header SHALL be a JSON array of entries with a
required `command` name that parses into the `RedisCommand` enum and belongs
to the batchable subset (single-shot producer commands with batch encoders;
blocking pops, consumer and pubsub commands are excluded), plus per-entry
arguments using the same names and semantics as the existing
`CamelRedis.*` header arguments (`key`/`keys`, `field`, `member`, `score`,
count/limit/range scalars). This typed allowlist SHALL fail closed: unknown
commands, non-batchable commands, unknown entry fields, wrong-typed
arguments, missing required arguments, malformed entries, or a count above
the documented cap SHALL produce named errors before send, naming the
offending index where applicable. Unlike single-command dispatch (where
`CamelRedis.Command` is rejected and the command is fixed by the URI),
entry-level `command` selection is permitted here because `command=multi` is
the operator opt-in and every entry is validated against the typed allowlist.
Per-entry values SHALL come from the exchange body as a JSON array aligned by
index; body element `i` is entry `i`'s value payload (scalar, array of
scalars for multi-value commands, or null for entries that take no body
value), and a length mismatch SHALL fail before send. With `atomic=true`
(default) the batch SHALL be sent as `MULTI`/`EXEC`
(`redis::pipe().atomic()`); with `atomic=false` as a plain pipeline. Error
precedence SHALL distinguish proven non-execution from uncertain outcomes:
`EXECABORT` and whole-request server rejections (`LOADING`, `BUSY`) mean no
entry executed; a connection failure before send means nothing executed; a
connection failure after entries were flushed (or with unknown flush status)
is outcome-unknown and follows the execution-outcome split — the batch is
replayed only when every entry is idempotent. An `EXEC` or pipeline reply
carrying per-entry errors means earlier entries DID execute and the batch
SHALL never be replayed wholesale — entry errors surface as named errors and
are never transient. `EXECABORT` SHALL be a non-transient error and SHALL
never be retried. A batch SHALL be classified idempotent iff every entry is
idempotent per `is_idempotent_command`.

#### Scenario: allowlisted batch runs atomically

- **GIVEN** `CamelRedis.Batch` listing `SET` and `INCR` entries with values in
  the body and `atomic=true`
- **WHEN** the producer executes
- **THEN** the commands are sent as one `MULTI`/`EXEC` transaction and the
  per-command replies are returned

#### Scenario: unknown batch command fails closed before send

- **GIVEN** `CamelRedis.Batch` containing an entry whose `command` does not
  parse into `RedisCommand`
- **WHEN** the producer resolves the batch
- **THEN** it fails before send with a named error naming the header and the
  entry index; nothing is sent

#### Scenario: non-batchable command is rejected

- **GIVEN** `CamelRedis.Batch` containing a consumer-only or blocking entry
  (for example `XREADGROUP` or `BLPOP`)
- **WHEN** the producer resolves the batch
- **THEN** it fails before send with a named error naming the index

#### Scenario: unknown entry field or bad argument type fails closed

- **GIVEN** `CamelRedis.Batch` containing an entry with an unknown field or a
  non-numeric `score`
- **WHEN** the producer resolves the batch
- **THEN** it fails before send with a named error naming the index and the
  field

#### Scenario: body length mismatch fails closed

- **GIVEN** `CamelRedis.Batch` with 3 entries and a body array of 2 elements
- **WHEN** the producer resolves the batch
- **THEN** it fails before send with a named error naming the expected and
  actual counts

#### Scenario: partial entry failure is never replayed

- **GIVEN** an `atomic=false` pipeline where entry `INCR` succeeded and a
  later entry returned a per-command error (for example `WRONGTYPE`)
- **WHEN** the reply is evaluated
- **THEN** the batch is NOT replayed (the `INCR` already took effect); the
  entry error surfaces as a named error and is classified never-transient

#### Scenario: atomic batch disconnect after flush is outcome-unknown

- **GIVEN** an `atomic=true` batch whose connection drops after the
  transaction was flushed but before the reply arrived
- **WHEN** the retry classifier evaluates the failure
- **THEN** it is outcome-unknown (the `EXEC` may have committed atomically):
  the batch is replayed only when every entry is idempotent

#### Scenario: plain pipeline disconnect after flush is outcome-unknown

- **GIVEN** an `atomic=false` pipeline whose connection drops after entries
  were flushed
- **WHEN** the retry classifier evaluates the failure
- **THEN** it is outcome-unknown (some entries may have executed): the
  pipeline is replayed only when every entry is idempotent

#### Scenario: non-idempotent batch is not retried after an unconfirmed success

- **GIVEN** a batch containing `INCR` (not idempotent) and an I/O failure
  after flush
- **WHEN** the retry classifier evaluates the batch
- **THEN** the batch is outcome-unknown and is NOT retried

#### Scenario: all-idempotent batch is retried after an unconfirmed success

- **GIVEN** a batch containing only idempotent entries (for example read-class
  commands) and an I/O failure after flush
- **WHEN** the retry classifier evaluates the batch
- **THEN** the batch is retried under the retry budget

#### Scenario: EXECABORT is never transient

- **GIVEN** a `multi` transaction whose `EXEC` returns `EXECABORT`
- **WHEN** the retry classifier evaluates the error
- **THEN** it is classified never-transient and is not retried

### Requirement: Execution-outcome retry split

The system SHALL classify every failed request attempt into exactly one of
three buckets and apply bucket-specific replay rules. Replay safety is the
conjunction of two different questions: is the error class transient (an
error classification), and was the request's effect confirmed absent or
idempotent (a replay decision). (1) Not-executed: the request provably never
ran — connection checkout/dial failure before send, or a whole-request server
reply of `LOADING`/`BUSY` (`BUSYKEY` excluded, it is semantic) — and SHALL
always be replayed under the retry budget, even for non-idempotent commands.
(2) Outcome-unknown: I/O failed after the request was flushed, or the flush
status is unknown (conservative) — and SHALL be replayed only when the
request is idempotent (single command per `is_idempotent_command`; batch iff
every entry is; `eval`/`fcall` only with the `idempotent` endpoint opt-in).
(3) Never-transient: `EXECABORT`, per-command `EXEC` errors, fatal
config/auth errors (for example `NOAUTH`, `WRONGPASS`, `NOPERM`), and all
other semantic/business errors — never replayed. The invariant
`invalidates_connection(e) ⇒ is_transient_redis_error(e)` SHALL be preserved
(error classification, independent of replay): `LOADING`/`BUSY` are
transient but never invalidate the connection; connection-lost errors are
transient and do invalidate; an outcome-unknown failure on a non-idempotent
request invalidates the connection but is never replayed. Not-executed
retries SHALL share the existing consecutive-failure budget. The split SHALL
be recorded in ADR-0013 together with the ADR-0032 trust boundary (scripts
from URI/file only).

#### Scenario: not-executed retries for a non-idempotent command

- **GIVEN** a `GETDEL` request (not idempotent) whose connection checkout
  fails before send
- **WHEN** the retry classifier evaluates the failure
- **THEN** it is classified not-executed and IS retried under the retry
  budget

#### Scenario: LOADING and BUSY are always retried and never invalidate

- **GIVEN** any request whose whole-request reply is `LOADING` (or `BUSY`)
- **WHEN** the retry classifier evaluates the reply
- **THEN** it is classified not-executed and retried; the connection is NOT
  invalidated

#### Scenario: outcome-unknown respects idempotence

- **GIVEN** a request whose I/O fails after flush (or with unknown flush
  status)
- **WHEN** the retry classifier evaluates the failure
- **THEN** it is classified outcome-unknown and replayed only if the
  request's idempotence (per command, per batch, or per `eval`/`fcall`
  opt-in) holds

#### Scenario: non-idempotent outcome-unknown invalidates but never replays

- **GIVEN** a `GETDEL` request whose connection drops after flush
- **WHEN** the retry classifier evaluates the failure
- **THEN** the connection is invalidated (it cannot be trusted) but the
  request is NOT replayed; the two decisions are independent

#### Scenario: invalidation remains a strict subset of transient errors

- **GIVEN** the error corpus of transport and server errors
- **WHEN** the subset property test evaluates `invalidates_connection(e)`
  against `is_transient_redis_error(e)`
- **THEN** every invalidating error is transient and at least one transient
  class (for example `LOADING`) is non-invalidating

