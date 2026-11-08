# surreal-arm-tests

Pin the surrealdb-types 4.0 exhaustiveness arms in `surreal_value_to_cell` with table tests (bd rc-ivtr6, mission 526).

## Why

Trivial change — no spec breakdown needed. Mission 519 added the 4.0 `Number` widths (`I8/U8/I16/U16/I32/U64`, `F16`/`F32`), the `BigDecimal` fail-closed arm, the split temporal family (`Date`/`Time`/`Timestamp`) and the `Interval` fail-closed arm; the fixture-pinned server (v3.1.4) never emits them, so they ship untested.

## What changes

- Add table tests to the existing `mapping` module of `surreal_validate_test.rs`: integer widths min/max (incl. `u64::MAX` surviving), `F16`/`F32` finite→JSON numbers, `F16`/`F32` NaN and ±∞ failing closed naming field and kind, `BigDecimal` failing closed, `Date`/`Time`/`Timestamp` stringifying as `value.to_string()`, `Interval` failing closed.
- Tests only: no production code change, no new dependency — fixtures build from the `surrealdb::types` re-exports (`f16`, `BigDecimal`, `Date::from_days`, `Time::from_nanos`, `Timestamp::from_parts`, `Interval: FromStr`).
