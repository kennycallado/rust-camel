## ADDED Requirements

### Requirement: Redis state assertion

A `validate` action MAY carry
`target: {redis: {datasource: <name>, key: <key>, type: <t>, ttl: <bound>?}}`.
The `datasource` SHALL be a configured datasource name and the `key` SHALL
be a document-authored literal; neither is env-interpolated (the
identifier law). The `type` SHALL be one of `string`, `hash`, `list`,
`set`, or `zset`. The `expectation` SHALL reuse the SQL state assertion
grammar exactly: exactly one row shape (`rows` tuples through the shared
dual matcher grammar, or one row-count bound), an optional `columns`
list, `unordered: true` switching to multiset matching, and the same
load-time error catalog naming the action index and the offending field.
A `deadline` is valid on a redis target.

The type schema is inherent and defines the projection:

- `string` projects `[value]`, one row.
- `hash` projects `[field, value]`, one row per field.
- `list` projects `[index, value]`, one row per element.
- `set` projects `[member]`, one row per member.
- `zset` projects `[member, score]`, one row per member.

`columns`, when declared, SHALL be a non-empty subset of the declared
type's schema in any order; a name outside the schema is a load-time
`doc-validation` error naming the action index. Every `rows` row SHALL
carry exactly as many cells as the effective projection (the declared
`columns`, or the full schema when absent); a length mismatch is a
load-time `doc-validation` error naming the row index, whether or not
`columns` is declared. `ttl`, when declared, SHALL be a map carrying at
least one of `atLeast` and `atMost`, each a positive humantime duration
whose nanosecond value is an exact multiple of one millisecond; a
sub-millisecond duration is a load-time error, never silently truncated.
Each bound SHALL convert to milliseconds with a checked `u128` to `u64`
conversion that fails the load on overflow; when both are present,
`atLeast` SHALL NOT exceed `atMost` in milliseconds. An empty `ttl` map,
a zero or sub-millisecond bound, an overflowing bound, and an inverted
bound are load-time `doc-validation` errors naming the action index.
These load-time rules run in every build, whether or not the `redis`
feature is compiled.

Execution (feature `redis`) SHALL resolve the datasource through the
same `DatasourceCatalog` the booted composition root built (one
datasource name, one connection handle; the executor SHALL NOT construct
its own) through the shared steering resolver, with the family label
`redis validation`. The datasource `db_url` SHALL be a Redis client URL
(`redis://` or `rediss://`) parsed by the driver's own URL support, not
the component endpoint parser: the grammar is
`redis://[<username>][:<password>@]<host>[:<port>][/<db>]`, including an
ACL username and a database index. A `rediss://` URL in a build without
the component `tls` feature, a scheme outside `redis`/`rediss`, and any
URL the driver rejects SHALL fail closed as a config-class apparatus
error naming the datasource, with the URL redacted; sentinel and cluster
topologies are not datasource-URL grammar and SHALL be rejected.

Each snapshot SHALL be one atomic, read-only `EVAL` execution of a fixed
Lua script over the single key that issues `TYPE`, the type-matching read,
and `PTTL` in one server-side execution, so the observed type, payload,
and remaining TTL are a single coherent view — no `TYPE`/read/`PTTL` race
can split them. The script SHALL issue only read commands. The executor
SHALL dispatch in Rust on the declared type to select the script read
(`GET`, `HGETALL`, `LRANGE`, `SMEMBERS`, or `ZRANGE ... WITHSCORES`) and
SHALL treat the observed type as snapshot data: a snapshot SHALL succeed
carrying the observed type, the projected rows, and the remaining TTL,
and SHALL NOT fail because the key is missing or the observed type
differs from the declared type. Type agreement and the `ttl` bound are
judged only by the poll's final decision, so a key that is initially
missing and appears within the deadline, and a transiently wrong-typed
key that is corrected within the deadline, both recover and pass; a key
still missing or still mistyped at expiry fails as a verdict-class
validation mismatch. The only mid-window exit is the count-bound ceiling
breach. A malformed script reply (not the expected three-element array),
an unsupported observed type, a driver error, and a connection failure
SHALL stop the poll as apparatus-class errors.

When the observed type equals the declared type, the executor SHALL
project the payload into matcher tuples:

- `string`: one row `[value]`; a Nil reply is a null cell.
- `hash`: `[field, value]` per pair, ordered by field.
- `list`: `[index, value]` per element, index a number.
- `set`: `[member]` per member, ordered lexicographically.
- `zset`: `[member, score]` per member in rank order.

