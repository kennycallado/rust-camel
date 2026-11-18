# Design: sqlbinds

## Approach

One bind planner owns every SQL bind decision. The policy core (closed
wire-family eligibility matrix, hybrid fallback, checked per-target
normalization) already lives in `bind_kinds.rs` and the producer single path
already uses it. This change routes the remaining two writers through it and
removes the raw binder's reachability.

### 1. One planner in `bind_kinds.rs`

Consolidate the decision into one entry point:

```rust
pub(crate) async fn plan_binds(
    conn: &mut AnyConnection,
    sql: &str,
    rows: &[&[serde_json::Value]],
    mode: BindMode,
) -> Result<BindPlan, CamelError>
```

`BindMode` is `Single | Consumer | Batch` and only selects the alias
namespace suffix (`/* {mode}-bind-kinds-probe */`,
`/* {mode}-bind-kinds-fallback */`). `rows` is one row for single/consumer;
the batch routes through the shape planner (`group_rows_by_sql` +
`probe_batch_shapes` + `plan_batch_items`, §2) rather than this
single-statement entry point.

Two distinct statement-metadata stages, kept explicit:

- **The probe decides the mode.** `plan_binds` performs the existing probe
  dance (prepare with empty parameter types on the probe alias;
  `AnyDriverError` or an empty type list means "no description") and
  classifies the statement:
  - **Described-named** (probe types present and every row passes
    `described_mode_eligible`): the real SQL text, persistent. The probe
    never supplies the executed statement's bind targets.
  - **Hybrid-alias** (description present but at least one row fails the
    matrix): alias text (`/* {mode}-bind-kinds-fallback */`),
    `persistent(false)`.
  - **Legacy-scan** (no usable description — e.g. NUMERIC columns): alias
    text, `persistent(false)`, `scan_bind_targets(rows)`. Unchanged
    behaviour; per the ruling a description-less probe must not start
    failing or every batch touching a NUMERIC column breaks.
- **The read-back decides the targets on the named path — when it agrees
  with the probe.** For every described-named execution — single, consumer,
  and batch alike — the statement actually executed is prepared with empty
  parameter types and its returned parameter kinds are mapped through
  `bind_target_from_any_kind`. On a cache hit the read-back parameters are
  the cached statement's actual OIDs, so normalization always targets the
  statement that runs. Probe kinds are used for targets only when building
  hybrid fallback targets.
- **Probe-versus-read-back agreement check.** PG infers parameter types
  deterministically from the SQL text, so on a healthy connection the
  read-back kinds always equal the probe kinds. A disagreement means the
  named entry was seeded by a raw bind (bind-derived OIDs — the rc-0eiqm
  poisoning mechanism, including entries left by a pre-upgrade binary).
  Agreement is compared on the RAW parameter kinds (position-wise
  `AnyTypeInfoKind` equality), not on mapped numeric targets — mapped
  comparison would hide TEXT-vs-BOOL disagreements (both map to no target).
  When the read-back is unusable or disagrees, the planner SHALL treat the
  statement as ineligible: it executes on the unnamed alias with the
  probe-derived hybrid targets, never on the named statement — binding
  probe-derived bytes into a statement whose cached parameter OIDs disagree
  would corrupt the wire encoding in the other direction (f64 bytes into an
  int8-parameter statement). The alias is bind-governed, so the probe's
  described numeric guards (e.g. 2^53 at a DOUBLE position) fire at bind
  time before any bytes reach the server, with no dependency on pool
  recycle. The probe alias text is never bound against, so it cannot be
  poisoned.
