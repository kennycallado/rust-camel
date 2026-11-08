# sql-single-bind-normalization

Revision 3 — gate history: revision 1 rejected (described nonnumeric
position corruption, overstated pool-wide guarantee, select-path coverage,
cost wording). Revision 2 rejected (bool wire bytes under a described TEXT
OID; message-wide fallback dropping described numeric targets). Revision 3
addresses the two blocking findings with a closed wire-family matrix and a
hybrid fallback that preserves every available numeric described target.
The change remains bounded reuse of the landed batch machinery
(`0eff69b23`, bd `rc-fef4z`). bd `rc-mn139`, P1, data corruption.

## Why

`execute_select` and `execute_modify` build `sqlx::query(AssertSqlSafe(..))`
and bind through `bind_json_values` (i64 first, else f64). sqlx keys its
statement cache by SQL text alone and fixes parameter OIDs at the first
`Parse`; the connection is pooled and reused. A second message whose JSON
number at the same parameter position has a different kind is decoded under
the first message's OIDs — int8/float8 bit reinterpretation, no error. Same
mechanism proven for batch (archive `sql-batch-bind-kinds`), now on the path
every user hits.

Secondary hazard (same bd): a same-connection entry left by single mode with
a bind-derived int8 OID at a float8 column makes the batch described path
normalize to i64, so an integer > 2^53 passes the batch guard and the server
rounds silently. Both endpoints can share that connection through a named
datasource pool.

## Chosen fix (preferred shape from the mission order)

Apply described-OID normalization to the single path. Reuse the landed
machinery; no second scheme:

- `scan_bind_targets` — unchanged (per-message scan fallback).
- `bind_target_from_any_kind` — unchanged (described kind → wire encoding).
- `normalize_for_target` + the checked conversion stack (`normalize_number`,
  `f64_to_int`, `to_f32_checked`, `reject_number`) — factored to take a
  `BindCtx` that names the offending bind: `BatchItem(usize)` reproduces the
  existing batch wording byte-for-byte; `Single` names only the parameter
  position. Batch behaviour does not change bytes.
- `bind_batch_item` — same signature, delegates to the shared
  `bind_values_for_targets`; new `bind_single_item` is the single-mode entry.

### Per-statement plan (`plan_single_binds`, producer.rs)

For a statement with at least one binding, executed on one pooled
connection (probe and execute share that connection — normalization must
target the OIDs of the statement that actually runs):

1. **Probe** on a distinct SQL-text namespace
   (`{sql} /* single-bind-kinds-probe */`), exactly like the batch probe:
   preparing with empty parameter types seeds a cache entry under the probed
   text, and for unmapped parameter types (e.g. `NUMERIC`) that entry is one
   no payload can satisfy. Keeping the probe on its own text keeps the
   user's SQL text namespace clean. Probe error classification mirrors
   batch: `AnyDriverError` → fallback; other errors → typed failure.
2. **Described mode** — only if the read-back is *eligible*: every position
   either maps to a numeric bind target (`bind_target_from_any_kind`) or
   carries a payload whose default-binder wire family matches the
   described kind (matrix below). An eligible statement then
   `prepare_with`es the user's text (cache hit after the first
   message; the read-back parameters are then the cached statement's
   actual OIDs), binds with `bind_single_item`, and executes the user's
   text persistently on the same connection.
3. **Hybrid fallback** (undescribed types, or an incompatible payload at a
   nonnumeric described position): execute
   `{sql} /* single-bind-kinds-fallback */` with `persistent(false)` —
   an unnamed statement whose parameter OIDs derive from the binds
   themselves. **Targets are hybrid, not message-wide-scan**: every
   numeric described target is kept (so the 2^53 and range guards still
   fire at positions the server described); only positions without a
   described numeric target resolve from this message's
   `scan_bind_targets`. Because the statement is unnamed, each position's
   Parse OID comes from its own normalized bind — a kept `Double` target
   binds f64 under a float8 OID, a scanned integer binds i64 under an
   int8 OID, a scanned bool binds bool — all exact, none reinterpreted.
   The fallback alias is distinct from the probe alias on purpose: sqlx
   consults the SQL-text cache before the persistence flag, so a
   probe-seeded entry would otherwise be reused. Nothing is cached under
   the fallback text.

### Described-mode eligibility (gate findings 1–2, revisions 2–3)