Hash and set SHALL be ordered at projection so an ordered `rows`
assertion is deterministic; list and zset keep their server order.

The value law is fail-closed. A Nil reply maps to null, an integer reply
to a number, and a simple or bulk string to a string when it is valid
UTF-8. A sorted-set score SHALL parse to a finite `f64` and normalize. A
non-integral value is emitted as a JSON float (`1.5`). An integral value
SHALL be emitted as a JSON integer only when it lies in `[-2^63, 2^63)` —
inclusive lower bound `-2^63`, exclusive upper bound `+2^63` — checked
BEFORE any integer conversion, with no saturating cast. An integral score
outside that range (`+2^63`, `1e20`, ...) SHALL fail closed as an
apparatus error. The document SHALL spell an integral score expectation
as an integer (`2`, never `2.0`): the expected cell uses the shared exact
`serde_json::Number` equality, which distinguishes an integer from a
float, and the family's integer normalization is what makes the Redis
lexical form `2` or `2.0` match the integer `2`. A non-finite score fails
closed. Any other reply kind, and a reply whose arity contradicts the
declared type, SHALL fail closed naming the schema column and, for a
container row, the row ordinal — never an actual member or field
identifier. An empty container yields zero rows.

`ttl`, when declared, SHALL be checked over the remaining time to live
reported by the same atomic snapshot (`PTTL`, milliseconds) through the
shared count-bound algebra: `atLeast` alone is an `AtLeast` bound,
`atMost` alone an `AtMost` bound, and both a `Range` bound over whole
milliseconds. The `PTTL` reply SHALL be interpreted as `-2` (the key is
missing), `-1` (the key is persistent), or `>= 0` (whole milliseconds
remaining); any other negative value fails closed. All legal `ttl` bounds
are positive, so a remaining value of `0` satisfies every legal `atMost`
bound, and fails every legal `atLeast` bound and every `Range` bound
(whose minimum is positive). A nonnegative remaining value is compared
with `bound_holds` after a checked `i64`/`u64` to `usize` conversion that
fails closed on overflow. A persistent key (`-1`) and a missing key (`-2`)
SHALL both fail a declared `ttl` bound with a cell-free mismatch. TTL is
evaluated on every snapshot, so a deadline decides it with the final
snapshot.

Poll semantics SHALL match the SQL state assertion: Redis key contents
are not monotone, so the action never settles early; without a deadline
one immediate snapshot decides; with a deadline the action polls at a
fixed interval, fails immediately on a snapshot above a count bound's
ceiling, and otherwise decides on the final snapshot. Without the
feature, a well-formed redis target SHALL fail at action time with a
named demand-gate error naming the `redis` feature; without a booted
catalog, the action SHALL fail closed.

Redaction and diagnostics (ADR-0051). A mismatch or fail-closed
projection SHALL carry only: the datasource name; the document-authored
key; the declared and observed type names; the rendered bound or the
expected and actual row counts; for a `ttl` bound, the observed remaining
TTL in whole milliseconds or the missing/persistent status; the schema
column names in projection order; and, for a fail-closed cell, the schema
column name and the row ordinal. It SHALL NOT contain the resolved
`db_url` (driver errors pass through the ADR-0051 sanitizer) nor any
actual value, member, or field identifier. The connection handle's lifecycle is drop-scoped, not
close-scoped: the boot's datasource catalog and the boot context's health
registry hold the only references, the factory's `close` is the default
no-op, and no shutdown step removes those references. The handle is
therefore released only when the boot-owned catalog and context are
dropped at boot scope end; a later boot over the same datasource alias
resolves a fresh handle through a new catalog. No shutdown step is
claimed to release it.

#### Scenario: redis target requires datasource key and type

- **GIVEN** a `validate` redis target missing its `datasource`, `key`,
  or `type` field
- **WHEN** the document loads
- **THEN** the load fails `doc-validation` naming the action index and
  the missing field

#### Scenario: unknown redis type is a load error

- **GIVEN** a `validate` redis target declaring `type: stream`
- **WHEN** the document loads
- **THEN** the load fails `doc-validation` naming the action index and
  the unrecognized type

#### Scenario: unknown projection column is a load error

- **GIVEN** a `validate` redis target declaring `type: hash` and
  `columns: [field, missing]`
