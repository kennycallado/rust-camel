# sql-batch-bind-kinds

Revision 2 — first expert gate rejected revision 1 for underspecified wire
encodings, cache-handling, IN-clause shapes, and rounding semantics. All five
findings are addressed below; none requires a public API or config change.

## Why

Batch SQL inserts silently corrupt numeric data. bd `rc-fef4z`, P0.

Endpoint `sql:INSERT INTO t (f,i) VALUES (#,#)?batch=true` with table
`t (f DOUBLE PRECISION, i INT)`:

- body `[[10.5,1],[20,2],[3,3]]` stored `f = 10.5, 1e-322, 1.5e-323`
- body `[[5,4],[6.5,5]]` stored `f = 5, 4.619004367821865e+18`

No error; the transaction commits. Single (non-batch) inserts looked fine.

## Confirmed mechanism

`execute_batch` runs one `sqlx` execute per item inside one transaction; the
SQL text is identical for every item. `bind_json_values` binds a JSON number
as `i64` when it fits, else `f64`, so the same parameter position carries
different Rust bind kinds across items.

sqlx-postgres 0.9.0 (registry source):

- The statement cache is keyed by SQL text only:
  `connection/executor.rs` `get_or_prepare` returns the cached statement
  with `cache_statement.get_mut(sql)`. This lookup runs before the
  `persistent` flag is consulted, so `persistent(false)` cannot bypass a
  pre-existing entry.
- On a cache miss, the `Parse` message carries the current execution's
  bind-derived parameter OIDs (`&arguments.types`). The first item decides
  the OIDs for the whole batch.
- Later items bind payloads encoded per their own Rust kind, but the server
  decodes them under the statement's declared OIDs. `int8` and `float8`
  share the 8-byte binary width, so an `i64` payload is silently reinterpreted
  as IEEE-754 bits and vice versa.

Evidence: the repo harness (testcontainers Postgres, `camel-test/tests/sql_test.rs`)
reproduces the first body bit-for-bit on the unfixed code: stored
`f = [10.5, 1e-322, 1.5e-323]` with no error. The external reporter
reproduced both bodies on PostgreSQL 17.11 (`postgres:17`). The regression
tests pin `postgres:17`.

## Chosen fix (owner steer 2026-11-08)

JSON cannot distinguish `20` from `20.0`, and JS producers emit whole floats
as ints, so `[[10.5],[20]]` is the normal case. Rejecting mixed kinds is not
acceptable. The fix makes every parameter position carry exactly one wire
encoding per statement, so payloads can never be decoded under a mismatched
OID.

### Per-position target kinds and exact wire encodings

Each described numeric parameter type maps to one Rust bind kind with a
checked value conversion:

| Described type (AnyTypeInfoKind) | Bind encoding | Value conversion |
|---|---|---|
| `SmallInt` | `i16` | round to `i64`, range-check to `i16` |
| `Integer` | `i32` | round to `i64`, range-check to `i32` |
| `BigInt` | `i64` | round floats, range-check to `i64` |
| `Real` | `f32` | numeric to `f64` (integer guard below), then `f32` |
| `Double` | `f64` | numeric to `f64`, integer precision guard below |

Broad `i64`/`f64` categories are not used, because a `i64` payload under a
described `int4`/`int2` OID fails server-side width checks and would
regress today's working homogeneous batches.

Conversion rules:

- **Int targets (`i16`/`i32`/`i64`).** Float values round with
  `f64::round_ties_even` — ties-to-even, the same rule as Postgres's
  `rint` in its `float8` -> integer casts (`2.5 -> 2`, `3.5 -> 4`,
  `-2.5 -> -2`) — then range-check; out-of-range fails with a typed
  error. Values stay integers end to end; `f32`/`f64` payloads never reach
  an integer OID.
- **Float targets (`f32`/`f64`).** Integer values convert to `f64` first.
  An integer with magnitude greater than 2^53 fails with a typed error.
  The threshold is deliberately conservative: it guarantees exactness for
  `Double` positions, and for `Real` positions integers beyond 2^24 still
  round to the nearest `f32` — the same rounding the server applies in its
  `float8` -> `float4` cast — but the owner-specified 2^53 line keeps the
  error rule simple and predictable. The `f64` result then converts to
  `f32` for `Real` targets with explicit overflow/underflow checks: a
  finite value that would become infinite as `f32`, or a non-zero value
  that would underflow to zero, fails with a typed error (Postgres `dtof`
  rejects both; a bare Rust cast would silently store different data).
- **Unsigned JSON integers beyond `i64::MAX`.** Detected through the
  `u64` magnitude before any `f64` conversion; they fail with a typed
  error at every numeric target.
- **Non-numeric JSON values at described numeric positions.** String,
  bool, array and object values fail with a typed error naming the batch
  index, position and kind. Raw text or bool payloads must never reach a
  numeric OID: an 8-character string would silently reinterpret as
  `float8` bits, exactly the corruption this change removes. (This does
  not affect mixed numeric kinds, which normalize.) Single-mode still
  accepts server-side `text` -> numeric coercion; batch mode now requires
  numeric JSON at numeric positions.
- **`null`.** Binds the typed `Option` of the target encoding.

### Statement and cache handling

