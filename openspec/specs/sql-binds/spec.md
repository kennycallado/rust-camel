# sql-binds Specification

## Purpose
TBD - created by archiving change sqlbinds. Update Purpose after archive.
## Requirements
### Requirement: Closed wire-family eligibility matrix

The system SHALL decide described-mode eligibility per statement from a
closed matrix of described parameter kinds against payload wire families
(`Bool`, `Numeric`, `Text`, `Null` — the families the default JSON binder can
put on the wire). A described numeric kind (SMALLINT, INTEGER, BIGINT, REAL,
DOUBLE PRECISION) is eligible with any payload; at bind time the payload is
normalized to the exact wire encoding of the described kind, and a
non-numeric payload (bool, string, array, object) at that position SHALL fail
with a typed error. A described BOOL position is eligible only with `Bool` or
`Null` payloads; described TEXT and BLOB positions are eligible only with
`Text` or `Null` payloads; a described Null (unmapped) kind is eligible only
with a `Null` payload. Any statement carrying at least one ineligible
position is ineligible as a whole and SHALL take the hybrid fallback, where
every payload binds under the encoding of the normalized arguments (alias
OIDs follow the bound values) and the server either casts
it faithfully or fails loudly — measured on postgres:17/sqlx 0.9.0: an int8
payload into a TEXT column stores the correctly-cast decimal text, a bool
stores `"true"`, a bare `SELECT $1` reads back the exact integer, and only
an explicit `::text` cast feeding a BIGINT column fails loudly (PG 42804).
The ASCII-byte read-back `"12345678"` and the wire-byte `"\u0001"` are
impossible because no described-TEXT statement metadata ever normalizes a
numeric or bool payload's bytes. A kind
outside the closed set (e.g. NUMERIC) SHALL make the description unusable;
the FIRST batch shape's probe selects the batch scan mode (today's
semantics): an unusable first probe keeps the legacy message-wide scan for
the whole batch, while a later unusable shape contributes ineligible items
that send the batch to the alias WITHOUT erasing earlier shapes' described
numeric guards (a kept BIGINT target from a later shape's usable probe
SHALL never replace the message-wide scan's F64 guard at a shared
position, and a later NUMERIC shape SHALL never turn an earlier shape's
typed 2^53 rejection into a silent I64 bind).

| Described kind | Bool payload | Numeric payload | Text payload | NULL payload |
|---|---|---|---|---|
| SMALLINT / INTEGER / BIGINT | typed error at bind | normalized to exact encoding | typed error at bind | typed NULL |
| REAL / DOUBLE PRECISION | typed error at bind | checked conversion (2^53, f32 overflow/underflow, ties-to-even ranges) | typed error at bind | typed NULL |
| BOOL | eligible | hybrid alias | hybrid alias | eligible |
| TEXT | hybrid alias | hybrid alias | eligible | eligible |
| BLOB | hybrid alias | hybrid alias | eligible | eligible |
| Null (unmapped) | hybrid alias | hybrid alias | hybrid alias | eligible |

#### Scenario: every matrix cell is decided

- **GIVEN** each described kind in the closed set (SMALLINT, INTEGER, BIGINT,
  REAL, DOUBLE PRECISION, BOOL, TEXT, BLOB, Null) paired with each payload
  wire family (Bool, Numeric, Text, Null)
- **WHEN** the planner evaluates described-mode eligibility
- **THEN** every kind × family pair is decided by the matrix table above and
  no pair falls through to an implicit default

#### Scenario: described TEXT position with a JSON number never reinterprets bytes

- **GIVEN** a statement whose bare `SELECT $1` parameter postgres:17
  describes as TEXT, with payload `3544952156018063160`
- **WHEN** the statement executes through a path that returns rows (the
  planner-routed single/consumer read-back)
- **THEN** the statement is ineligible, runs on the unnamed alias, and the
  read-back is the exact integer — the ASCII-byte read-back `"12345678"`
  (the int8 wire bytes of that integer ARE those ASCII digits) is
  impossible

#### Scenario: a JSON number into a TEXT column stores the correctly cast text

- **GIVEN** a batched `INSERT` into a TEXT column whose parameter postgres
  describes as TEXT, with payload `3544952156018063160`
- **WHEN** the batch executes on the unnamed alias
- **THEN** the stored value is the correctly cast decimal text
  `"3544952156018063160"` (measured on postgres:17/sqlx 0.9.0) — never the
  bytes-as-text corruption

#### Scenario: bool payload at a described TEXT position never reinterprets bytes

- **GIVEN** a statement whose parameter position postgres describes as TEXT,
  carrying a bool payload — the corruption shape of rc-7e681, where the
  bool's one wire byte bound under the TEXT OID read back as `"\u0001"`
- **WHEN** the statement executes
- **THEN** the statement is ineligible and runs on the unnamed alias with the
  bool bound under its own BOOL OID, so the read-back is a bool or a
  server-cast value and never the reinterpreted byte `"\u0001"`

#### Scenario: incompatible payload at a described BOOL position never reinterprets bytes

- **GIVEN** a statement whose parameter position postgres describes as BOOL,
  carrying a numeric or string payload
