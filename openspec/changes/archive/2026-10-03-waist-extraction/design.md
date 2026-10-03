# Design: waist-extraction

## Approach

Two new modules inside `camel-integration-test`, plus one ADR amendment.
Nothing outside the crate changes (docs aside). e_opus pre-flight
(ses_efe6963c0ffeZZKPvLGNhAWlj0, GO-WITH-CAUTIONS) shaped the seams.

**1. `src/steering.rs` — the datasource steering axis.**

- `sanitize_db_error` moves here (unit tests ride along). Two public
  paths survive: the crate-root re-export in `lib.rs`, and a
  compatibility re-export inside `sql_action`
  (`pub use crate::steering::sanitize_db_error;`) — the module is
  public surface today, so `sql_action::sanitize_db_error` must keep
  resolving. The surreal family imports from `steering` directly,
  ending the reach-through. Ungated: it is public surface.
- One resolver, cfg-gated `#[cfg(any(feature = "sql", feature =
  "surreal"))]` (its `use` statements gated identically), generic over
  the handle type — no family enum:

  ```rust
  pub(crate) async fn resolve_datasource<T: 'static + Send + Sync>(
      catalog: &Arc<dyn DatasourceCatalog>,
      name: &str,
      label: &str,
  ) -> Result<(Arc<T>, String), String>
  ```

  It runs `get_config` -> `get_pool` -> `downcast::<T>()` and returns
  the handle plus the `db_url` (callers still sanitize statement and
  query errors against it). Error strings, byte-identical to today:
  `"{label}: unknown datasource '{name}'"` and
  `"{label}: datasource '{name}': {sanitized}"`. Labels stay at the
  call sites: `sql action`, `sql validation`, `surreal action`,
  `surreal validation`. The "no catalog" check stays at the validate
  sites (it predates resolution and wraps differently).
- Resolution wrapping stays separate from snapshot formatting at the
  validate sites: a resolver error is already a complete message, so it
  wraps into `ActionTransport { Other { message } }` directly — it
  never passes through the family `apparatus` helper (which prefixes
  raw sanitized snapshot text with `"{label}: datasource '{name}': "`;
  passing resolver output through it would double the prefix). The
  `apparatus` helpers keep their exact bodies for snapshot and query
  errors. Exact-string regression tests pin every label against
  unknown-name and pool-failure messages — equality, not substring.
- The four sites call it. Action sites return the `String` unchanged.
  Statement-level errors (`"datasource '{name}' statement [{i}]:
  {sanitized}"`) stay in the family executors — they carry no label
  and no pool logic.

**2. `src/runner/poll.rs` — the deadline poll driver.**

cfg-gated `#[cfg(any(test, feature = "http", feature = "sql",
feature = "surreal"))]` so feature-off builds stay warning-clean. Two
total judgments instead of one partial judge (a single
`judge(_, is_final)` closure could return `Continue` on the final read,
forcing a fallback no caller wants):

```rust
pub(super) async fn poll_until<S, F, Fut>(
    deadline: Option<Duration>,
    interval: Duration,
    snapshot: impl FnMut() -> Fut,          // FnMut() -> Fut<Output = Result<S, F>>
    early: impl FnMut(&S) -> Option<Result<(), F>>,
    decide: impl FnMut(&S) -> Result<(), F>,
) -> Result<(), F>
```

- No deadline: one snapshot, `decide`.
- With deadline: `until` is fixed BEFORE the first snapshot (the
  existing loops' anchor). Each iteration then runs — snapshot (a
  snapshot error stops at once), `early` may stop the poll, then the
  expiry check against `until`, then `decide` on the expiry snapshot,
  else sleep `min(interval, remaining)`. A deadline never cancels an
  in-flight snapshot: an overrunning snapshot completes and its early
  judgment still precedes the expiry decision. The `now` of the expiry
  check is read after the snapshot and the early judgment, never
  before them.
