# Park notes: sqlbinds

Mission: one bind planner for producer single, batch and consumer
(bd `rc-7e681` + `rc-0eiqm`). Worktree
`/home/shared/rust-camel-worktrees/sqlbinds`, branch `feature/sqlbinds`.
Tasks 1.1–1.6 complete: planner consolidated in `bind_kinds.rs`, batch and
consumer routed through `plan_binds`, raw binder `#[cfg(test)]`, ten
docker attack tests GREEN (suite 63/63), pinned suites untouched.

## Release-note caveat (verbatim)

> The SQL bind planner fix prevents new statement-cache poisoning. sqlx
> statement caches are per connection and do not survive process exit:
> restart on upgrade to clear entries seeded by a pre-upgrade binary.
> PgBouncer session mode must keep `server_reset_query=DISCARD ALL` (the
> default).

Upgrade-caveat scenario: satisfied only when the GENERATED release notes for
the next version carry this paragraph verbatim. The commit body, the bd
`rc-0eiqm` comment, and this park file are the three sources the release cut
consumes; the master conductor verifies the generated notes before tagging.

## Experiment: own-OID binds on the alias (design.md §1)

Live experiment against postgres:17 / sqlx 0.9.0, run in this worktree.

| Position shape | Named prepare describes | Alias prepare describes (`/* …-bind-kinds-fallback */`) | Measured execution outcome |
|---|---|---|---|
| bare `$1` in `INSERT INTO t (txt TEXT) VALUES ($1)` | TEXT | TEXT | int8 payload binds as int8 (alias OIDs follow the normalized arguments, not the Parse-described kinds); the server's assignment cast stores the correctly-cast decimal text `"3544952156018063160"` — no `"12345678"` bytes-as-text |
| bare `$1` with a bool payload into a TEXT column | TEXT | TEXT | bool binds as BOOL and the server cast stores `"true"` — never the `0x01` wire byte as `"\u{1}"` |
| bare `SELECT $1` | TEXT | TEXT | planner-routed read-back returns the exact integer `3544952156018063160` (value pinned by the pinned single-path test `single_select_bare_param_number_stays_numeric`) |
| `$1::text` feeding a BIGINT column | TEXT | TEXT | prepare itself fails: PG 42804 `column "n" is of type bigint but expression is of type text` (no text→bigint assignment cast) — loud, not silent |

Conclusion recorded by the ruling: the alias path needs no per-position reject
logic (`BindPlan.targets` stays `Vec<Option<BindTarget>>`); the rule is "bind
under the payload's own OID, the server either casts it or fails loudly", and
the rc-7e681 corruption (`"12345678"`) is decode-side — reading rows through
statement metadata described as TEXT — which is exactly what normalizing
against the read-back OIDs prevents.

## Test mapping: ten tests → six acceptance items → spec scenarios

Mission acceptance items used for the mapping:

- **A1** described TEXT/BOOL/BLOB positions never bind reinterpreted payload
  bytes (exact-number read-back, bool cast, TEXT-column decimal text).
- **A2** inserts the server cannot cast fail loudly — never a silent
  reinterpretation.