- **WHEN** the document loads
- **THEN** the load fails `doc-validation` naming the action index and
  `missing`

#### Scenario: row length mismatch is a load error without columns

- **GIVEN** a `validate` redis target declaring `type: list` with no
  `columns` and a `rows` shape whose row carries three cells
- **WHEN** the document loads
- **THEN** the load fails `doc-validation` naming the row index (the
  list schema is two cells)

#### Scenario: empty ttl bound is a load error

- **GIVEN** a `validate` redis target whose `ttl` node carries neither
  `atLeast` nor `atMost`
- **WHEN** the document loads
- **THEN** the load fails `doc-validation` naming the action index

#### Scenario: inverted ttl bound is a load error

- **GIVEN** a `validate` redis target whose `ttl` declares
  `atLeast: 60s` and `atMost: 30s`
- **WHEN** the document loads
- **THEN** the load fails `doc-validation` naming the action index

#### Scenario: sub-millisecond ttl bound is a load error

- **GIVEN** a `validate` redis target whose `ttl` declares
  `atLeast: 500us`
- **WHEN** the document loads
- **THEN** the load fails `doc-validation` naming the action index — the
  bound is never silently truncated to zero

#### Scenario: zero ttl bound is a load error

- **GIVEN** a `validate` redis target whose `ttl` declares `atMost: 0s`
- **WHEN** the document loads
- **THEN** the load fails `doc-validation` naming the action index

#### Scenario: overflowing ttl bound is a load error

- **GIVEN** a `validate` redis target whose `ttl` bound has more whole
  milliseconds than fit `u64`
- **WHEN** the document loads
- **THEN** the load fails `doc-validation` naming the action index — the
  checked conversion never wraps

#### Scenario: deadline on a redis target is accepted

- **GIVEN** a `validate` action whose target is `redis` and whose node
  carries `deadline: 2s`
- **WHEN** the document loads
- **THEN** the load succeeds

#### Scenario: string projection reads one value row

- **GIVEN** a string key seeded with `alice` and a redis target declaring
  `type: string`
- **WHEN** the validate declares `rows: [["alice"]]`
- **THEN** the action passes

#### Scenario: hash projection reads field value rows

- **GIVEN** a hash key holding `name = alice` and `age = 42`
- **WHEN** the validate declares `type: hash`, `columns: [field, value]`,
  and `unordered: true` with both pairs
- **THEN** the action passes

#### Scenario: list projection reads index value rows

- **GIVEN** a list key holding `a`, then `b`
- **WHEN** the validate declares `type: list` and
  `rows: [[0, "a"], [1, "b"]]`
- **THEN** the action passes

#### Scenario: set projection reads member rows

- **GIVEN** a set key holding members `a`, `b`, and `c`
- **WHEN** the validate declares `type: set` and `unordered: true` with
  all three members
- **THEN** the action passes

#### Scenario: sorted set projection reads member score rows

- **GIVEN** a sorted set key holding `alice` at score `1.5` and `bob` at
  score `2`
- **WHEN** the validate declares `type: zset` and
  `rows: [["alice", 1.5], ["bob", 2]]`
- **THEN** the action passes

#### Scenario: columns select and reorder the projection

- **GIVEN** a hash key holding `name = alice` and `age = 42`
- **WHEN** the validate declares `type: hash`, `columns: [value]`, and
  `unordered: true` with rows `[["42"], ["alice"]]`
- **THEN** the action passes — only the value column is projected

#### Scenario: count bound passes on projected row count

- **GIVEN** a set key holding three members
- **WHEN** the validate declares `type: set` and
  `expectation: {atLeast: 2}` with no `rows`
- **THEN** the action passes

#### Scenario: wildcard ignore cell matches any value

- **GIVEN** a hash key holding `name = alice`
- **WHEN** the validate declares `type: hash`, `columns: [field, value]`,
  and `rows: [[{ignore: null}, {equals: "alice"}]]`
- **THEN** the action passes

#### Scenario: unsupported datasource URL is a config error

- **GIVEN** a redis datasource whose `db_url` is a sentinel topology URL
  or a malformed Redis client URL
- **WHEN** the executor resolves the connection handle
- **THEN** the action fails apparatus-class naming the datasource with the
  URL redacted, and constructs no connection

#### Scenario: rediss without the tls feature is rejected

- **GIVEN** a `rediss://` datasource `db_url` in a build without the
  component `tls` feature