- **WHEN** the statement executes
- **THEN** the statement is ineligible and takes the hybrid alias, where the
  payload binds under its own OID and the server casts it faithfully or fails
  loudly — the payload's wire bytes are never bound under the BOOL encoding

### Requirement: Hybrid fallback preserves described numeric targets per SQL shape

The system SHALL build fallback targets by keeping every described numeric
bind target and resolving only the positions without one from the message-wide
scan across all of the message's binding rows (float wins per position).
IN-clause expansion changes parameter positions per item, so described
targets SHALL be tracked per resolved SQL shape — the described numeric
targets of one shape SHALL never be applied to another shape's positions —
while the scan fallback stays message-wide so a float anywhere in the
message keeps winning its position for every shape (a per-shape scan would
loosen today's typed 2^53 guard into a silent server-side round).
An integer payload whose magnitude exceeds 2^53 at a kept DOUBLE PRECISION
or REAL target SHALL fail with the typed 2^53 error; a kept target SHALL
never be dropped in favour of a scanned target. Positions without a
described numeric target keep the legacy scan behaviour, which applies the
existing checked conversions (ties-to-even rounding for float to integer,
f32 rounding with overflow/underflow checks).

#### Scenario: probe-failure batch keeps the described float guard

- **GIVEN** a batch whose probe describes a DOUBLE PRECISION position but
  whose per-item describe read-back yields no usable parameter types, with
  payload `9007199254740993` at that position
- **WHEN** the batch binds
- **THEN** it fails with the typed 2^53 error
  (`integer value 9007199254740993 exceeds 2^53 and would lose precision
  bound as f64`) and never binds a cast-rounded value

#### Scenario: partially unresolved batch applies the existing checked conversions

- **GIVEN** a batch where only some positions resolve to described numeric
  targets and the rest are unresolved, with payloads representable in every
  position's resolved encoding
- **WHEN** the batch executes on the unnamed alias
- **THEN** every value binds through the existing checked-conversion
  semantics — exact round-trip where the value is exactly representable,
  existing typed rejection where it is not — and unresolved positions bind
  through the scanned target or the default path

#### Scenario: a message-wide float keeps every shape's 2^53 guard

- **GIVEN** a batch whose items resolve to different NUMERIC-probe shapes
  (no usable description) where one shape carries a float at a position and
  another shape carries an integer above 2^53 at the same position index
- **WHEN** the batch plans alias targets
- **THEN** the message-wide scan gives that position the F64 target for
  every shape, and the integer fails with the typed 2^53 error — it is
  never bound as I64 to round silently server-side

#### Scenario: variable-length IN with a later float parameter keeps its guard

- **GIVEN** a batch whose items resolve to different SQL shapes because an
  IN list expands to different placeholder counts, where a DOUBLE PRECISION
  parameter sits after the expanded list in one shape
- **WHEN** the batch plans binds
- **THEN** the later shape's DOUBLE target is tracked for that shape's
  positions, an integer above 2^53 at that position fails with the typed 2^53
  error, and shapes that were accepted before keep their accepted status

#### Scenario: a read-back that disagrees with the probe executes on the alias

- **GIVEN** a statement whose probe describes a DOUBLE PRECISION position,
  whose named statement-cache entry was seeded with bind-derived OIDs (a
  raw int8 bind at that position, as a pre-upgrade writer leaves behind), and
  a payload integer above 2^53 at that position
- **WHEN** the statement plans binds on the same pool with no recycle
- **THEN** the read-back disagrees with the probe, the planner treats the
  statement as ineligible, and it executes on the unnamed alias where the
  probe's DOUBLE target is kept — the payload fails with the typed 2^53
  error at bind time and no f64 bytes ever enter the int8-parameter cached
  statement

### Requirement: Batch binds go through the planner

The system SHALL plan batch binds once per message with the shared planner:
eligibility is evaluated across every item's bindings against the described
kinds of the item's own resolved SQL shape, and one ineligible item sends the
whole batch to the unnamed alias namespace
(`/* batch-bind-kinds-fallback */`, executed with `persistent(false)`).
Eligible homogeneous batches SHALL keep today's described persistent path,
including the whole batch transaction with rollback on failure. A probe that
yields no usable description as the FIRST batch shape's probe (AnyDriverError
or an empty type list, e.g. NUMERIC columns) SHALL keep the legacy
message-wide scan behaviour for the whole batch — scan guards, not described
guards; success is not guaranteed. Server and connection errors still fail
the message with zero commits: because every shape is probed before any item
executes, a later shape's prepare-time server error surfaces at probe time
through the `Batch query prepare failed` wrapper (pre-change code surfaced
the same failure at that item's execute as `Batch query execution failed`). A later shape's unusable probe contributes ineligible items
without erasing earlier shapes' described guards. A probe whose server or connection fails (errors
other than the driver-metadata error) SHALL propagate unchanged.

#### Scenario: homogeneous eligible batches stay byte-identical

- **GIVEN** the pinned batch suites `batch_homogeneous_kinds_unchanged`,
  `batch_mixed_int_float_kinds_never_corrupts`, and
  `batch_numeric_column_fallback_works_twice`