- **A3** probe-failure batches raise the typed 2^53 error in both probe
  orders (first-probe-unusable keeps the legacy message-wide scan; a
  later-unusable shape never erases earlier shapes' described guards).
- **A4** described numeric targets survive every fallback; only unresolved
  positions scan, message-wide.
- **A5** the rc-0eiqm end-to-end attack closes (consumer bind then batch
  2^53 → typed error) with no dependency on pool recycle.
- **A6** pinned suites stay green unchanged
  (`batch_homogeneous_kinds_unchanged`,
  `batch_mixed_int_float_kinds_never_corrupts`,
  `batch_numeric_column_fallback_works_twice`).

| # | Test | Label | Live baseline observation (from doc-comment) | Item | Spec scenarios |
|---|---|---|---|---|---|
| a | `batch_bare_select_text_described_own_oid_green` | RED | item 0's int8 bytes bound as ASCII `"12345678"` under the described-TEXT `$1`; item 1's int8 bytes for `123` (seven NULs + 0x7B) died at execution with PG `invalid byte sequence for encoding "UTF8": 0x00`; rollback, `mock:result` never received an exchange | A1 | described TEXT position with a JSON number never reinterprets bytes |
| b | `batch_text_column_int_stores_correctly_cast_value` | RED | described-TEXT named statement bound item 0's int8 bytes as TEXT `"12345678"`; item 1's bytes rejected (`0x00`), batch failed and rolled back | A1 | a JSON number into a TEXT column stores the correctly cast text |
| c | `batch_text_described_number_cast_or_loud_never_silent` | defense-in-depth | already loud pre-fix: described-TEXT probe of `VALUES ($1::text)` into BIGINT raises PG 42804 (`column "n" is of type bigint but expression is of type text`), batch fails through `Batch query prepare failed: …`, zero rows | A2 | closed matrix requirement (uncastable payload fails loudly) |
| d | `batch_bool_payload_casts_not_reinterprets` | RED | route A stored `true` at the described BOOL position, but route B bound the raw `0x01` bool byte under the described-TEXT statement and stored `"\u{1}"` instead of `"true"` | A1 | bool payload at a described TEXT position never reinterprets bytes |
| e | `batch_hybrid_keeps_described_float_guard_on_alias` | defense-in-depth | already green pre-fix: described-mode batch kept the described DOUBLE target and fired `Batch item 0, parameter 1: integer value 9007199254740993 exceeds 2^53 and would lose precision bound as f64`, zero rows | A4 | probe-failure batch keeps the described float guard |
| f | `batch_in_shapes_track_per_shape_targets` | RED | per-shape IN resolution worked and the failing leg already errored at `Batch item 1, parameter 3`, but the control leg corrupted item 2 — the bool at the TEXT shape position stored `"\u{1}"` where the alias cast must store `"true"` | A4 (per-shape tracking), A1 | variable-length IN with a later float parameter keeps its guard; a message-wide float keeps every shape's 2^53 guard |
| g | `consumer_post_query_seeded_entry_batch_2pow53_typed` | RED | the consumer's raw post-query int8 bind seeded the shared single connection's statement cache under `INSERT INTO sqlbinds_t8 (f) VALUES ($1)`; the batch's named prepare was a cache hit carrying BIGINT metadata, so 9007199254740993 bound as int8 and int8→float8 assignment cast stored it rounded — observed rows `[20.0, 9007199254740992.0]`, no typed error | A5 | post-fix the consumer no longer seeds bind-derived entries (prevention); the disagreement branch (pre-upgrade-seeded entry -> alias with probe-kept targets) is exercised at the unit level (`decide_path_aliases_on_disagreement`, `later_unusable_shape_never_erases_described_guards`) — recorded residual: no live PG17 integration test forces a disagreement |
| h | `consumer_post_query_number_at_float_is_typed` | RED | post-query's raw persistent int8 bind assigned into `f DOUBLE PRECISION` and silently stored 9007199254740992.0; no error on `mock:error`/`mock:consumed` within 5 s — stored `f = [9007199254740992.0]` | A5 | post-query binds through the planner |
| i | `batch_bool_column_int_payload_casts_or_fails_loudly` | **defense-in-depth** (honest correction — the doc-comment says `RED`, but pre-fix PG ALREADY rejected the payload with 22P03, so there was no silent corruption to turn green) | pre-fix: the named BOOL-described statement failed at execution with 22P03 `incorrect binary data format in bind parameter 1` instead of the pinned 42804; zero rows | A2 | incompatible payload at a described BOOL position never reinterprets bytes |
| j | `consumer_poll_and_stream_stay_green_through_planner` | defense-in-depth | both legs already green pre-fix: the poll delivered the row body unchanged and the drained StreamList NDJSON carried both rows; the planner's empty-bindings fast path must keep this | A6 | poll query binds through the planner; stream poll binds through the planner |

Pinned suites (unchanged, green inside the 63-test suite):
`batch_homogeneous_kinds_unchanged`,
`batch_mixed_int_float_kinds_never_corrupts`,
`batch_numeric_column_fallback_works_twice`.

## Consumer choice: machinery, not alias-only

All three consumer sites (`poll_database`, `poll_database_stream`,
`execute_post_query`) route through `plan_binds` (design.md §3) — described
mode when eligible, alias fallback otherwise, probe-versus-read-back mismatch
→ alias with probe-kept hybrid targets.

NOT alias-only. Justification: an alias alone stops new poisoning but leaves
the per-encoding guards off — an integer above 2^53 would still round silently
through the int8→float8 assignment cast — and would add a fourth bind policy
(and pay a server Parse per message, while the warm probe/describe is a cache
hit). With the planner, every bind is normalized against the read-back OIDs of
the statement that runs, and the migrated writers no longer seed raw-bind
entries under real SQL text; the agreement check neutralizes entries seeded by
a pre-upgrade binary with no pool recycle. Alias-only would have needed the
pool-recycle dependence the ruling forbids.

## Batch alias / namespace

- Fallback execution: `<item sql> /* batch-bind-kinds-fallback */` with
  `persistent(false)` (producer.rs) — nothing binds on the named text, so no
  probe failure can poison the named cache entry.
- Shape probes: `<shape sql> /* batch-bind-kinds-probe */`, one per distinct
  resolved SQL shape (IN-clause expansion shifts positions, so metadata is
  per shape, never per batch).
- Consumer namespaces: `/* consumer-bind-kinds-probe */` and
  `/* consumer-bind-kinds-fallback */` (`BindMode::Consumer` through
  `plan_binds`).
- Single keeps `/* single-bind-kinds-probe|fallback */` verbatim.
- Fallback is always `persistent(false)`; nothing is cached under fallback or
  probe text that any payload binds against.

## Eligibility matrix as implemented

The closed matrix is the table in
`openspec/changes/sqlbinds/specs/sql-binds/spec.md` (Requirement "Closed
wire-family eligibility matrix"), implemented by `described_mode_eligible` /
`bind_target_from_any_kind` in `crates/components/camel-sql/src/bind_kinds.rs`
and exercised cell-by-cell by `eligibility_matrix_is_closed`.

Shape-layer rule (batch): described numeric targets come from each shape's own
probe kinds; the fallback scan for positions without a described numeric
target is the message-wide `scan_bind_targets(all rows)` — never a per-shape
scan. The FIRST shape's probe selects the batch scan mode (today's
semantics); a later unusable shape contributes ineligible items (whole batch
to the alias) without erasing earlier shapes' described guards.

## Recorded spec-wording delta (Task 1.4 review)

In **legacy-scan mode** (first probe unusable), a later shape's prepare-time
server error now surfaces at PROBE time as `Batch query prepare failed` —
`probe_batch_shapes` probes all shapes before any execution — where pre-change
code surfaced it at EXECUTE time as `Batch query execution failed` for that
item. Same failure set, same rollback / zero-commit outcome; only the wrapper
text and the timing differ. This is plan-mandated (tasks.md Task 1.4 step 1:
"propagates through this same wrapper before any item binds") and should be
reconciled at spec-sync time with the sentence "existing server-error
surfaces are unchanged" — the wrapper class is unchanged, the surface point
for this one legacy-mode case moved from execution to prepare.

## Residual

A NUMERIC-probe batch keeps the legacy message-wide scan (first probe
unusable = whole batch legacy). That scan can still round an integer above
2^53 at a float column server-side: without a usable description there is no
described DOUBLE target to fire the typed guard, and the integer may bind as
I64 through the int8→float8 assignment cast. postgres-native describe
(bd `rc-cdger`) closes it by giving NUMERIC positions a usable description.
Not deferred from the acceptance list — this is the ruling's documented
residual, same behaviour as today (no loosening).

## Deferrals

```json
[]
```

## Diffstat

```text
 openspec/changes/sqlbinds/design.md               |  256 +++++
 openspec/changes/sqlbinds/proposal.md             |   85 ++
 openspec/changes/sqlbinds/specs/sql-binds/spec.md |  304 ++++++
 openspec/changes/sqlbinds/tasks.md                |  704 +++++++++++++
 10 files changed, 3657 insertions(+), 262 deletions(-)
```

(Captured with `git diff main...HEAD --stat | tail -5` at Task 1.7 start, before
this park file was added; the park commit adds one file.)

`docs/**` is untouched (`git diff main...HEAD --stat -- docs/` is empty), so
`mdbook test` is N/A.

## Gate table

Run foreground from the worktree root
(`/home/shared/rust-camel-worktrees/sqlbinds`), branch `feature/sqlbinds`.

| Gate | Command | Exit code |
|---|---|---|
| fmt check | `cargo fmt --check --all` | 0 |
| clippy | `cargo clippy -p camel-component-sql --all-targets -- -D warnings` | 0 |
| crate tests | `cargo test -p camel-component-sql` | 0 (308 passed, 0 failed) |
| docker suite (REQUIRED) | `cargo test -p camel-test --features integration-tests --test sql_test` | 0 (63 passed, 0 failed) |
| lint-unwrap | `cargo xtask lint-unwrap` | 0 |
| lint-secrets | `cargo xtask lint-secrets` | 0 |
| lint-unbounded-wait | `cargo xtask lint-unbounded-wait` | 0 |
| lint-ignore | `cargo xtask lint-ignore` | 0 |
| doc | `RUSTDOCFLAGS="-D warnings" cargo doc -p camel-component-sql --no-deps` | 0 |
| gates-local registry | `bash scripts/gates-local.sh` | 0 (30 passed, 0 failed; 4 skipped: lint-commits, lint-bridge-releases, doc-guide-test, cargo-audit — network/docs-toolchain) |
| doc-guide-test | `cd docs && mdbook test` | N/A — no `docs/**` changes |

No waivers: the docker suite is GREEN (63/63).

## Squash-commit body draft

```text
fix(sql): one bind planner for producer, batch and consumer

Route batch and consumer binds through plan_binds so every bind decision
(eligibility matrix, hybrid fallback, checked normalization) has one owner
and bind_json_values has no production caller.

Closes rc-7e681: batch described-OID corruption (int8 bytes read back as
TEXT "12345678", 0x01 as "\u0001") and the scan-drop that rounded integers
above 2^53 at a described DOUBLE/REAL position.

Closes rc-0eiqm: the consumer's three sites (poll, stream poll, post-query)
bound raw JSON persistently on pooled connections, seeding bind-derived OID
entries that a later described-path execution normalized to.

The SQL bind planner fix prevents new statement-cache poisoning. sqlx
statement caches are per connection and do not survive process exit:
restart on upgrade to clear entries seeded by a pre-upgrade binary.
PgBouncer session mode must keep `server_reset_query=DISCARD ALL` (the
default).

Bd: rc-7e681, rc-0eiqm
```