- **WHEN** the executor resolves the connection handle
- **THEN** the action fails apparatus-class naming the datasource with the
  URL redacted

#### Scenario: database index and ACL username are honored

- **GIVEN** a datasource `db_url` of `redis://alice:secret@host:6379/1`
- **WHEN** the executor connects
- **THEN** the driver's own URL parser applies the ACL username `alice`,
  the password, and database `1` — the component endpoint parser, which
  would reject `/1` and discard the username, is not used

#### Scenario: a missing key is a validation mismatch at expiry

- **GIVEN** a redis target declaring `type: string` for a key that does
  not exist at the deadline
- **WHEN** the final snapshot decides
- **THEN** the action fails as a validation mismatch naming the key, the
  declared type, and the observed type `none`

#### Scenario: an initially missing key that appears within the deadline passes

- **GIVEN** a redis target declaring `type: string` for a key absent at
  the first snapshot and written by a route before the deadline
- **WHEN** the final snapshot decides
- **THEN** the action passes — the missing first snapshot was verdict
  data, not a snapshot error

#### Scenario: a transient wrong type corrected within the deadline passes

- **GIVEN** a redis target declaring `type: hash` for a key that is a
  string on the first snapshot and is rewritten as a hash before the
  deadline
- **WHEN** the final snapshot decides
- **THEN** the action passes — the transient type mismatch was verdict
  data, not a snapshot error

#### Scenario: wrong type at expiry is a validation mismatch

- **GIVEN** a key holding a string and a redis target declaring
  `type: hash`
- **WHEN** the final snapshot decides
- **THEN** the action fails as a validation mismatch naming the declared
  type `hash` and the observed type `string`, and the value is not
  projected

#### Scenario: a mutated key is observed as one coherent snapshot

- **GIVEN** a key whose value and TTL a route mutates between snapshots
- **WHEN** a snapshot runs
- **THEN** the observed type, payload, and TTL come from the same atomic
  script execution — no `TYPE`/read/`PTTL` split is observable

#### Scenario: a non-utf8 cell fails closed by column and ordinal

- **GIVEN** a bulk string reply whose bytes are not valid UTF-8
- **WHEN** the projection walks that cell
- **THEN** the action fails naming the schema column and the row ordinal
  — never the member or field identifier, and never a lossy replacement

#### Scenario: secret hash fields and set members never reach diagnostics

- **GIVEN** a hash whose field name is a sentinel secret and a set whose
  member is a sentinel secret, each under a failing assertion
- **WHEN** the action fails
- **THEN** the diagnostic contains neither sentinel — only schema column
  names and row ordinals

#### Scenario: an unsupported reply kind fails closed by column and ordinal

- **GIVEN** a reply kind outside the value law for the declared type
- **WHEN** the projection walks that reply
- **THEN** the action fails naming the schema column and the row ordinal —
  never a silent null

#### Scenario: a wrong-arity reply fails closed by column and ordinal

- **GIVEN** a hash reply with an odd number of field and value elements
- **WHEN** the projection walks that reply
- **THEN** the action fails naming the schema column and the row ordinal,
  with no actual member or field identifier

#### Scenario: integral sorted-set scores normalize to integers

- **GIVEN** a sorted set whose Redis score spelling is `2` or `2.0`
- **WHEN** the validate declares `type: zset` and `rows: [["alice", 2]]`
- **THEN** the action passes — the finite integral score is emitted as the
  JSON integer `2`

#### Scenario: non-integral sorted-set scores match as floats

- **GIVEN** a sorted set whose Redis score is `1.5`
- **WHEN** the validate declares `type: zset` and
  `rows: [["alice", 1.5]]`
- **THEN** the action passes

#### Scenario: the i64 lower bound score is accepted as an integer

- **GIVEN** a sorted set whose Redis score is `-9223372036854775808`
  (`-2^63`, the inclusive lower bound)
- **WHEN** the validate declares `type: zset` and
  `rows: [["alice", -9223372036854775808]]`
- **THEN** the action passes — the range is checked before conversion and
  the value is exactly representable

#### Scenario: an integral score at +2^63 fails closed as apparatus

- **GIVEN** a sorted set whose Redis score is `9223372036854775808`
  (`+2^63`, the exclusive upper bound)
- **WHEN** the projection walks that score
- **THEN** the action fails closed as an apparatus error, and no
  saturating cast to `i64` occurs