- **WHEN** they run unchanged against the planner-routed batch
- **THEN** all three stay green with identical assertions

#### Scenario: one ineligible item sends the whole batch to the alias and commits nothing

- **GIVEN** a batch whose first item is fully eligible and a later item
  carries an ineligible payload (a number at a described TEXT position), with
  `9007199254740993` at a described DOUBLE PRECISION position and an INSERT
  that would commit rows
- **WHEN** the batch executes
- **THEN** the whole batch runs on the unnamed alias, the later item fails
  with the pinned typed 2^53 error, and zero rows from the batch are
  committed (the transaction rolls back)

#### Scenario: probe without a usable description keeps the legacy scan

- **GIVEN** a batch whose statement contains an unmapped kind (NUMERIC) so
  the probe returns no usable parameter types
- **WHEN** the batch executes
- **THEN** the batch binds with the legacy message-wide scan on the unnamed
  alias, exactly as before this change

### Requirement: Consumer binds go through the planner

The system SHALL route all three consumer bind shapes — the poll query, the
stream poll, and the post-query — through the shared bind planner, planning
and binding on one pooled connection per execution, normalizing against the
read-back parameter OIDs of the statement that actually executes. No consumer
site SHALL bind raw JSON on a persistent pooled query. This is prevention:
migrated writers seed no raw-bind entries under real SQL text on fresh
connections. Repair of entries seeded by a pre-upgrade binary is out of
scope; the release notes SHALL state that an upgrade restart clears sqlx
statement caches and that PgBouncer session mode must keep
`server_reset_query=DISCARD ALL` (the default).

#### Scenario: poll query binds through the planner

- **GIVEN** a consumer poll statement
- **WHEN** the poll executes
- **THEN** the binds — parameterized or the empty-bindings fast path — are
  planned (described mode when eligible, unnamed alias otherwise) and no raw
  persistent statement is prepared under the poll SQL text

#### Scenario: stream poll binds through the planner

- **GIVEN** a stream-list consumer poll statement
- **WHEN** the stream executes
- **THEN** the stream plans the binds on the connection it fetched from and
  binds per that plan, exactly like the producer StreamList path

#### Scenario: post-query binds through the planner

- **GIVEN** a consumer on-consume post-query writing parameterized values
- **WHEN** the post-query executes
- **THEN** the binds are planned and bound per the described or scanned
  targets on one pooled connection, never as raw JSON on a pooled query

#### Scenario: a consumer-seeded entry cannot seed a silent writer

- **GIVEN** a consumer post-query writes a JSON number at a float position on
  a pooled connection, and a batch afterwards executes an integer above 2^53
  at that position
- **WHEN** the batch runs on the same pool, with no pool recycle in between
- **THEN** the batch fails with the typed 2^53 error and never stores or
  returns a rounded value

#### Scenario: upgrade caveat is documented

- **GIVEN** the release notes for the version carrying this change
- **WHEN** they describe the bind-planner fix
- **THEN** they state that restarting on upgrade clears poisoned statement
  caches and that PgBouncer session mode must keep
  `server_reset_query=DISCARD ALL`

### Requirement: No raw default binder outside the planner

The system SHALL restrict the default JSON binder (`bind_json_values`) to
test code (`#[cfg(test)]`) so it has no production caller. The planner's
default arm binds through the single-value helper; any future fourth bind
path that tries to call the raw binder SHALL fail to compile.

#### Scenario: the raw binder has no production caller

- **GIVEN** the crate sources on the main branch of this change
- **WHEN** the code is inspected or compiled outside `#[cfg(test)]`
- **THEN** `bind_json_values` is unreachable: no non-test module references
  it, and a new caller outside test code does not compile

### Requirement: Failures are typed errors, never reinterpreted bytes

The system SHALL fail binds with typed errors that name the batch item (for
batch mode), the parameter position, and the reason, reusing today's pinned
wording byte-for-byte (`Batch item N, parameter P: …` / `parameter P: …`).
Normalization errors are distinguishable from server failures by their
existing error wrappers, unchanged. A payload that cannot be bound faithfully
under a position's encoding SHALL never be bound as reinterpreted bytes:
unsigned integers beyond i64::MAX, integers beyond the 2^53 limit at float
targets, floats outside an integer target's range, and f32
overflow/underflow all fail with their existing typed errors.

#### Scenario: batch wording stays byte-exact

- **GIVEN** a payload that trips the 2^53 guard at batch item 1, parameter 0
- **WHEN** the typed error is produced
- **THEN** the message is byte-identical to today's
  `Batch item 1, parameter 0: integer value 9007199254740993 exceeds 2^53
  and would lose precision bound as f64`

#### Scenario: single and consumer wording names only the position

- **GIVEN** a payload that trips the 2^53 guard on a single or consumer
  statement at parameter 0
- **WHEN** the typed error is produced
- **THEN** the message is byte-identical to today's
  `parameter 0: integer value 9007199254740993 exceeds 2^53 and would lose
  precision bound as f64`

