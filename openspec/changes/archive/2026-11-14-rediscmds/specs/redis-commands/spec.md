## ADDED Requirements

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

The system SHALL classify retry safety per command in
`is_idempotent_command`: retry-safe iff a retry after an unconfirmed success
converges server state and does not lose data the first attempt reported.
Count-style replies may differ across attempts (`DEL`/`UNLINK` 1 then 0) and
random-sample reads may return a fresh draw (`SRANDMEMBER` precedent). The
classifier is option-blind, so the worst-case option form governs: `SET` classifies
not idempotent because `ReturnOldValue` and `NX` lose replay data, and the
classifier cannot be overridden per route (the producer always consults it);
routes needing plain-`SET` retry semantics use route-level redelivery, which
re-executes the whole exchange and may duplicate other side effects.
Reads (`GETRANGE`, `SMISMEMBER`, `SINTERCARD`, `ZDIFF`, `ZINTER`, `ZUNION`,
`ZRANGEBYLEX`, `ZRANDMEMBER`, `HRANDFIELD`, `LPOS`, `TOUCH`) and converging
writes (`SETRANGE`, `UNLINK`) are retry-safe. The pop/move family (`LMPOP`,
`BLMPOP`, `ZMPOP`, `BZMPOP`, `LMOVE`, `BLMOVE`), `SET`, `GETDEL`, `GETEX`,
`COPY`, and `HINCRBYFLOAT` are not.

#### Scenario: pop family is not retried after an unconfirmed success

- **GIVEN** a transport error after `LMPOP` (or `ZMPOP`/`LMOVE`/`BLMOVE`) was flushed
- **WHEN** the retry classifier evaluates the command
- **THEN** the command is classified not idempotent, so the driver does not re-issue it (a re-issued pop reports empty and silently loses the popped item)

#### Scenario: SET with option forms is not retried after an unconfirmed success

- **GIVEN** a transport error after `SET` (with `NX` or `ReturnOldValue`) was flushed
- **WHEN** the retry classifier evaluates the command
- **THEN** the command is classified not idempotent: the classifier cannot see options, and a retry would report the wrong old value or a false NX failure

#### Scenario: reads are retried safely

- **GIVEN** a transport error after `SMISMEMBER` (or any listed read) was flushed
- **WHEN** the retry classifier evaluates the command
- **THEN** the command is classified idempotent and may be retried