- **Own-OID binds on the alias are faithful — measured, not assumed.** A
  live experiment against postgres:17 / sqlx 0.9.0 (run in this worktree,
  recorded in the park) established: a bare `$1` describes as TEXT on both
  named and alias prepares, yet at execution the parameter encoding follows
  the bound values — an int8 payload binds as int8 everywhere tested: bare
  `SELECT $1` reads back the exact integer; an INSERT into a TEXT column
  stores the correctly-cast decimal text `"3544952156018063160"`; a bool
  stores `"true"`; only an explicit `::text` cast feeding a BIGINT column
  fails loudly (PG 42804, no text→bigint assignment cast). No bytes-as-text
  corruption occurs at the server on any alias shape. (Protocol note: the
  alias OIDs follow the normalized arguments, not the Parse-described
  kinds.) The corruption bd
  rc-7e681 recorded ("read back as TEXT yields \"12345678\"") is therefore
  decode-side — reading rows through statement metadata that was described
  as TEXT — which is exactly what binding through normalized targets instead
  of raw JSON prevents. Consequently the alias path needs NO per-position
  reject logic: `BindPlan.targets` stays `Vec<Option<BindTarget>>`, and the
  ruling's mechanism — bind under the payload's own OID, the server either
  casts it or fails loudly — is implemented verbatim and is empirically
  demonstrated. Authorized interpretation, recorded for the park: the
  mission's acceptance wording "→ typed error" for the described-TEXT and
  described-BOOL attacks is satisfied by this cast-or-loud-failure contract
  (the 2026-11-15 e_opus ruling's own words); what is forbidden is the
  ASCII-byte read-back and the wire-byte reinterpretation, never the
  faithful server cast.

### 2. Batch (`execute_batch`) and SQL-shape metadata

Batch items can resolve to **different SQL texts**: IN-clause expansion
rewrites `in (:list)` into one placeholder per element (`query.rs`), so a
variable-length IN shifts every later parameter position. Per-position
metadata is therefore tracked **per resolved SQL shape**, never per batch:

- The planner (or the batch loop through it) describes each distinct resolved
  shape once, on that shape's own probe alias
  (`<shape sql> /* batch-bind-kinds-probe */`; warm executions are statement
  cache hits).
- Whole-batch eligibility is retained: each item is matrix-checked against
  its own shape's described kinds, and **one ineligible item sends the whole
  batch to the alias**. All plans are decided BEFORE any item executes
  (probes and named read-backs complete for every shape first), so a
  mismatch discovered on a later shape still moves earlier items to their
  aliases. A shape whose probe yields no usable description
  contributes ineligible items (they keep the legacy scan behaviour for
  their positions), which for an all-NUMERIC batch reproduces today's
  unconditional legacy scan exactly.
- Hybrid fallback targets are built from two layers with a strict
  no-loosening rule: the **described numeric targets come from the shape's
  own probe kinds** (IN-clause expansion means each shape describes its own
  positions; shape A's targets never apply to shape B's positions), while
  the **scan fallback for positions without a described numeric target is
  the message-wide scan across ALL binding rows** — never a per-shape scan.
  A message-wide scan lets a float anywhere in the message win its position
  and keeps the 2^53 guard firing for every shape, exactly as today's
  single message-wide scan does; per-shape scans would let an
  integer-above-2^53 bind as I64 and round silently server-side where the
  current code raises the typed error (a loosening — regression-tested).
  The batch scan mode follows TODAY's semantics: the FIRST shape's probe
  selects it (producer.rs:846-875 probes `prepared_items.first()`). If the
  first probe is unusable, the whole batch keeps the legacy message-wide
  scan (every item AliasPlan, scan targets only) — for an all-NUMERIC
  batch this reproduces today's unconditional legacy scan exactly. If the
  first probe is usable, the batch is in described mode: a LATER unusable
  shape contributes ineligible items (whole batch to the alias) but must
  NOT erase the earlier shapes' described guards — their hybrid targets
  keep the described numeric targets on the alias. Both orders are
  regression-tested: reversing the probe order must preserve precision
  (a later-unusable NUMERIC shape must not turn the first shape's typed
  2^53 rejection into a silent I64 bind), and a mixed usable/unusable
  batch must not let a kept BIGINT target from the later shape's usable
  probe replace the message-wide scan's F64 guard at a shared position
  (`n NUMERIC` with `k BIGINT`: the empty-list shape describes the shared
  bind index as BIGINT).
- Described-named items keep today's named prepare + per-item read-back and
  bind on the persistent named text — the named path runs ONLY when the
  read-back agrees with the shape's probe kinds (raw-kind equality per
  position, §1). A disagreeing or unusable read-back makes the item execute
  on its alias with the probe-kept hybrid targets instead of binding
  probe-derived bytes into a statement whose cached OIDs disagree, so a
  described DOUBLE position keeps its 2^53 guard even against a poisoned
  entry (rc-7e681 defect 2, closed without depending on pool recycle).
- Hybrid-alias items execute on `<item sql> /* batch-bind-kinds-fallback */`
  with `persistent(false)` — an unnamed statement whose parameter OIDs come
  from the binds themselves, so an ineligible payload binds under its own
  OID and the server either casts it or fails loudly (rc-7e681 defect 1). No
  probe failure can poison the named cache entry because nothing binds on
  the named text.

