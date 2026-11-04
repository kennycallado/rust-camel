# Proposal: cron-unix-dow

## Why

The cron component advertises Unix 5-field schedules (`min hour dom month dow`).
It normalizes a 5-field expression by prepending a seconds field and hands the
dow field unchanged to the `cron` crate (0.17). The crate numbers days of week
`1-7` with `1 = Sunday`. Unix numbers them `0-7` with `0` and `7 = Sunday`,
`1 = Monday`. A numeric schedule therefore fires on the wrong day, silently.
Repro from bd rc-fa4sd: `cron:nightly?schedule=0 2 * * 1` runs on Sunday, not
Monday.

## What Changes

- Translate the numeric day-of-week field from Unix semantics to cron-crate
  ordinals inside `CronConfig::normalized_cron_expression`
  (`crates/components/camel-cron/src/lib.rs`). Values `0` and `7` map to
  Sunday; `1-6` shift to crate ordinals `2-7`. Ranges, lists, and steps map
  value by value. Day names (`MON`) pass through unchanged.
- Reject day-of-week values outside `0-7` with an endpoint-creation error that
  names the field and the allowed range.
- Add regression tests that assert the actual next-fire instant for numeric
  dow schedules, not just parse success. Cover `0`, `1`, `6`, `7`, `8`, ranges
  that change meaning (`1-5`, `6-7`), steps, names, and the bd repro schedule.
- Document Unix dow semantics in the crate `CONTEXT.md` and in
  `docs/src/components/cron.md`.

Excluded: changes to `camel-component-api`, the `CronService` SPI, or any
other component.

## Acceptance criteria

- `cron:t?schedule=0 2 * * 1` passes validation and its next fire lands on a
  Monday at 02:00 in the configured timezone (asserted on the instant).
- `0` and `7` select Sunday; `1` selects Monday; `6` selects Saturday.
- `8` fails validation with a clear error naming day-of-week and `0-7`.
- `1-5`, `6-7`, `0-7`, `*/2`, `1-5/2`, `1/2`, and names (`MON`, `MON-FRI`,
  `MON-FRI/2`) keep their Unix calendar meaning, asserted as complete firing
  sequences across one anchored week.
- All existing tests keep passing; no new warnings under `cargo clippy -p
  camel-component-cron -- -D warnings`.

## Risk budget

Acceptable: a stricter validation error at endpoint creation for values
outside `0-7`, zero steps, descending ranges, and mixed name/number dow
tokens. Not acceptable: any dow-only schedule that passes validation but
fires on a different day than a Unix crontab would. Scope is the dow field
only: the crate intersects restricted dom and dow where Unix cron ORs them;
that existing behavior is out of scope and stays unchanged. Translation
stays inside the component crate; no SPI or public API change.

Refs: bd rc-fa4sd.
