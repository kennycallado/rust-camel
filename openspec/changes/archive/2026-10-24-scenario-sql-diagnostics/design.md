# Design: scenario-sql-diagnostics

## Approach

The scenario driver installs `ensure_capture_subscriber` before the full boot.
Its registry combines an unfiltered `CaptureLayer` and a console formatter.
Apply `tracing_subscriber::filter::Targets` to the formatter only.
Use TRACE as the default and INFO for the `sqlx` target prefix.
This reuses the existing dependency and avoids configuration parsing or new public API.
Extract the existing subscriber composition into a private constructor that accepts a `MakeWriter`.
Production passes `std::io::stdout`. The formatter regression passes a temporary file through the same constructor.

Keep `CaptureLayer` unfiltered. SQLx DEBUG events must remain available to `logs:` assertions.
Do not read `RUST_LOG` or change boot-time tracing configuration.
This preserves the no-new-ambient-reads requirement in `integration-tier` and ADR-0069.

Add the `ScenarioAction::Sql` match arm to `action_label`.
Update the existing SQL CLI end-to-end test to require the precise label and suppress ordinary statement output.
Add a subprocess regression with a SQL action and `logs.noLevelAbove: trace`.
Require a verdict failure naming `DEBUG sqlx::query` while the console omits raw query text.
Set `RUST_LOG=off` on that child to verify capture remains independent of ambient settings.

Before changes to production code, build the regular CLI and execute `partner-sql.test.yaml` in its example directory.
The existing `appdb` datasource uses shared in-memory SQLite.
HTTP partners use loopback listeners. No external service is required.
Only an observed seven-action success permits removal of the three verification markers.

The CLI flavor spec correction replaces `is` with `SHALL be` and preserves the feature naming contract.

## Executable regressions

The `test_scenario_cli_e2e` binary requires the `itest-e2e` Cargo feature.
Run its SQL cases with `cargo test -p camel-cli --features itest-e2e --test test_scenario_cli_e2e sql_`.
Require four executed tests. An empty test run provides no evidence.

- `sql_validate_e2e_pass` in `crates/camel-cli/tests/test_scenario_cli_e2e.rs`:
  use `write_sql_validate_doc` with expected rows `[[1, "alice"], [2, "bob"]]` in a temporary directory.
  Run `Command::new(env!("CARGO_BIN_EXE_camel"))` with `test`, the document path, and that working directory.
  Require success, `#scenario[0] sql`, `#scenario[1] validate`, and `2 passed, 0 failed`.
  Require combined stdout/stderr to omit `CREATE TABLE seeded`, `INSERT INTO seeded`, and `SELECT id, name FROM seeded`.
- `sql_debug_logs_remain_captured` in the same file:
  reuse that setup and append `logs:\n  noLevelAbove: trace\n` to the document.
  Run the same command with child `RUST_LOG=off`.
  Require exit code 1, `2 passed, 1 failed`, `#logs`, `logs.noLevelAbove`, and `DEBUG sqlx::query`.
  Require the same SQL-text exclusions. This test rejects a registry-wide filter.
- `console_filters_sqlx_without_filtering_capture` in `crates/camel-integration-test/src/log_capture.rs`:
  create `tempfile::NamedTempFile`, pass its reopened `std::fs::File` to the production constructor, and open a capture window.
  Under its scoped dispatch, emit `sqlx::query` events at TRACE, DEBUG, INFO, WARN, and ERROR.
  Emit one event from `scenario_console_test` at TRACE. Give each event a distinct message marker.
  Read the temporary file after the scoped dispatch. Require it to omit the SQLx TRACE/DEBUG markers and contain the other four markers.
  Require the capture window to contain all six markers. Do not require an exact window length because windows are process-global.
- Existing `logs_violation_fails_document_at_cli` must still report exit 1, `1 passed, 1 failed`, and WARN for its route.

## Affected crates

- `camel-integration-test`: formatter-only target filter and lifecycle documentation.
- `camel-cli`: SQL action label and subprocess regressions.
- Documentation and examples: verified fixture markers and the console verbosity policy.

## Architecture boundaries

The harness owns observation and reporting. Runtime, DSL, Components, Services, Languages, and Functions keep their existing contracts.
[ADR-0069](https://github.com/kennycallado/rust-camel/blob/main/docs/adr/0069-integration-tier-testing-contract.md) governs scenario hermeticity and outcome classes.
[ADR-0051](https://github.com/kennycallado/rust-camel/blob/main/docs/adr/0051-credential-redaction-at-diagnostic-boundaries.md) governs diagnostic redaction. Filtering is a separate concern.

## Alternatives considered

- A registry-wide filter would hide events from assertions.
- Ambient `RUST_LOG` guidance cannot control the current scenario subscriber and would alter its hermetic policy.
- Disabling SQLx logs entirely would hide warnings and errors.
- A new logging configuration option would add an unnecessary public contract.