A described position the default binder cannot encode faithfully must not
execute under the described statement. On Postgres the described read-back
is a closed set (`sqlx-postgres 0.9.0` `TryFrom<&PgTypeInfo>`): only Bool,
Null, SmallInt, Integer, BigInt, Real, Double, Text and Blob map; every
other server type (numeric, timestamps, dates, uuid, json, …) fails the
probe with `AnyDriverError` and the whole text runs fallback. Within that
closed set the compatibility rule is a wire-family match between the
payload's default-binder encoding and the described kind:

| Payload family (default binder) | Compatible described kinds |
|---|---|
| Null | all |
| Bool (1 byte) | Bool |
| Number (int8/f64 bytes) | numeric kinds only — always mapped to a `BindTarget`, never eligible for default binding |
| String / Array / Object (raw UTF-8 JSON text bytes) | Text, Blob (bytea binary is raw bytes; citext maps to Text) |

Anything else at a nonnumeric described position — a bool payload under a
described TEXT OID (reproduced on `postgres:17`: `true` returns TEXT
`"\u0001"`, `false` fails invalid-UTF-8), a number under TEXT (reproduced:
`3544952156018063160` returns the string `"12345678"`) — marks the message
ineligible and routes it to the hybrid fallback, where the payload binds
under its own OID and stays exact. Eligibility is a pure helper,
`described_mode_eligible(kinds, bindings)` built on
`payload_wire_family`/`described_wire_family`, with unit tests covering
every matrix cell.

The hybrid fallback preserves the described numeric guards (gate finding
2): for `SELECT $1::double precision AS f, $2 AS n` with
`[9007199254740993, 3544952156018063160]` the described read-back is
`(Double, Text)`; the number at the Text position makes the message
fallback, but position 0 keeps its `F64` target — the 2^53 guard fires
with the typed error instead of a silent `::double precision` cast rounding
to `9007199254740992`. A message-wide scan (revision 2's shape) would have
dropped that guard; the hybrid keeps guards at every position the server
described.

The shipped batch path shares both holes (numbers and bools bound under
described TEXT OIDs; probe-failure batches scanning away described numeric
targets). Fixing batch is out of scope here — batch behaviour must not
change bytes (mission non-goal) — and is filed as one bd follow-up
discovered from rc-mn139.

Statements without bindings keep today's path untouched (no probe, no
normalization, persistent execution of the user's text): there is nothing
to reinterpret.

All three `outputType` select paths and `execute_modify` go through the
same plan. `StreamList` acquires its connection inside the stream and plans
against it, so the probe-and-execute-same-connection invariant holds there
too.

### Typed-error wording (single mode)

`"parameter {position}: {reason}"` — e.g.
`parameter 0: integer value 9007199254740993 exceeds 2^53 and would lose
precision bound as f64`. Batch wording stays
`"Batch item {item}, parameter {position}: {reason}"` — byte-identical,
pinned by unit tests, including the full nonnumeric-rejection clause
(`"...; batch mode requires numeric JSON at described numeric positions"`).

## The 2^53 policy is per-position-encoding, not global

The batch proposal's claim ("Integers up to 2^53 in `Double` positions keep
their exact value", archive `sql-batch-bind-kinds`, *Policy consequence for
users*) is a per-position-encoding guarantee: the guard lives in the
conversion to the position's described encoding, so it holds only while the
described encoding is the server's inference for that SQL text. rc-mn139's
second hazard is exactly a broken premise: a poisoned cache entry flips a
`Double` position to an int8 encoding and the guard never runs.

**This change is prevention, not repair.** It removes the producer-side
poisoning source: single-mode statements described in eligible mode only
ever seed the user's SQL text with server-inferred OIDs, and fallback
statements are unnamed on a private alias. The described read-back itself
cannot repair a pre-existing entry — on a cache hit, `prepare_with` returns
the cached OIDs as-is, and normalization targets those. Two residual
writers are outside this fix and acknowledged, not claimed away:

- the polling consumer binds raw JSON persistently through the same pool
  (`consumer.rs` post-query path, `bind_json_values` on a default
  persistent query) and can still seed a poisoned entry;
- cache entries created by a pre-upgrade binary survive on live pooled
  connections until the pool recycles them.

Within producer-only traffic the premise holds again — which is exactly
what the `batch_guard_still_holds_after_a_single_mode_entry` regression
proves. The consumer-side writer is filed as a bd follow-up discovered
from rc-mn139. The archived batch wording is left as-is (no history
rewrites); this section is the nuance statement the mission order asks
for.

