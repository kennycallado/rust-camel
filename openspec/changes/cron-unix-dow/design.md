# Design: cron-unix-dow

## Approach

Decision: **translate** the numeric day-of-week field from Unix semantics to
cron-crate ordinals. Keep day names working unchanged.

Rationale. The component's contract is Unix 5-field cron. The crate docs, the
component docs page, and the Cargo description all say "Unix 5-field cron
expression", and `docs/src/components/cron.md` advertises numeric ranges
(`1-5`) and lists (`1,3,5`). Rejecting numeric dow would invalidate
expressions the docs advertise and break crontab lines users paste in.
Translation is local to one component crate, needs no SPI change, and
converts the failure mode from "fires on the wrong day" to "explicit error
at endpoint creation" (for out-of-range values only).

The `cron` crate numbers dow `1-7` with `1 = Sunday`. Unix numbers dow `0-7`
with `0` and `7 = Sunday`. The mapping for a single numeric value `v` is
`v % 7 + 1`: `0 → 1` (Sun), `1 → 2` (Mon), `6 → 7` (Sat), `7 → 1` (Sun).
`*` and `*/n` pass through untouched: both semantics anchor the implicit base
on Sunday (Unix `0`, crate `1`), so the selected day sets already coincide.
Names pass through: the crate maps `MON` to Monday consistently inside its own
ordinal space, so name expressions are already correct today.

Algorithm, applied to the dow field only:

1. Split the dow field on `,`.
2. For each token, split off an optional `/step` suffix first. The step, if
   present, must be a nonzero positive integer; `/0` is rejected.
3. Classify the part before the step:
   - `*` (with or without step): pass through with its step untouched. Both
     semantics anchor the implicit base on Sunday (Unix `0`, crate `1`), so
     the selected day sets already coincide.
   - Only letters (names such as `MON`, `MON-FRI`): pass through with its
     step untouched. The crate maps `MON` to Monday consistently inside its
     own ordinal space, so named expressions are already correct today.
     `MON-FRI/2` is therefore valid and stays valid.
   - Numeric range or value (`a`, `a-b`, with `a`, `b` in `0..=7`):
     validate, then apply the step over the source range BEFORE mapping:
     `a` expands to `[a]`; `a-b` expands to `a, a+1, ..., b`; `a/s` expands
     to `a, a+s, ...` up to the field maximum `7`; `a-b/s` expands to
     `a, a+s, ...` up to and including `b`. Map each value with `v % 7 + 1`,
     dedupe preserving order, and emit a comma-joined list of ordinals.
     Expansion (not endpoint mapping) keeps wrapped ranges correct: `6-7`
     becomes `7,1`, `0-7` becomes the full set.
   - Rejected forms: a value outside `0..=7` (for example `8`) with an
     endpoint-creation error naming the day-of-week field and the `0-7`
     range; a descending source range (`6-0`); a zero step (`1/0`); and a
     token with mixed numeric and named endpoints (`1-FRI`), because the two
     ordinal spaces differ and the form is ambiguous.
4. Reassemble: `"0 "` + minute + hour + dom + month + translated dow.

Scope note: this change fixes day-of-week semantics only. Classic Unix cron
treats restricted DOM and DOW with OR semantics; the crate intersects them
(`cron-0.17.0/src/schedule.rs:72-88`). Schedules that restrict both day
fields keep today's intersection behavior. That difference is out of scope
here and must not be promised away in docs.

`normalized_cron_expression` changes from `String` to
`Result<String, CamelError>`. Both call sites (`validate` and the consumer
`start`) already produce `CamelError`, so error propagation is direct. The
normalized expression stays an internal detail; `CronSchedule.expression`
keeps receiving the 6-field crate format.

Boundary cases required in tests: `0`, `1`, `6`, `7`, `8`; wrapped range
`6-7`; full range `0-7`; step forms `1-5/2`, `1/2`, `*/2`; named forms
`MON-FRI`, `MON-FRI/2`, list `1,FRI`; dedup pair `0,7`; meaning-changing
schedule `0 2 * * 1` (bd repro). Calendar tests assert complete firing
sequences across one anchored week (Monday 2026-11-02 through Sunday
2026-11-08, UTC), not a single next fire: a lone next-fire check can pass on
an incorrect day-set superset. The regression test for the bd repro asserts
the actual next-fire instant: build `cron::Schedule` from the normalized
expression, take `after(&base).next()`, and assert weekday and clock time.
Parse success alone cannot catch a translated-but-wrong mapping.

## Affected crates

- `camel-component-cron`: dow translation in `normalized_cron_expression`,
  fallible signature, new tests; doc fix in the stale test comment that
  claims `"0 9 * * 1"` means Mondays (it will after this change).
- Docs (no crate change): `crates/components/camel-cron/CONTEXT.md` gains the
  dow semantics statement as crate authority; `docs/src/components/cron.md`
  Schedule format section cites it.

## Architecture boundaries

Change is contained in the Components layer, inside one component crate.
No `camel-component-api` type or SPI change: `CronSchedule.expression` keeps
its meaning (crate-format expression). Runtime, DSL, Services, and Languages
are untouched. A custom `CronService` receives correct expressions because
translation happens at config normalization, not inside `TokioCronService`.

## Alternatives considered

1. Reject numeric dow, require names. Rejected: contradicts the documented
   Unix contract, breaks advertised ranges and lists, loses compatibility
   with pasted crontab lines for no behavioral gain.
2. Translate inside `TokioCronService`. Rejected: scheduling semantics belong
   to config normalization and validation; SPI backends would otherwise
   duplicate or miss the fix.
3. Keep crate semantics and document `1 = Sunday`. Rejected: this is the
   reported bug. The component's whole surface advertises Unix semantics;
   documenting away the contract fails existing users silently.

## Risk budget

The translation must not alter `*`, names, or the first four fields. Failure
mode is a clear validation error. For schedules that restrict only dow (dom
unrestricted), a validated schedule must fire on the same days as a Unix
crontab with the same expression and timezone. Schedules restricting both
dom and dow keep the crate's intersection semantics (documented out of
scope above); complete Unix parity for that combination is not promised.