- Partner passes `early = judged_failure-or-settles` (monotone
  early-settle; `settles_early` implies `bound_holds`, so the final
  read matches today) and `decide = judged_failure-then-settles(final)`.
  SQL and Surreal pass `early = above_ceiling breach only` and
  `decide =` their existing `decide`. Per-family poll interval constants
  stay in the family files (all 100 ms today, still named per family).
- Driver unit tests use `#[tokio::test(start_paused = true)]` — no real
  sleeps (lint-test-sleep); this needs tokio `test-util` as a
  dev-dependency (the workspace `full` feature does not enable it).
  Cases: no-deadline single snapshot, early stop, final-snapshot
  decide, snapshot error stops at once, sleep never exceeds the
  remaining window, an overrunning snapshot still gets its early
  judgment before the expiry decision.

Isolated verification: every gate run includes warning-denied builds
(`cargo clippy -p camel-integration-test -- -D warnings`, plus
`cargo check` equivalents) for five feature matrices — featureless,
`http`-only, `sql`-only, `surreal`-only, and combined — so dead-code
regressions in either seam's cfg surface fail the change, not CI.

The driver does NOT go into `camel-matchers`: ADR-0072 pins that crate
to pure types and functions, and the driver awaits tokio sleeps.

**3. ADR-0069 section 14 — traffic/state adapter taxonomy.**

Amendment, following the section-13 precedent (status line gains the
amendment note). Content: section 5's "partner-side assertions are the
only normative proof" is qualified as traffic-family law — for state
families, normative proof is the catalog-backed observation of rows at
rest, which section 14 recognizes; traffic adapters (harness-owned far
side of the wire; partner family) vs state adapters (rows at rest
through a datasource catalog pool; SQL and Surreal families); the waist
map — steering axis (this change), poll driver (this change), matcher
algebra home (`camel-matchers`, citing ADR-0072 as Proposed, deciding
nothing about it); activation-per-need (one feature per family, the
section-8 demand gate applies per family; a fourth family reuses the
waist and adds only vocabulary, projection, and an action name); the
explicit non-goal: no generic `state:` verb, no shared state-family
trait. Matcher vocabulary ownership stays with ADR-0072 — section 14
claims none of it. ADR-0069 is human-ratified, so the amendment is
flagged for human ratification in the park report. `CONTEXT-MAP.md`
ADR index gains the amendment line;
`crates/camel-integration-test/CONTEXT.md` gains the four glossary
terms (traffic adapter, state adapter, steering axis, poll driver) —
lint-context-citations requires the CONTEXT.md anchor.

## Affected crates

- `camel-integration-test`: new `steering.rs` and `runner/poll.rs`;
  `sql_action.rs`, `surreal_action.rs`, `runner/sql_validate.rs`,
  `runner/surreal_validate.rs`, `runner/partner_validate.rs` now call
  the seams; `lib.rs` re-export source moves.
- Docs only: `docs/adr/0069-integration-tier-testing-contract.md`
  (section 14), `CONTEXT-MAP.md` (index line),
  `crates/camel-integration-test/CONTEXT.md` (glossary).

## Architecture boundaries

No runtime crate touched (Runtime, DSL, Components, Services,
Languages, Functions all untouched). The test-workspace boundary from
ADR-0069 section 10 holds: the waist lives inside
`camel-integration-test`; `camel-matchers` purity (ADR-0072) holds; the
publish topology (ADR-0055) is untouched — no dependency edges change.

## Alternatives considered

- One waist module instead of two: rejected — the seams have different
  gates (resolver needs `sql`/`surreal`; driver needs any family) and
  different callers; one module would couple its cfg attributes.
- Driver in `camel-matchers`: rejected — ADR-0072 purity rule; the
  driver is tokio-dependent.
- A `StateAdapter` trait unifying SQL and Surreal: rejected — one step
  from the forbidden generic `state:` verb, and the rule-of-three waist
  needs only functions, not a type hierarchy.
- Rewriting per-family poll prose in the spec: rejected — ADDED-only
  delta; the canonical requirements stay the single source of truth.