#### Scenario: a large integral score fails closed as apparatus

- **GIVEN** a sorted set whose Redis score is `1e20`
- **WHEN** the projection walks that score
- **THEN** the action fails closed as an apparatus error, never emitting a
  wrapped or saturated integer

#### Scenario: ttl atLeast passes within bound

- **GIVEN** a key with 60 seconds of remaining TTL and a redis target
  declaring `ttl: {atLeast: 30s}`
- **WHEN** the action executes
- **THEN** the action passes

#### Scenario: ttl atMost fails above bound

- **GIVEN** a key with 60 seconds of remaining TTL and a redis target
  declaring `ttl: {atMost: 30s}`
- **WHEN** the action executes
- **THEN** the action fails as a validation mismatch naming the bound and
  the observed TTL, and contains no value

#### Scenario: persistent key fails a declared ttl bound

- **GIVEN** a key with no expiry and a redis target declaring
  `ttl: {atLeast: 1s}`
- **WHEN** the action executes
- **THEN** the action fails as a validation mismatch — the key is
  persistent

#### Scenario: missing key fails a declared ttl bound

- **GIVEN** a redis target declaring a `ttl` bound for a key that does
  not exist
- **WHEN** the action executes
- **THEN** the action fails as a validation mismatch naming the key and
  the observed type `none`

#### Scenario: a zero remaining ttl satisfies atMost but not atLeast or Range

- **GIVEN** a key whose `PTTL` reply is `0` (it expires within the next
  millisecond)
- **WHEN** a redis target declares `ttl: {atLeast: 1ms}`
- **THEN** the action fails as a validation mismatch; with
  `ttl: {atMost: 1ms}` the same key passes, and with a `Range` such as
  `ttl: {atLeast: 1ms, atMost: 60s}` it also fails — no legal bound is
  zero

#### Scenario: ttl mismatch detail includes the observed TTL status

- **GIVEN** a failing `ttl` assertion, on a persistent key and on an
  expiring key
- **WHEN** the action fails
- **THEN** the detail names the datasource, the key, and the rendered
  bound, plus the observed remaining milliseconds (expiring) or the
  persistent/missing status, and contains no value or identifier

#### Scenario: an unknown negative PTTL fails closed

- **GIVEN** a malformed `PTTL` reply of `-3`
- **WHEN** the snapshot interprets it
- **THEN** the action fails closed as an apparatus error, and never
  treats it as a remaining TTL

#### Scenario: ttl is decided at the deadline's final snapshot

- **GIVEN** a key whose TTL passes the bound on an early snapshot and
  fails it before the deadline
- **WHEN** the deadline expires
- **THEN** the final snapshot decides and the action fails — TTL never
  settles early

#### Scenario: deadline poll passes when state settles in window

- **GIVEN** a key a running route writes within the deadline
- **WHEN** the validate declares a `deadline` and the rows the route
  will write
- **THEN** the final snapshot at expiry matches and the action passes

#### Scenario: no early settle — a matching snapshot is not proof

- **GIVEN** a key present at the first poll snapshot and deleted before
  the deadline, with a `rows` expectation and a `deadline`
- **WHEN** the deadline expires
- **THEN** the final snapshot decides and the action fails as a
  validation mismatch

#### Scenario: ceiling breach fails immediately

- **GIVEN** a set key with two members and `expectation: {atMost: 1}`
  with a deadline
- **WHEN** the first snapshot reads two members
- **THEN** the action fails immediately without waiting the deadline

#### Scenario: unknown datasource names the redis validation label

- **GIVEN** a redis target naming datasource `missing` and a boot whose
  catalog has no `missing`
- **WHEN** the action executes
- **THEN** the failure is apparatus-class and its message reads
  `redis validation: unknown datasource 'missing'`

#### Scenario: mismatch detail elides values and db_url

- **GIVEN** a failing redis assertion against a datasource whose
  `db_url` embeds a credential
- **WHEN** the action fails
- **THEN** the detail names the datasource, the key, the declared and
  observed types, the expected and actual row counts, and the schema
  column names in projection order, and contains neither the `db_url` nor
  any actual value, member, or field identifier

#### Scenario: driver error is sanitized

- **GIVEN** a redis read that fails at execution with a driver error
  carrying the resolved URL
- **WHEN** the action executes
- **THEN** the failure carries the datasource name, the key, and
  ADR-0051-sanitized driver error text, and never the resolved `db_url`

