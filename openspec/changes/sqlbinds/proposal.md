# Proposal: sqlbinds

## Why

One bind-policy defect family has two shipped writers, both reproduced on
postgres:17 / sqlx 0.9.0:

- **rc-7e681 (batch path, `execute_batch`)**: a described nonnumeric position
  (TEXT/BOOL/BLOB — a bare-parameter batched SELECT is described as TEXT by
  PG) still accepts a JSON number or bool and binds that payload's wire bytes
  under the nonnumeric OID: `3544952156018063160` read back as TEXT yields
  `"12345678"`, `true` yields `"\u0001"`. Silent corruption, no error. And on
  a per-item describe that yields no usable types, the batch falls back to a
  message-wide scan that drops described numeric targets, so an integer above
  2^53 at a described DOUBLE PRECISION position cast-rounds
  (`9007199254740993` → `9007199254740992`) instead of raising the typed 2^53
  error.
- **rc-0eiqm (polling consumer)**: all three bind sites (`poll_database`,
  `poll_database_stream`, `execute_post_query`) call the raw default binder on
  persistent pooled queries. sqlx caches statements by SQL text and fixes
  parameter OIDs at the first Parse, so a consumer message can seed a
  bind-derived OID entry that a later described-path producer or batch
  execution normalizes to — bypassing the per-encoding guards the producer fix
  added. The producer fix is prevention, not repair.

The machinery already exists (`bind_kinds.rs`: eligibility matrix, hybrid
fallback, checked normalization) and the producer single path already uses it.
The gap is that batch and consumer do not, and that the raw binder stays
callable from anywhere.

## What Changes

- One bind planner in `bind_kinds.rs`, called by all three sites: producer
  single (already migrated), producer batch, consumer (all three shapes). The
  raw binder `bind_json_values` becomes `#[cfg(test)]` so the compiler
  enforces "no raw binds outside the planner".
- Batch: closed-matrix eligibility across every item; one ineligible item
  sends the whole batch to an unnamed alias statement with hybrid targets
  (described numeric targets kept, unresolved positions scanned). A probe
  with no usable description keeps the legacy scan. Homogeneous eligible
  batches stay byte-identical.
- Consumer: poll, stream poll, and post-query binds plan through the planner
  (described mode when eligible, alias fallback otherwise).
- New capability spec `sql-binds` stating the matrix, the hybrid fallback,
  planner-only binds, and typed-error policy. This closes the spec gap left
  by `sql-single-bind-normalization` (archived with `skip_specs: true`).

Explicitly excluded: SQL control-plane work (`rc-qv2iz`, `rc-cdger`,
`rc-2ljv5`, `rc-y6fkh`), repair of pre-upgrade statement-cache entries
(sqlx caches are per connection and do not survive process restart;
release-notes note instead), any new public config key, any API surface
leaving the crate (`bind_kinds.rs` stays `pub(crate)`).

## Acceptance criteria

- Described TEXT/BOOL/BLOB positions never bind reinterpreted payload bytes:
  planner-routed read-backs of a bare `SELECT` with `3544952156018063160`
  return that exact number (the batch path itself discards SELECT rows);
  a bool returns a bool or cast value, never `"\u0001"`; an INSERT of that
  number into a TEXT column stores the correctly-cast decimal text; inserts
  the server cannot cast fail loudly (authorized interpretation of the
  mission's "typed error" wording, per the 2026-11-15 ruling).
- Probe-failure batches raise the typed 2^53 error, not a rounded value, in
  both probe orders: a first-probe-unusable batch keeps today's legacy
  message-wide SCAN guards; a later-unusable shape never erases earlier
  shapes' DESCRIBED guards.
- Described numeric targets survive every fallback; only unresolved positions
  scan (message-wide).
- The rc-0eiqm end-to-end attack closes: a consumer post-query bind followed
  by a batch integer above 2^53 at that position gives the typed error, with
  no dependency on pool recycle.
- `batch_homogeneous_kinds_unchanged`,
  `batch_mixed_int_float_kinds_never_corrupts`,
  `batch_numeric_column_fallback_works_twice` stay green unchanged.

## Risk budget

Acceptable: extra probe on first use of a consumer/batch statement (warm path
is a cache hit, ~130 ns); PG error surfaced where the server cannot cast.
Out of bounds: loosening any existing typed error, text-encoding numeric
payloads under nonnumeric OIDs, changing accepted homogeneous batch behaviour,
touching the excluded control-plane issues.

Refs: bd rc-7e681, rc-0eiqm (one change per delta85 ruling). Affected crates:
`camel-component-sql`, `camel-test` (integration tests).
