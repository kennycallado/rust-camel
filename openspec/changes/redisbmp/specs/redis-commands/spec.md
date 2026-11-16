# redis-commands — delta for redisbmp

## ADDED Requirements

### Requirement: Bitmap bit access (SETBIT and GETBIT)

The system SHALL support `SETBIT` and `GETBIT` with `CamelRedis.Key` and
`CamelRedis.BitOffset` (u64 in `0..=4294967295`, the 512 MB bitstring cap;
out-of-range values fail before send with a named error), and
`CamelRedis.BitValue` (`0` or `1`) for `SETBIT`. The bit value is an argument
header, not a body payload (ADR-0098 covers values only). Arguments SHALL be
validated before send with named errors; the reply (`SETBIT`: previous bit,
`GETBIT`: current bit) SHALL map to the exchange body as a JSON number.

#### Scenario: SETBIT wire frame

- **GIVEN** `command=SETBIT`, `CamelRedis.Key=k`, `CamelRedis.BitOffset=7`,
  `CamelRedis.BitValue=1`
- **WHEN** the producer sends the command
- **THEN** the wire frame is `SETBIT k 7 1` and the reply integer (the
  previous bit) becomes the body as a JSON number

#### Scenario: SETBIT rejects a non-binary bit value before send

- **GIVEN** `command=SETBIT` and `CamelRedis.BitValue=2`
- **WHEN** the producer validates the arguments
- **THEN** it fails before send and the error names `CamelRedis.BitValue`

### Requirement: Bitmap range queries (BITCOUNT and BITPOS)

The system SHALL support `BITCOUNT` and `BITPOS` with `CamelRedis.Key`, a
`BITPOS`-only `CamelRedis.BitValue` (`0` or `1`), optional range headers
`CamelRedis.Start`/`CamelRedis.End` (i64; negative values index from the end,
Redis-compatible bounds), and optional `CamelRedis.BitRangeUnit`
(`BYTE` or `BIT`, default `BYTE` — the Redis default is a range-unit default
with no value semantics, so it is documented, not accidental). The range
grammar follows Redis 7: `BITCOUNT` requires `Start` and `End` together;
`BITPOS` may give `Start` alone but a unit requires both endpoints;
violations (`BITCOUNT` with only one endpoint, `End` without `Start`, a unit
without a full range) SHALL fail before send with named errors. Replies
SHALL map to the exchange body as a JSON number (`BITPOS` returns `-1` when
the bit is never set in the scanned range).

#### Scenario: BITCOUNT with BIT ranges

- **GIVEN** `command=BITCOUNT`, `CamelRedis.Key=k`, `CamelRedis.Start=0`,
  `CamelRedis.End=2`, `CamelRedis.BitRangeUnit=BIT`
- **WHEN** the producer sends the command
- **THEN** the wire frame is `BITCOUNT k 0 2 BIT` and the reply integer
  becomes the body as a JSON number

#### Scenario: BITCOUNT rejects an incomplete range before send

- **GIVEN** `command=BITCOUNT`, `CamelRedis.Key=k`, `CamelRedis.End=2` only
  (or `CamelRedis.Start=0` only)
- **WHEN** the producer validates the arguments
- **THEN** it fails before send and the error names `CamelRedis.Start` and
  `CamelRedis.End` and the BITCOUNT both-endpoints rule

#### Scenario: BITPOS rejects a unit without a full range before send

- **GIVEN** `command=BITPOS`, `CamelRedis.Key=k`, `CamelRedis.BitValue=1`,
  `CamelRedis.Start=0`, `CamelRedis.BitRangeUnit=BIT` (no `CamelRedis.End`)
- **WHEN** the producer validates the arguments
- **THEN** it fails before send and the error names `CamelRedis.End` and the
  unit-requires-both-endpoints rule

### Requirement: Bitmap aggregation (BITOP)