## Probe cost (measured)

Standalone measurement, `sqlx 0.9.0` Any driver against `postgres:17`
(throwaway container, `release` profile, `max_connections(1)`, two runs):

- Cold probe (first prepare of a text on a connection — one
  Parse/Describe round trip): 239 µs / 254 µs.
- Cold real-text `prepare_with` (the prepare today's execution pays
  anyway inside `execute`): 103 µs / 169 µs; read-back parameters
  `Some(Left([Double, Integer]))` — the server-inferred OIDs, as the
  design requires.
- Warm probe, every message after the first (statement-cache hit, no
  server round trip): 132–138 ns.
- Warm real-text prepare: 127–130 ns.
- Baseline plain `SELECT 1` execution for scale: 57.6–58.7 µs.

Steady state: two cache-hit lookups per message (~260 ns combined) on top
of the execution's own round trip — ~0.5% of one round trip, **while both
cache entries remain resident** (pool `maxLifetime`/idle eviction re-arms
the one-time cold cost for the recycled connection; `persistent` is always
on in the described path, so entries are cached normally). The only extra
round trip is the probe's Parse/Describe once per (SQL text, pooled
connection).

## Verification plan

Mission §3 test list (all four named regressions, red first):

- `single_insert_mixed_kinds_never_corrupts`: the same single-mode
  endpoint (pinned to one pooled connection) receives a float message then
  an int message; stored `f`/`i` must be exactly `10.5`/`1` then
  `20.0`/`2` — never reinterpreted bits.
- `single_select_mixed_kinds_never_corrupts`: parameter kinds flip at the
  same position per message, across **all three select output paths**
  (`SelectList`, `SelectOne`, `StreamList` — the StreamList body fully
  consumed to NDJSON in a route processor).
- `single_bind_precision_error_is_typed`: an integer > 2^53 at a
  described float position errors with the batch-equivalent typed message
  (single-mode wording: `parameter 0`, no batch index) and writes no row.
- `batch_guard_still_holds_after_a_single_mode_entry`: through one shared
  single-connection datasource pool, a single-mode int/float insert must
  not poison the batch described path — the batch message with an integer
  > 2^53 at the float position still trips the guard.

Revision-3 additions (gate findings):

- `single_select_bare_param_number_stays_numeric` pins the gate finding:
  the bare-parameter SELECT regression with the `3544952156018063160` trap
  value (must not come back as `"12345678"`), then a float at the same
  position on a warm statement.
- `single_select_bool_then_string_stays_exact`: `SELECT # AS value` with a
  bool payload (must return boolean `true`, not TEXT `"\u0001"`) then a
  string payload (described mode, exact) — kind flip at the same position.
- `single_select_hybrid_keeps_described_float_guard`:
  `SELECT #::double precision AS f, # AS n` with
  `[9007199254740993, 3544952156018063160]` must fail with the typed 2^53
  error naming parameter 0 (described `Double` target kept through the
  hybrid fallback), never a silent cast-round; a second message
  `[2.5, 3544952156018063160]` stores/returns both values exactly.
- Unit tests (no Docker) pin the **complete batch error strings
  byte-for-byte** during the `BindCtx` factor — the
  `"Batch item {item}, parameter {position}: {reason}"` shape including the
  `"...; batch mode requires numeric JSON at described numeric positions"`
  clause — plus the single-mode wording, every `described_mode_eligible`
  matrix cell (Null/Bool/Number/String × Text/Blob/Bool/numeric kinds),
  and the hybrid target assembly (described numeric targets preserved;
  only unresolved positions scanned).

Tooling gates:

- Red first on `main` (worktree at base `b4a3f459c`): the new integration
  tests fail with the corrupted values / silent rounding; raw outputs land
  in the park file.
- Green after the fix: exact stored values, typed errors, batch guard
  intact.
- `cargo test -p camel-component-sql --lib`; full
  `cargo test -p camel-test --test sql_test` (count stated in the park
  file).
- `cargo fmt --check --all`, `cargo clippy -p camel-component-sql
  --all-targets -- -D warnings`, `cargo xtask lint-unwrap`.

## Non-goals

- No batch behaviour change (bytes); the batch guard may only get stricter
  or equal, never looser — it is untouched here and its premise is restored.
- No new config knob, no driver change, no retry changes.