1. **Described path (normal).** Per batch item, the producer calls
   `Executor::prepare_with(sql, &[])` on the transaction connection before
   binding. The backend prepares with empty `Parse` parameter types so the
   server infers OIDs from SQL text and catalog, and returns
   `AnyStatement.parameters`. The prepared statement is cached under its
   SQL text and reused by later items and messages. Any-mapped parameter
   kinds are described-eligible: numeric kinds bind through their exact
   wire encoding, non-numeric kinds (text, bool, blob) keep the default
   JSON binding under their described OIDs.

   Correctness does not depend on fresh server inference: on a cache hit,
   the read-back parameters are the cached statement's actual OIDs, and
   normalization targets those OIDs, so every payload matches the statement
   that executes it. This also covers warm pools carrying entries prepared
   before a schema change or by an older code version: whatever OIDs the
   cached statement declares, the binds are normalized to them.

2. **Fallback path (metadata unavailable only).** If the probe of the
   described types fails with `sqlx::Error::AnyDriverError` (an unmapped
   parameter type such as `NUMERIC`), or returns `Right(count)` or an
   empty parameter-type list, the batch items execute as uncached
   statements on a semantically identical alias text
   (`<sql> /* batch-bind-kinds-fallback */`) with `persistent(false)`.
   sqlx consults its SQL-text statement cache before the persistence
   flag, so executing the original text could still reuse an entry whose
   OIDs cannot take normalized payloads (the probe, and any earlier
   single-mode message, may have left one). The alias text lives in its
   own cache namespace that nothing else uses, and `persistent(false)`
   keeps the batch's statements out of it — every item prepares an
   unnamed statement whose parameter types derive from the
   scan-normalized binds. The probe itself also runs on its own alias
   text (`/* batch-bind-kinds-probe */`), so its entries never collide
   with the user's SQL text. Server and connection failures from the
   probe propagate unchanged — only metadata unavailability selects the
   fallback.

3. **Per-batch scan (fallback normalization).** A position binds as the
   float encoding once any row carries a float JSON number at that
   position; otherwise the integer encoding. One kind per position per
   message; kinds are never mixed within an item's statement.

4. **IN-clause templates.** `:#in:` parameters resolve by name against the
   exchange, not from the positional batch array, so IN-clause expansion
   combined with `batch=true` is not a supported combination today: item
   resolution fails with the existing typed error before any execution.
   The per-item described prepare (1) keeps the design correct for any
   future per-item SQL shape, because each distinct SQL text is described
   and normalized independently (cached per text, so uniform batches pay
   one described prepare).

### Typed-error transport

Normalization failures surface as `CamelError::ProcessorError` — the
component's existing error variant — naming the 0-based batch index, the
0-based parameter position, the offending value, and the target encoding
(e.g. "Batch item 1, parameter 0: integer value 9007199254740993 exceeds
2^53 and would lose precision bound as f64"). The error handler / DLC
transports it; nothing is written for a failed item.

## Rejected alternatives

- **Reject heterogeneous kinds per parameter position.** Rejected by the
  owner: JSON number parsing erases the int/float distinction for whole
  values, so legitimate traffic would fail. It also leaves the cross-message
  hole open (an all-int batch reusing a statement cached by an earlier
  all-float batch on the same pooled connection corrupts identically).
- **Per-item `persistent(false)` everywhere.** Does not actually isolate:
  sqlx consults the SQL-text cache before persistence, so a pre-existing
  entry is reused anyway. Correct only after a cache clear; kept as part of
  the fallback path (2).
- **Rewrite user SQL with explicit casts.** Invasive and fragile; it needs
  column types the component does not own.

## Policy consequence for users

- Batch values at a numeric position are normalized to that position's
  described encoding. Integers up to 2^53 in `Double` positions keep their
  exact value; `Real` positions store the nearest `f32`, as the column
  type dictates.
- An integer beyond 2^53 into a float position, a float that rounds
  outside the described integer range, a non-numeric JSON value at a
  numeric position, or a `REAL` overflow now fails with a typed error
  instead of storing corrupted bits.
- Homogeneous batches and single (non-batch) inserts keep their current
  behaviour. No new config knob, no public API change.
- The non-batch path has the same latent cross-message hazard (one poisoned
  cache entry per connection). It is out of scope here and tracked as a
  separate bd follow-up.

## Verification

- Integration tests (`camel-test/tests/sql_test.rs`, `postgres:17`):
  mixed bodies store exact values (cold path); the same endpoint handles two
  different kind profiles sequentially on a warm pool; homogeneous batches
  over `SMALLINT`/`INTEGER`/`REAL`/`DOUBLE PRECISION` columns store exact
  wire-width values; a `NUMERIC` column batch works through the fallback —
  twice in a row through the same pool, with the probe-warmed incompatible
  cache entry exercised; the 2^53 case, an 8-character string at a numeric
  position, and a `REAL`-overflow float each produce the typed error and
  write no row.
- Unit tests (no Docker): target-kind mapping, scan float-wins, every
  conversion branch (ties-to-even halves positive/negative, range checks,
  2^53 guard, `f32` overflow/underflow, unsigned magnitude, string/bool
  rejection), error message content; batch on a single-connection pool
  (fallback path, no second acquire).

## Out of scope

- Non-batch (single) binding path, retry classification, polling consumer.
- Sibling issues `rc-s7lb0` (NUMERIC/TIMESTAMPTZ decode) and `rc-sa1zo`
  (connect_timeout).