#### Scenario: feature off fails naming the gate

- **GIVEN** a document declaring a validate redis target, built without
  the `redis` feature
- **WHEN** the action runs
- **THEN** the action fails naming the `redis` feature

#### Scenario: no catalog fails closed

- **GIVEN** a validate redis action run through the single-action loop
  (no booted catalog)
- **WHEN** the action runs
- **THEN** the action fails closed naming the missing catalog

#### Scenario: single catalog invariant

- **GIVEN** a booted scenario with a configured redis datasource and a
  redis target
- **WHEN** the executor resolves the connection handle
- **THEN** the handle is the one the composition root's catalog built for
  that name — no second client or catalog is constructed

#### Scenario: redis connection handle is drop-scoped, not close-scoped

- **GIVEN** a first document boot that read a redis datasource, whose
  boot-owned catalog and context are dropped at boot scope end
- **WHEN** a second document boots the same datasource alias in the same
  process
- **THEN** the second boot resolves a fresh handle through a new catalog;
  the factory's no-op close did not release the first handle, and durable
  key state persists as the document author's clean-first responsibility

## MODIFIED Requirements

### Requirement: Uniform datasource steering across state families

Every state-family operation — a prepare action where the family has
one, and a validate target, in each state family — SHALL resolve its
datasource by name through the boot's datasource catalog in one uniform
way. A resolution failure SHALL name the operation's family label (`sql
action`, `sql validation`, `surreal action`, `surreal validation`,
`redis validation`) and the datasource name, and SHALL NOT contain the
datasource URL: every exact, nonempty occurrence of the `db_url` SHALL
be replaced by `[REDACTED]` before the failure is reported, while
existing driver detail SHALL be retained otherwise.

#### Scenario: unknown datasource in a prepare action names the label and the name

- **GIVEN** a `sql:` prepare action naming datasource `appdb` and a boot
  whose catalog has no `appdb`
- **WHEN** the action executes
- **THEN** the failure is apparatus-class and its message reads
  `sql action: unknown datasource 'appdb'`, with the same shape
  (`surreal action: ...`) for a `surreal:` prepare action

#### Scenario: unknown datasource in a validate target names the label and the name

- **GIVEN** a `validate` sql target naming datasource `appdb` and a boot
  whose catalog has no `appdb`
- **WHEN** the action executes
- **THEN** the failure is apparatus-class and its message reads
  `sql validation: unknown datasource 'appdb'`, with the same shape
  (`surreal validation: ...`) for a surreal target and
  (`redis validation: ...`) for a redis target

#### Scenario: a pool failure redacts the datasource URL

- **GIVEN** a state-family operation whose datasource URL carries a
  secret path component
- **WHEN** pool creation fails
- **THEN** the failure message names the label and the datasource, and
  every occurrence of the `db_url` reads `[REDACTED]`

#### Scenario: a handle downcast failure keeps its driver detail

- **GIVEN** a state-family operation whose resolved pool handle is not
  the handle type the family expects
- **WHEN** the handle is downcast
- **THEN** the failure message names the label and the datasource,
  retains the downcast driver detail, and contains no datasource URL

### Requirement: Deadline poll driver contract

Every validate target that polls under a deadline — partner, sql,
surreal, and redis — SHALL run one shared poll discipline: without a
deadline, one immediate snapshot SHALL decide; with a deadline, the
harness SHALL fix the expiry instant before the first snapshot and poll
snapshots until it, where a snapshot error SHALL stop the poll at once,
a family-supplied early judgment MAY stop the poll before the expiry
instant (an in-flight snapshot is never cancelled by the deadline; its
early judgment still precedes the expiry decision), the snapshot at
expiry SHALL decide otherwise, and the sleep between snapshots SHALL
never exceed the remaining window. Per-family poll semantics (which
judgments settle early, which wait the window) SHALL stay owned by the
family requirements; this contract governs only the shared discipline.

#### Scenario: no deadline decides on one immediate snapshot

- **GIVEN** a `validate` action with a count bound and no `deadline`
- **WHEN** the action executes against a partner whose recorder already
  holds a satisfying count
- **THEN** the action decides on that single snapshot without polling

#### Scenario: an early judgment stops the poll before the deadline

- **GIVEN** a partner `validate` action with `deadline: 5s` and an
  `atLeast` count bound
- **WHEN** the filtered count reaches the bound while retries are still
  in flight
- **THEN** the action passes without waiting the full deadline

#### Scenario: the snapshot at expiry decides an absence claim

- **GIVEN** a partner `validate` action with `deadline: 2s` and an
  `atMost` bound the mid-window snapshots satisfy
- **WHEN** the deadline elapses
- **THEN** the action decides on the final snapshot, not on an earlier
  passing one

#### Scenario: a snapshot error stops the poll at once

- **GIVEN** a sql `validate` action with a deadline whose datasource
  query fails on the first snapshot
- **WHEN** the poll takes its first snapshot
- **THEN** the action fails apparatus-class immediately, without
  further snapshots or sleeps

#### Scenario: a redis target polls through the shared driver

- **GIVEN** a redis `validate` action with `deadline: 2s` whose key a
  route deletes mid-window after a matching snapshot
- **WHEN** the deadline elapses
- **THEN** the action decides on the final snapshot and the shared
  driver takes no early-settle shortcut

### Requirement: Demand-gated activation and CI isolation

The system SHALL gate adapter activation behind Cargo features — `http`
for v1, `sql` for the sql state family, `surreal` for the surreal state
family, `redis` for the redis state family — SHALL reserve
`testcontainer` and `user-provided` partner provisioning values as
grammar that the v1 runner rejects as unsupported, and SHALL keep the
default test suite unchanged in runtime and composition. The
`integration-http` and `integration-sql` CI jobs SHALL run their
loopback scenarios on relevant pull request paths, an
`integration-surreal` CI job SHALL prove the surreal feature stands
alone (building `camel-cli` without `integration-http` and
`integration-sql`), and an `integration-redis` CI job SHALL run the
redis state battery against a provisioned Redis container; loopback
scenarios SHALL carry no `#[ignore]` marker.