The system SHALL support `BITOP` with `CamelRedis.BitOperation`
(`AND`, `OR`, `XOR`, `NOT`), `CamelRedis.Destination`, and
`CamelRedis.Keys` (the SINTERSTORE convention). `NOT` SHALL require exactly
one source key; `AND`/`OR`/`XOR` SHALL require one or more. Violations fail
before send with named errors. The reply (the destination's new length) SHALL
map to the body as a JSON number.

#### Scenario: BITOP NOT is single-key only

- **GIVEN** `command=BITOP`, `CamelRedis.BitOperation=NOT`,
  `CamelRedis.Destination=d`, `CamelRedis.Keys=[a, b]`
- **WHEN** the producer validates the arguments
- **THEN** it fails before send and the error names `CamelRedis.Keys` and the
  single-source rule for `NOT`

#### Scenario: BITOP OR aggregates N sources

- **GIVEN** `command=BITOP`, `CamelRedis.BitOperation=OR`,
  `CamelRedis.Destination=d`, `CamelRedis.Keys=[a, b, c]`
- **WHEN** the producer sends the command
- **THEN** the wire frame is `BITOP OR d a b c` and the reply integer becomes
  the body as a JSON number

### Requirement: BITFIELD sub-command encoding

The system SHALL support `BITFIELD` and `BITFIELD_RO` with `CamelRedis.Key`
and `CamelRedis.BitFieldOps` (string list). Entries SHALL tokenize to
`GET <type> <offset>`, `SET <type> <offset> <value>`,
`INCRBY <type> <offset> <increment>`, or `OVERFLOW <WRAP|SAT|FAIL>`, where
`<type>` is `[iu]<width>` with signed widths 1..=64 and unsigned widths
1..=63 (`u64` is invalid). An absolute `<offset>` is a non-negative integer
fitting i64 (Redis parsing bounds). A `#<n>` offset SHALL be evaluated
independently as `n × <this entry's width>` bits from the start — the Redis
rule: the multiplier is the current entry's encoding width, never a running
position (two `GET u8 #1` entries address the same bits; `i16 #1` is bits
16–31 while `u8 #1` is bits 8–15) — with checked multiplication that fails
before send with a named error on overflow. Malformed entries SHALL fail
before send with a named error. Overflow behaviour SHALL always be explicit,
with an effective mode in force *before every mutating entry*
(`SET`/`INCRBY`): the initial
mode comes from `CamelRedis.BitFieldOverflow` (`WRAP|SAT|FAIL`) and inline
`OVERFLOW` entries switch the mode for following entries; a `SET`/`INCRBY`
entry reached with no effective mode SHALL fail before send with a
`CamelRedis.BitFieldOverflow` error (there is no default; the server default
`WRAP` silently wraps counters and must never apply by accident — a trailing
`OVERFLOW` entry does not retroactively cover a preceding mutation). `GET`
entries need no mode. `BITFIELD_RO` SHALL accept only `GET` entries and
SHALL reject `CamelRedis.BitFieldOverflow` and `SET`/`INCRBY`/`OVERFLOW`
entries with named errors. Replies (an array of integers or nils) SHALL map
to the body as a JSON array of numbers and nulls.

#### Scenario: BITFIELD overflow must be explicit

- **GIVEN** `command=BITFIELD`, `CamelRedis.Key=k`, and
  `CamelRedis.BitFieldOps=["INCRBY u8 #0 1"]` with no
  `CamelRedis.BitFieldOverflow` and no `OVERFLOW` entry
- **WHEN** the producer validates the arguments
- **THEN** it fails before send and the error names
  `CamelRedis.BitFieldOverflow`

#### Scenario: a trailing OVERFLOW entry does not cover a preceding mutation

- **GIVEN** `command=BITFIELD`, `CamelRedis.Key=k`, and
  `CamelRedis.BitFieldOps=["INCRBY u8 0 1", "OVERFLOW SAT"]`
- **WHEN** the producer validates the arguments
- **THEN** it fails before send with a `CamelRedis.BitFieldOverflow` error:
  the `INCRBY` would otherwise execute under the server's implicit `WRAP`

#### Scenario: BITFIELD encodes GET/SET/INCRBY entries

- **GIVEN** `command=BITFIELD`, `CamelRedis.Key=k`,
  `CamelRedis.BitFieldOverflow=WRAP`, and
  `CamelRedis.BitFieldOps=["GET u8 0", "SET i16 #8 -1", "INCRBY u4 #0 3"]`
- **WHEN** the producer sends the command
- **THEN** the wire frame is
  `BITFIELD k OVERFLOW WRAP GET u8 0 SET i16 #8 -1 INCRBY u4 #0 3` and the
  reply array of integers/nils becomes a JSON array of numbers/nulls

#### Scenario: repeated #n offsets address the same bits

- **GIVEN** a key holding the bytes `[0, 1, 2]` and
  `command=BITFIELD_RO` with `CamelRedis.BitFieldOps=["GET u8 #1", "GET u8 #1"]`
- **WHEN** the producer sends the command
- **THEN** both entries address bits 8–15 (`1 × 8`, evaluated independently,
  not accumulated) and the reply is `[1, 1]`

#### Scenario: #n multiplies the current entry's width

- **GIVEN** `command=BITFIELD_RO`, `CamelRedis.Key=k`, and
  `CamelRedis.BitFieldOps=["GET u8 #1", "GET i16 #1"]`
- **WHEN** the producer sends the command
- **THEN** the entries address bits 8–15 (`1 × 8`) and bits 16–31
  (`1 × 16`) respectively — the multiplier is each entry's own width

#### Scenario: #n offset multiplication overflow is refused before send

- **GIVEN** `command=BITFIELD`, `CamelRedis.Key=k`,
  `CamelRedis.BitFieldOverflow=WRAP`, and
  `CamelRedis.BitFieldOps=["INCRBY i64 #9223372036854775807 1"]`
- **WHEN** the producer validates the arguments
- **THEN** it fails before send with a named error naming
  `CamelRedis.BitFieldOps`: `n × width` overflows the offset type

#### Scenario: BITFIELD_RO rejects write entries

- **GIVEN** `command=BITFIELD_RO`, `CamelRedis.Key=k`, and
  `CamelRedis.BitFieldOps=["SET u8 0 1"]`
- **WHEN** the producer validates the arguments
- **THEN** it fails before send and the error names `CamelRedis.BitFieldOps`
  and the read-only rule

### Requirement: HyperLogLog commands (PFADD, PFCOUNT and PFMERGE)

The system SHALL support `PFADD` (element in the body per ADR-0098),
`PFCOUNT` (`CamelRedis.Keys`, one or more keys; single-key is a one-element
list), and `PFMERGE` (`CamelRedis.Destination` plus `CamelRedis.Keys`, one or
more sources). Missing or empty key lists SHALL fail before send with named
errors. Replies SHALL map as: `PFADD` integer (1 if the structure changed) to
a JSON number, `PFCOUNT` integer cardinality to a JSON number, `PFMERGE`
status to text.

#### Scenario: PFCOUNT over multiple keys

- **GIVEN** `command=PFCOUNT`, `CamelRedis.Keys=[h1, h2]`
- **WHEN** the producer sends the command
- **THEN** the wire frame is `PFCOUNT h1 h2`, the reply integer becomes the
  body as a JSON number, and the command is classified idempotent (its
  single-key form may refresh the HLL's cached cardinality — a converging
  internal write; the multi-key form only reads its sources)

#### Scenario: PFMERGE merges into the destination

- **GIVEN** `command=PFMERGE`, `CamelRedis.Destination=h3`,
  `CamelRedis.Keys=[h1, h2]`
- **WHEN** the producer sends the command
- **THEN** the wire frame is `PFMERGE h3 h1 h2` and the status reply becomes
  the body as text

### Requirement: WAIT durability probe

The system SHALL support `WAIT` with `CamelRedis.NumReplicas` (u64) and
`CamelRedis.WaitTimeout` (u64 milliseconds, distinct from the TTL header
`CamelRedis.Timeout`). Both SHALL be wire-checked before send: Redis parses
`WAIT`'s arguments as signed 64-bit, so values above `i64::MAX` (such as
`u64::MAX`) SHALL fail before send with named header errors, and
`CamelRedis.WaitTimeout` must be `> 0`. The reply (the number of replicas
that acknowledged
the writes sent on the invoking connection up to the `WAIT` command) SHALL
become the exchange body as a JSON number per the body-only contract. `WAIT`
SHALL be defined as a connection-local acknowledgement probe, not a
durability guarantee for earlier route writes: a `WAIT` served on a
different connection (pool checkout) or after a reconnect (write offset
lost) acknowledges nothing about earlier exchanges, and the docs shall say
so. `WAIT` SHALL be classified idempotent: no server state change; a replay
re-probes the current connection's offset and may return a different count
(count-style reply, `DEL` precedent). It SHALL never hang forever:
`CamelRedis.WaitTimeout` of zero (which makes Redis block forever) and
missing values SHALL fail before send with named errors, the server-side
wait per attempt is bounded by `CamelRedis.WaitTimeout`, and a WAIT-only
aggregate deadline of `CamelRedis.WaitTimeout + endpoint timeout` SHALL bound
the whole exchange (connection checkout, retries, backoff, replies); the
deadline SHALL be constructed with checked arithmetic, and an unrepresentable
deadline fails before send with a named error (never a panic). On deadline
expiry the exchange fails with a named timeout error naming
`CamelRedis.WaitTimeout` and the connection is released normally.

