# Proposal: sql-any-type-matrix

## Why

bd rc-s7lb0 (P2): the SQL component routes every query through sqlx's `Any` driver
(`AnyPool`, `install_default_drivers`). Two data-fidelity defects follow from types the
`Any` type set does not cover (NUMERIC, TIMESTAMPTZ and everything else outside
BOOLEAN/SMALLINT/INTEGER/BIGINT/REAL/DOUBLE/TEXT/BLOB):

1. **Non-actionable fetch failure.** On sqlx 0.9 a NUMERIC/TIMESTAMPTZ column fails at
   fetch time — `AnyRow` conversion raises `ColumnDecode` with source
   "Any driver does not support the Postgres type PgTypeInfo(Numeric)" — and the
   producer/consumer wrap it as `ProcessorError("Query execution failed: …")`. The
   message names the column and type but gives no workaround, so routes fail on every
   poll with nothing to act on.
2. **Silent null substitution in `row_to_json`.** When a column value survives the fetch
   but cannot decode into any tried Rust type, `utils.rs` substitutes JSON `null` behind
   a WARN ("Could not decode column value, falling back to null"). The same silent-null
   substitution fires for non-UTF-8 `BYTEA` values and for non-finite REAL/DOUBLE values
   (NaN/±Infinity through `Number::from_f64` → `unwrap_or(Null)`). A null in the body is
   data loss disguised as a value: the server sent a value and the route sees none.

## What Changes

- `camel-component-sql/src/utils.rs`:
  - Factor `undecodable_column_error(column, type_name)` returning
    `CamelError::TypeConversionFailed` naming the column, the driver-reported type, and
    the CAST workaround. Unit-tested without a server.
  - `row_to_json`: the undecodable fallback, the non-UTF-8 `BYTEA` branch, and the
    non-finite float branch return that typed error. Real SQL NULL stays JSON `null`
    (existing `Ok(None)` branch plus a raw-value null check so a NULL of a
    non-string-decodable kind never triggers the error).
  - New `fetch_error` helper used at all fetch sites (producer SelectList/SelectList
    fetch, both StreamList paths, consumer poll): rewrites the sqlx unsupported-type
    `ColumnDecode`/`AnyDriverError` into the same actionable error; other sqlx errors
    keep the current `ProcessorError` shape.
- Docs: one short type matrix table in `crates/components/camel-sql/README.md` and
  `docs/src/components/database.md` — what the `Any` path decodes today, the gaps
  (NUMERIC, TIMESTAMPTZ, and other unlisted types) with the CAST workaround.
- Tests: unit tests for the error builders; integration tests in
  `crates/camel-test/tests/sql_test.rs` (feature `integration-tests`, `support::postgres`):
  NUMERIC and TIMESTAMPTZ selects produce the typed error with column and type (never a
  null body), non-UTF-8 BYTEA produces the typed error (on `main` it returns a null
  body — the silent-null demonstration), a plain `NULL::text` still comes back as
  `null`, and the polling consumer surfaces one clear bridged error instead of a null
  row.

Affected crates: `camel-component-sql`, `camel-test` (integration tests), `docs`.

## Acceptance criteria

- Undecodable column → typed error naming column, driver type, and CAST workaround; never
  a substituted null.
- SQL NULL → JSON `null` (unchanged; the new error cannot fire on it).
- Polling consumer → one clear error through `bridgeErrorHandler=true` + dead-letter,
  not a null row.
- Type matrix documented in both docs targets.
- Red-first demonstrated against `main`; gates: fmt, clippy `-D warnings`, lint-unwrap,
  lint-context-citations, mdbook test, `cargo test -p camel-component-sql`,
  `cargo test -p camel-test --features integration-tests --test sql_test` (Docker).

## Risk budget

Small, contained. Deliberate behavior change: non-UTF-8 BYTEA, non-finite floats, and
undecodable types now fail the route instead of yielding null bodies — that is the point
of the fix, documented in the matrix. Out of bounds: no typed (non-`Any`) driver path, no
`AnyPool` sizing or batch bind changes (bd rc-fef4z owns that), no new URI parameter, no
stringifying NUMERIC values as a "fix". A future typed-driver path would decode
NUMERIC/TIMESTAMPTZ natively and retire the CAST workaround; recorded here, not designed.
