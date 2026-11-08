# Proposal: sql-any-decode-fidelity

## Why

Retro-audit 2026-11-08 (`.opencode/fleet/inbox/reference/retro-audit-2026-11-08.md`,
reviewer `r_gpt`, findings re-verified by the conductor against sqlx 0.9 source)
confirmed four defects in the sql-any-type-matrix landing (`21f63dc91`, follow-up
of rc-s7lb0; bd stays closed):

1. **Finite `REAL` fails the route.** `row_to_json`
   (`crates/components/camel-sql/src/utils.rs:18-73`) tries `i64`, `i32`, `f64`,
   `bool`, `String`, `Vec<u8>`, raw-null, then the typed error.
   `AnyRow::try_get` enforces strict type compatibility
   (`sqlx-core-0.9.0/src/row.rs:112-127`); for the Any driver, `f64` (Double) is
   refused for a `REAL` (Real) column. No branch decodes `REAL`, so
   `SELECT 1.5::real` fails a route the README matrix advertises as supported.
2. **Streaming paths lose the typed error.** `producer.rs` `StreamList` (~:395)
   and `consumer.rs` (~:249) wrap `row_to_json` errors in
   `ProcessorError("JSON serialization failed: …")`, so `TypeConversionFailed`
   survives only outside streaming.
3. **The NULL guarantee is over-broad.** `README.md:332-333` says SQL NULL
   decodes to JSON `null` "for every column type". False:
   `AnyColumn::try_from(&PgColumn)` rejects any type outside the Any set
   (`sqlx-postgres-0.9.0/src/any.rs:190-210`), so a projection containing
   `NULL::numeric` fails at row construction with the typed error, before any
   null handling.
4. **CHAR is advertised but unsupported.** `README.md:340` lists
   "TEXT, VARCHAR, CHAR, CITEXT". The Any mapper maps only
   `Text | Varchar | citext`; `PgType::Bpchar` is absent, so CHAR fails at row
   construction.

## What Changes

- `camel-component-sql/src/utils.rs`: new `f32` branch in `row_to_json` after the
  `f64` branch, mirroring it: finite value → JSON number via `f64::from(v)`;
  NaN/±Infinity → the same typed `undecodable_column_error`. The branch body is
  a named helper so it is unit-testable without a server.
- `camel-component-sql/src/producer.rs` (`StreamList`) and
  `camel-component-sql/src/consumer.rs` (streaming consumer): propagate the
  `row_to_json` error unchanged; keep the `serde_json::to_vec` wrapper as is.
- Docs (`crates/components/camel-sql/README.md`, `docs/src/components/database.md`):
  narrow the NULL guarantee to the supported set and state the typed-error
  behavior for other types; move CHAR from the supported string row to the
  unsupported list with the `CAST(col AS text)` hint; note REAL decodes as f32
  widened to f64, finite values only.
- Tests first: five integration tests in `crates/camel-test/tests/sql_test.rs`,
  two unit tests in `utils.rs`, and one unit test on `poll_database_stream` in
  `consumer.rs`. Red-first evidence is captured on the pre-fix tree. Tests that
  lock behavior the docs mis-describe (non-finite REAL, unsupported typed NULL,
  CHAR) are expected green-on-arrival at the integration level; this is
  recorded per test, not forced red. Streaming coverage observes the actual
  defect site: the StreamList integration test uses a non-UTF-8 BYTEA (fetches
  fine, fails inside `row_to_json` at the wrapped line) and drains the received
  `Body::Stream` itself; the consumer's `poll_database_stream` wrapper removal
  is covered by a unit test (NUMERIC inputs fail at fetch, before the wrapped
  line, and `useIterator` never enters `poll_database_stream`).

Affected crates: `camel-component-sql`, `camel-test` (integration tests), `docs`.

## Acceptance criteria

- `SELECT 1.5::real AS r UNION ALL SELECT 2.5::real AS r` through a sql producer
  route returns the JSON numbers 1.5 and 2.5 (not strings, not an error).
- `SELECT 'NaN'::real AS r`, `SELECT NULL::numeric AS n`,
  `SELECT 'ab'::char(2) AS c` → typed `TypeConversionFailed` naming the column,
  the driver-reported type, and the CAST hint; never a substituted null.
- `outputType=StreamList` (producer and consumer) surfaces the same typed error;
  the message never carries the `JSON serialization failed` wrapper.
- No surviving over-broad doc claim: `rg -n 'every column type|CHAR'` on the
  README and `docs/src/components/database.md` shows zero `every column type`
  hits; standalone `CHAR` only in the unsupported typed-error row (`VARCHAR`
  substring and charset prose hits are expected and ignored).
- Gates: `cargo fmt --check --all`, `cargo clippy -p camel-component-sql
  --all-targets -- -D warnings`, `cargo xtask lint-unwrap`,
  `cargo xtask lint-log-levels`, `cargo test -p camel-component-sql --lib`,
  full `--test sql_test` (Docker), `cd docs && mdbook test` (docs changed).
  `lint-commits` skipped per mission containment.
- Existing typed-error assertions are not weakened.

## Risk budget

Small, additive. REAL decodes where it previously failed; streaming errors keep
their type instead of a generic wrapper; two doc tables become accurate. Out of
bounds: no new URI parameter, no driver change, no schema change, no
retry/backoff changes, no bridge-script changes, no weakening of existing
typed-error assertions.