#### Scenario: WAIT reply becomes the body

- **GIVEN** `command=WAIT`, `CamelRedis.NumReplicas=1`,
  `CamelRedis.WaitTimeout=1000`
- **WHEN** the producer sends the command
- **THEN** the wire frame is `WAIT 1 1000`, the reply integer (acked
  replicas) becomes the body as a JSON number, and the command is classified
  idempotent

#### Scenario: WAIT with a zero timeout is refused before send

- **GIVEN** `command=WAIT` and `CamelRedis.WaitTimeout=0`
- **WHEN** the producer validates the arguments
- **THEN** it fails before send and the error names
  `CamelRedis.WaitTimeout`

#### Scenario: WAIT is connection-local, not a route-wide durability barrier

- **GIVEN** a route wrote on connection A and the `WAIT` exchange is served
  on another connection (or on a re-created connection after a reconnect)
- **WHEN** `WAIT` returns N acked replicas
- **THEN** the acknowledgement covers only the writes sent on the invoking
  connection up to the `WAIT` command; the exchange does not claim durability
  for the earlier write on connection A, and the docs state this scope

#### Scenario: a stalled server cannot hang WAIT

- **GIVEN** `command=WAIT`, `CamelRedis.NumReplicas=1`,
  `CamelRedis.WaitTimeout=1000`, and the server stops replying