#### Scenario: http scenarios isolated behind their feature

- **GIVEN** a workspace built without the `http` adapter feature
- **WHEN** the integration-tier tests compile
- **THEN** no HTTP partner code is compiled in and no http scenario
  runs

#### Scenario: surreal state isolated behind its feature

- **GIVEN** a workspace built without the `surreal` state feature
- **WHEN** the integration-tier tests compile
- **THEN** no surreal executor code is compiled in and no surreal
  scenario runs; a document declaring `surreal:` fails at load
  naming the `surreal` feature, and a surreal validate target that
  passes load fails at action time naming the `surreal` feature

#### Scenario: redis state isolated behind its feature

- **GIVEN** a workspace built without the `redis` state feature
- **WHEN** the integration-tier tests compile
- **THEN** no redis executor code is compiled in and no redis
  scenario runs; a redis validate target that passes load fails at
  action time naming the `redis` feature

#### Scenario: reserved provisioning value rejected

- **GIVEN** a scenario endpoint declaring the `testcontainer` or
  `user-provided` provisioning source
- **WHEN** the document loads in v1
- **THEN** the run reports the source as unsupported and exits 2

#### Scenario: default suite untouched

- **GIVEN** the default test suite before and after this change
- **WHEN** both run in CI
- **THEN** their runtime and executed set are identical, and only the
  opt-in `integration-http`, `integration-sql`, `integration-surreal`,
  and `integration-redis` jobs exercise scenario documents

### Requirement: Scenario datasource teardown

Each scenario boot owns its `DatasourceCatalog`. The boot teardown
SHALL close the SQL datasource pools the boot opened, after the
context stops: a close timeout warns and does not fail the shutdown,
while a close error fails it, matching the bridge-pool teardown
semantics (providers without an explicit close keep their default
no-op). The same teardown SHALL release the surreal clients the boot
opened. The surrealdb SDK has no explicit client close
(`invalidate()` revokes authentication; it is not a process-level
shutdown call), so the release contract is: the factory's close hook
SHALL invalidate the boot's surreal clients (revoking any session
credentials — on the auth-free embedded `mem://` tier this is
hygiene; on remote tiers it terminates the session), and the boot's
references die with the boot's scope. The teardown SHALL NOT be claimed
to release the redis connection handles: the redis pool factory keeps
the default no-op close, and the catalog's `close_all` neither removes
the catalog's cached handle nor the boot context's health-registry
reference. A redis handle is released only when the boot-owned catalog
and context are dropped at boot scope end (drop-scoped, not
close-scoped); no shutdown step removes those references. Each boot owns
a fresh `DatasourceCatalog`, so a later boot over the same alias resolves
through a new catalog to a new client or connection. With the pools
closed, a SQLite in-memory database dies with
its boot: a later document booting the same `[datasources]` alias in
the same process SHALL start from an empty database, including named
shared-memory URIs (`file:<name>?mode=memory&cache=shared`), where a
lingering connection would otherwise carry rows into the later boot.
An embedded `mem://` SurrealDB datasource SHALL die with its boot the
same way, by construction plus invalidation: every
`connect("mem://")` builds a fresh isolated embedded instance, the
factory builds one client per datasource name, and the boot's
teardown invalidates it, so a later boot over the same alias
constructs a new empty instance. File-backed (and other durable)
datasources — including remote `ws://`/`http://` SurrealDB instances
and every Redis instance — are outside this guarantee: their state
persists across boots, and isolation is the document author's
responsibility through the prepare-action clean-first idiom (a
`DELETE FROM` or table-recreating statement as the first `sql:`
prepare statement, a `REMOVE TABLE` or `DELETE` statement as the first
`surrealdb` prepare statement, or a clean-first route step for Redis),
mirroring ADR-0069 section 9 — user-provided durable infrastructure is
never the harness's hermeticity contract.

