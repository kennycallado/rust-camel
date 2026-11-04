# cron-schedule-semantics Specification

## Purpose
TBD - created by archiving change cron-unix-dow. Update Purpose after archive.
## Requirements
### Requirement: Numeric day-of-week follows Unix semantics

The cron component SHALL interpret a numeric day-of-week value with Unix
semantics: `0` and `7` select Sunday, `1` selects Monday, `6` selects
Saturday, and each value `2-5` selects the following weekday in order.

#### Scenario: bd repro schedule fires on Monday

- **GIVEN** the schedule `0 2 * * 1` in the UTC timezone
- **WHEN** the component normalizes the expression and computes the next
  fire after `2026-11-02T12:00:00Z`
- **THEN** the next fire instant is `2026-11-09T02:00:00Z` (Monday)

#### Scenario: Zero and seven both select Sunday with identical sequences

- **GIVEN** the schedules `0 2 * * 0` and `0 2 * * 7`
- **WHEN** the component computes the first two fires after
  `2026-11-02T12:00:00Z` for each schedule
- **THEN** both sequences are `2026-11-08T02:00:00Z` and
  `2026-11-15T02:00:00Z` (Sundays)

#### Scenario: Six selects Saturday

- **GIVEN** the schedule `0 2 * * 6`
- **WHEN** the component computes the next fire after
  `2026-11-02T12:00:00Z`
- **THEN** the next fire instant is `2026-11-07T02:00:00Z` (Saturday)

### Requirement: Day-of-week names keep their calendar meaning

The cron component SHALL pass day-of-week name tokens, including named
ranges and named ranges with numeric steps (`MON`, `MON-FRI`, `MON-FRI/2`,
any capitalization accepted by the crate), through translation unchanged, so
a named expression keeps its calendar meaning.

#### Scenario: MON schedule fires on Monday

- **GIVEN** the schedule `0 2 * * MON`
- **WHEN** the component computes the next fire after
  `2026-11-02T12:00:00Z`
- **THEN** the next fire instant is `2026-11-09T02:00:00Z` (Monday)

#### Scenario: MON-FRI fires exactly Monday through Friday

- **GIVEN** the schedule `0 2 * * MON-FRI`
- **WHEN** the component computes the first five fires after
  `2026-11-02T12:00:00Z`
- **THEN** the fires are Tuesday through Friday of the anchor week at
  02:00 UTC, then Monday 2026-11-09 at 02:00 UTC, and no other day

#### Scenario: MON-FRI with step stays valid and fires stepped weekdays

- **GIVEN** the schedule `0 2 * * MON-FRI/2`
- **WHEN** the component validates the schedule and computes the first
  three fires after `2026-11-02T12:00:00Z`
- **THEN** validation succeeds and the fires are Wednesday 2026-11-04,
  Friday 2026-11-06, and Monday 2026-11-09 at 02:00 UTC

### Requirement: Day-of-week operators preserve Unix meaning

The cron component SHALL translate numeric ranges, lists, and steps in the
day-of-week field value by value, including wrapped ranges, so the selected
day set matches Unix semantics. A numeric step expands over the source range
before mapping; `a/s` expands from `a` to the field maximum `7`.

#### Scenario: Work-week range fires exactly Monday through Friday

- **GIVEN** the schedule `0 9 * * 1-5`
- **WHEN** the component computes the first five fires after
  `2026-11-02T12:00:00Z`
- **THEN** the fires are Tuesday through Friday of the anchor week at
  09:00 UTC, then Monday 2026-11-09 at 09:00 UTC, and no other day

#### Scenario: Wrapped range fires Saturday and Sunday only

- **GIVEN** the schedule `0 2 * * 6-7`
- **WHEN** the component computes the first three fires after
  `2026-11-02T12:00:00Z`
- **THEN** the fires are Saturday 2026-11-07, Sunday 2026-11-08, and
  Saturday 2026-11-14 at 02:00 UTC, and no other day

#### Scenario: Full range fires every day exactly once per day

- **GIVEN** the schedule `0 2 * * 0-7`
- **WHEN** the component computes the first seven fires after
  `2026-11-02T12:00:00Z`
- **THEN** the fires are one per calendar day from Tuesday 2026-11-03
  through Monday 2026-11-09 at 02:00 UTC, each day exactly once

#### Scenario: Stepped range fires Monday, Wednesday, Friday

- **GIVEN** the schedule `0 2 * * 1-5/2`
- **WHEN** the component computes the first four fires after
  `2026-11-02T12:00:00Z`
- **THEN** the fires are Wednesday 2026-11-04, Friday 2026-11-06,
  Monday 2026-11-09, and Wednesday 2026-11-11 at 02:00 UTC

#### Scenario: Single value with step expands to the field maximum

- **GIVEN** the schedule `0 2 * * 1/2`
- **WHEN** the component computes the first three fires after
  `2026-11-02T12:00:00Z`
- **THEN** the fires are Wednesday 2026-11-04, Friday 2026-11-06, and
  Sunday 2026-11-08 at 02:00 UTC (expansion `1,3,5,7` includes Sunday
  through value `7`)

#### Scenario: Star with step fires the Unix stepped day set

- **GIVEN** the schedule `0 2 * * */2`
- **WHEN** the component computes the first four fires after
  `2026-11-02T12:00:00Z`
- **THEN** the fires are Tuesday 2026-11-03, Thursday 2026-11-05,
  Saturday 2026-11-07, and Sunday 2026-11-08 at 02:00 UTC

#### Scenario: Mixed numeric and name list keeps both days

- **GIVEN** the schedule `0 2 * * 1,FRI`
- **WHEN** the component computes the first three fires after
  `2026-11-02T12:00:00Z`
- **THEN** the fires are Friday 2026-11-06, Monday 2026-11-09, and
  Friday 2026-11-13 at 02:00 UTC

#### Scenario: Duplicate Sunday values collapse to Sunday-only behavior

- **GIVEN** the schedule `0 2 * * 0,7`
- **WHEN** the component computes the first two fires after
  `2026-11-02T12:00:00Z`
- **THEN** the fires are Sunday 2026-11-08 and Sunday 2026-11-15 at
  02:00 UTC, and no non-Sunday day fires

### Requirement: Out-of-range and ambiguous day-of-week tokens are rejected

The cron component SHALL reject at endpoint creation, with an error naming
the day-of-week field, a value outside `0-7`, a descending source range, a
zero step, and a token with mixed numeric and named endpoints. The
component SHALL NOT fall back to crate interpretation for these forms.

#### Scenario: Eight is rejected

- **GIVEN** the URI `cron:test?schedule=0 2 * * 8`
- **WHEN** the endpoint is created
- **THEN** creation fails with an error mentioning day-of-week and `0-7`

#### Scenario: Mixed endpoints are rejected

- **GIVEN** the URI `cron:test?schedule=0 2 * * 1-FRI`
- **WHEN** the endpoint is created
- **THEN** creation fails with an explicit ambiguity error

#### Scenario: Descending range is rejected

- **GIVEN** the URI `cron:test?schedule=0 2 * * 6-0`
- **WHEN** the endpoint is created
- **THEN** creation fails with an error naming the day-of-week field

#### Scenario: Zero step is rejected

- **GIVEN** the URI `cron:test?schedule=0 2 * * 1/0`
- **WHEN** the endpoint is created
- **THEN** creation fails with an error naming the day-of-week field