- **WHEN** the aggregate deadline (`WaitTimeout` + endpoint timeout) expires
- **THEN** the exchange fails with a named timeout error naming
  `CamelRedis.WaitTimeout`; it does not wait forever

#### Scenario: bounded retries fit inside the WAIT deadline

- **GIVEN** `command=WAIT` and transport errors that trigger bounded retries
- **WHEN** the retry loop would exceed the aggregate deadline
- **THEN** the exchange fails with a named timeout error naming
  `CamelRedis.WaitTimeout` instead of retrying past the deadline

#### Scenario: WAIT arguments above the wire range are refused before send

- **GIVEN** `command=WAIT` and `CamelRedis.WaitTimeout=u64::MAX` (or
  `CamelRedis.NumReplicas` above `i64::MAX`)
- **WHEN** the producer validates the arguments
- **THEN** it fails before send and the error names the offending header and
  the signed-64-bit Redis wire range

#### Scenario: an unrepresentable aggregate deadline is refused before send

- **GIVEN** `command=WAIT` with `CamelRedis.WaitTimeout` and an endpoint
  `timeout` whose sum cannot be represented as the aggregate deadline
- **WHEN** the producer constructs the deadline
- **THEN** it fails before send with a named error naming
  `CamelRedis.WaitTimeout`; it neither panics nor waits unbounded

## MODIFIED Requirements

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
`ZRANDMEMBER`, `HRANDFIELD`, `LPOS`, `TOUCH`, `GETBIT`, `BITCOUNT`, `BITPOS`,
`BITFIELD_RO`) and converging writes (`SETRANGE`, `UNLINK`, `PFCOUNT` — its
single-key form may refresh the HLL's cached cardinality, a converging
internal write; the multi-key form only reads its sources) are retry-safe,
as is the barrier probe `WAIT` (no server state change; connection-local
scope per the WAIT requirement; its count-style reply may differ across
attempts). The pop/move family (`LMPOP`,
`BLMPOP`, `ZMPOP`, `BZMPOP`, `LMOVE`, `BLMOVE`), `SET`, `GETDEL`, `GETEX`,
`COPY`, `HINCRBYFLOAT`, `SETBIT`, `BITOP` (destination write), `BITFIELD`
(worst-case form governs: `SET`/`INCRBY` write), `PFADD`, and `PFMERGE` are
not. Batch requests SHALL be idempotent iff every entry is idempotent.
`EVAL`/`FCALL` SHALL classify not idempotent unless the endpoint sets the
`idempotent` opt-in. For single-command dispatch the command is fixed by the
endpoint URI and SHALL never be overridable per exchange (ADR-0032); batch
entry selection is permitted only through the validated typed allowlist under
`command=multi`.

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

- **GIVEN** a transport error after `SMISMEMBER` (or any listed read,
  including `GETBIT`, `BITCOUNT`, `BITPOS`, `BITFIELD_RO`, `PFCOUNT`) was
  flushed
- **WHEN** the retry classifier evaluates the command
- **THEN** the command is classified idempotent and may be retried

#### Scenario: batch idempotence aggregates entries

- **GIVEN** a `multi` batch mixing `GET` (idempotent) and `GETDEL` (not)
- **WHEN** the retry classifier evaluates the batch
- **THEN** the batch is classified not idempotent and is not retried after an
  unconfirmed success

#### Scenario: WAIT is classified idempotent

- **GIVEN** a transport error after `WAIT` was flushed
- **WHEN** the retry classifier evaluates the command
- **THEN** the command is classified idempotent (barrier probe, no server
  state change) and may be retried
