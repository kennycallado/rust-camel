# Tasks: scenario-sql-diagnostics

## OpenSpec

### Task 1.1: Restore the normative CLI flavor requirement

**Files:**
- `openspec/specs/cli-feature-profiles/spec.md` (modified)

**Steps:**
1. Reproduce the strict validation failure for `Slim marker feature naming`.
2. Replace `camel-cli's slim marker feature is` with `camel-cli's slim marker feature SHALL be`.
3. Preserve all feature names and remaining requirement text. Beads: rc-y15du.

**Tests:**
- Run `openspec validate cli-feature-profiles --type spec --strict --no-interactive` before and after the edit.
  Before: exit 1 with the missing SHALL/MUST error. After: exit 0.
- Run `openspec validate --specs --strict --no-interactive`; require 128 passing specs.

**Acceptance:** The targeted and full validators pass. The diff changes only the normative verb.

- [x] 1.1

## Scenario harness and CLI

### Task 2.1: Filter scenario console SQLx events and label SQL action rows

**Files:**
- `crates/camel-integration-test/src/log_capture.rs` (modified)
- `crates/camel-cli/src/commands/test/scenario.rs` (modified)
- `crates/camel-cli/tests/test_scenario_cli_e2e.rs` (modified)
- `crates/camel-integration-test/CONTEXT.md` (modified)
- `docs/src/testing/scenario-sql.md` (modified)

**Steps:**
1. Require the baseline regular CLI build to finish successfully.
   From `examples/integration-testing`, run the worktree's `target/debug/camel test partner-sql.test.yaml`.
   Record exit 0 and `7 passed, 0 failed` before changing production code.
2. Write the three regression specifications below. Demonstrate a console-policy failure before the production fix.
3. Extract the existing subscriber composition into a private constructor that accepts `MakeWriter`.
   Production passes `std::io::stdout`. Tests pass a reopened temporary file.
4. Apply `Targets` only to the formatter: default TRACE, `sqlx` prefix INFO. Preserve the unfiltered capture layer.
   Add no ambient configuration reads or dependency.
5. Add `A::Sql { .. } => "sql"` to `action_label`.
6. Update the existing subscriber comment, add a harness CONTEXT section describing console versus capture,
   and document the fixed SQL console policy with a link to that authority. Keep fixture verification prose for Task 3.1.
   Beads: rc-mjkgp.

**Tests:**
- `sql_validate_e2e_pass`: use `write_sql_validate_doc` with `[[1, "alice"], [2, "bob"]]`.
  Spawn the real CLI with `test <doc>` in its temporary directory.
  Require success, `#scenario[0] sql`, `#scenario[1] validate`, and `2 passed, 0 failed`.
  Combined output must omit `CREATE TABLE seeded`, `INSERT INTO seeded`, and `SELECT id, name FROM seeded`.
- `sql_debug_logs_remain_captured`: use that setup and append `logs:\n  noLevelAbove: trace\n`.
  Spawn the CLI with child `RUST_LOG=off`.
  Require exit 1, `2 passed, 1 failed`, `#logs`, `logs.noLevelAbove`, and `DEBUG sqlx::query`.
  Require the same query-text exclusions.
- `console_filters_sqlx_without_filtering_capture`: create `tempfile::NamedTempFile` and pass its reopened `std::fs::File` to the production constructor.
  Use a scoped dispatch and an open capture window. Read the temporary file after emission.
  Emit distinct marker messages at all five SQLx levels plus another target at TRACE.
  Console: SQLx TRACE/DEBUG absent, the other four markers present. Capture: all six markers present.
  Do not assert exact capture length because other open windows can emit concurrently.
- Existing `logs_violation_fails_document_at_cli`: require exit 1, `1 passed, 1 failed`, and WARN.
- Run `cargo test -p camel-integration-test --lib console_filters_sqlx_without_filtering_capture`.
- Run `cargo test -p camel-cli --features itest-e2e --test test_scenario_cli_e2e sql_` and require four executed SQL tests.
  The source file is gated by `itest-e2e`; zero tests do not count as a pass. Run
  `cargo test -p camel-cli --test test_scenario_logs_violation`.

**Acceptance:** All listed regressions pass after a demonstrated red test. No registry-wide filter or new public API exists.

- [x] 2.1

## Examples and documentation

### Task 3.1: Verify the combined fixture and remove stale verification markers

**Files:**
- `examples/integration-testing/partner-sql.test.yaml` (modified)
- `examples/integration-testing/README.md` (modified)
- `docs/src/testing/scenario-sql.md` (modified)

**Steps:**
1. Rebuild the regular CLI after Task 2.1 and run the exact combined fixture again from its example directory.
2. Require exit 0 and `7 passed, 0 failed`. Then remove the fixture's TODO-verify header and corresponding prose in the README and guide.
3. Keep the example's meaning precise: scripted HTTP partner assertions and seeded SQL state assertions share one scenario.
   The HTTP requests do not populate the SQL table. Beads: rc-zmpbp.
4. Run the documentation gates and final repository quality checks from this worktree.

**Tests:**
- `../../target/debug/camel test partner-sql.test.yaml` from `examples/integration-testing`: require seven passing actions and no failed action.
- `nix shell nixpkgs#mdbook -c mdbook build docs`: exit 0.
- `nix shell nixpkgs#mdbook -c mdbook test docs`: exit 0.
- Check new links resolve and all three stale verification markers are gone.

**Acceptance:** The real fixture passes before prose changes. Documentation gates pass. The run's evidence is recorded in Beads.

- [x] 3.1