This is "stricter or equal": nothing that errors today becomes silently
accepted, no guard stops firing, previously accepted shapes keep their
accepted status, and corrupting inputs become correct (own-OID bind, server
cast) or loud (server error). The one deliberate movement — a mixed batch
whose secondary shape has an unusable description moves that shape's items
from named-default binds to alias+scan — only adds checked guards on numeric
payload positions and never changes an accepted outcome into a corrupted
one.

### 3. Consumer

Chosen option: **the machinery**, not an alias-only workaround (e_opus ruling
2026-11-15). All three sites route through `plan_binds`:

- `poll_database`: acquire one pooled connection, plan, bind via
  `bind_single_item`, fetch on that same connection (named path normalizes
  against the read-back OIDs of the statement that executes).
- `poll_database_stream`: same pattern inside the `try_stream!` block — plan
  and fetch on the connection acquired at stream start (mirrors the producer
  `StreamList` path).
- `execute_post_query`: acquire one pooled connection, plan, bind, execute on
  it (today it executes a raw persistent query directly on the pool).

Justification: an alias alone stops new poisoning but leaves the per-encoding
guards off — an integer above 2^53 would still round silently through the
int8→float8 assignment cast — and would add a fourth bind policy. With the
planner, every bind is normalized against the read-back OIDs of the statement
that runs, and migrated writers no longer seed raw-bind entries under real
SQL text on fresh connections.

**Scope boundary (recorded in the ruling):** repair of statement-cache
entries seeded by a pre-upgrade binary is out of scope. sqlx statement caches
are per connection and do not survive process exit, so an upgrade restart
clears them. Note the agreement rule already NEUTRALIZES such entries on the
migrated paths: a disagreeing read-back makes the planner bypass the poisoned
named statement and execute on the alias instead — the entry is bypassed,
not repaired (it still sits in the cache until recycle, but no bind
normalizes against it). This change SHALL still record in the release
notes: restart on upgrade clears poisoned caches, and PgBouncer session mode
must keep `server_reset_query=DISCARD ALL` (the default).

### 4. Compiler-enforced planner exclusivity

`bind_json_values` becomes `#[cfg(test)]`. Its production callers disappear
(the three consumer sites migrate; the planner's `Default` arm uses
`bind_single_json_value`, which stays production code). Any fourth bind path
then fails to compile.

### 5. Alias namespaces

Each mode owns its probe and fallback suffixes:
`/* single-bind-kinds-probe */`, `/* single-bind-kinds-fallback */` (existing,
kept verbatim), `/* batch-bind-kinds-probe */`,
`/* batch-bind-kinds-fallback */` (existing, kept verbatim), and new
`/* consumer-bind-kinds-probe */`, `/* consumer-bind-kinds-fallback */`.
Fallback executions are always `persistent(false)`; nothing is cached under
fallback or probe text that any payload binds against.

### 6. New capability spec `sql-binds`

`openspec/changes/sqlbinds/specs/sql-binds/spec.md` states normatively: the
closed eligibility matrix (every cell), the hybrid fallback rule (described
numeric targets never dropped, tracked per SQL shape), batch and consumer
binds go through the planner, no raw binder outside it, and failures are
typed errors — never reinterpreted bytes. This closes the gap left when
`sql-single-bind-normalization` archived with `skip_specs: true`.

## Affected crates

- `camel-component-sql`: `bind_kinds.rs` (planner consolidation, cfg(test)
  raw binder), `producer.rs` (batch eligibility gate + shape-aware hybrid
  fallback), `consumer.rs` (three sites through the planner). No public API
  change; `bind_kinds` stays `pub(crate)`.
- `camel-test`: new integration tests in `tests/sql_test.rs` (postgres:17
  testcontainers, same harness as the pinned bind tests). Existing tests
  untouched.

## Architecture boundaries

Component-internal correctness fix. No Runtime, DSL, builder, or config
surface changes; no new endpoint config keys; no cross-crate dependency
changes. The `sql-binds` capability spec is additive. Error messages stay
stable: the pinned bind wording (`Batch item N, parameter P: …`,
`parameter P: …`) is unchanged and covered by existing byte-exact tests; new
loud failures surface the existing `Batch query execution failed: …` /
`Post-query execution failed: …` wrappers around server errors, because the
message that was wrong is the silent success, not a wording. Normalization
failures (typed bind errors) are distinguishable from server failures by
their existing wrappers, unchanged.

## Phases

Single coherent slice: the planner consolidation, both writer migrations, and
the spec land as one change. Splitting them would leave a blessed intermediate
state where `bind_json_values` still has production callers (the ruling
requires the planner and the privacy to land together).