The boot freshness regression verifies this requirement with a
test-unique named URI. The implementation must preserve its current
policy: close errors fail shutdown, while a close timeout warns
without failing shutdown, unless a separately reviewed change
explicitly amends that policy. The named shared-cache memory URI
remains the adopted sequential-boot convention; the per-test name is
fixture hardening, while concurrent boots sharing one alias remain
outside the v1 contract.

#### Scenario: All registered pools are closed before return

- **GIVEN** a scenario has initialized one or more datasource pools
- **WHEN** its boot shutdown invokes datasource teardown
- **THEN** the close future is awaited before the next boot is
  created, and the existing close-error and timeout policy is
  preserved

#### Scenario: a second boot over the same memory alias starts empty

- **GIVEN** a first document boot whose `sql:` prepare seeded rows
  over a `sqlite::memory:?cache=shared` alias, and that boot
  completed teardown
- **WHEN** a second document boots over the same alias in the same
  process
- **THEN** the second boot's reads see zero of the first boot's rows —
  the in-memory database died with the first boot's pools

#### Scenario: a named shared memory URI dies with its boot

- **GIVEN** a first document boot seeding rows over a named
  shared-memory URI (`sqlite:file:<name>?mode=memory&cache=shared`),
  whose connections share one named database, and that boot completed
  teardown
- **WHEN** a second document boots over the same URI in the same
  process
- **THEN** the second boot's reads see zero of the first boot's rows

#### Scenario: a second boot over a mem surreal datasource starts empty

- **GIVEN** a first document boot whose `surreal:` prepare seeded
  records over a `mem://` datasource, and that boot completed
  teardown
- **WHEN** a second document boots over the same alias in the same
  process
- **THEN** the second boot's reads see zero of the first boot's
  records — the first boot's client was invalidated at teardown, the
  embedded database died with the first boot, and the second boot
  resolves through a fresh catalog and a new client

#### Scenario: a second boot over a redis datasource gets a fresh handle

- **GIVEN** a first document boot that read a redis datasource, whose
  boot-owned catalog and context are dropped at boot scope end after
  teardown
- **WHEN** a second document boots over the same alias in the same
  process
- **THEN** the second boot resolves a fresh connection handle through a
  new catalog, while durable key state persists and is the document
  author's clean-first responsibility — the no-op close did not release
  the first handle

#### Scenario: shutdown closes the datasource pools

- **GIVEN** a booted scenario whose document opened a SQL datasource
  pool through a `sql:` action or a sql `validate` target, a surreal
  client through a `surreal:` action or a surreal `validate` target,
  or a redis connection handle through a redis `validate` target
- **WHEN** the boot-owning caller runs the boot teardown
- **THEN** the pools that boot opened are closed, the boot's
  surreal clients' close hook ran (invalidation issued) before the
  teardown returns, the redis factory's no-op close ran without
  releasing the handle, and the teardown returns without close errors

#### Scenario: file-backed state persists across boots

- **GIVEN** a first document boot seeding rows into a file-backed
  sqlite datasource, and that boot completed teardown
- **WHEN** a second document boots over the same file-backed alias
- **THEN** the second boot observes the first boot's rows, and the
  second document's prepare is responsible for cleaning them
  (clean-first idiom)
