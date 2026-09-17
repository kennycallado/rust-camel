## ADDED Requirements

### Requirement: Nested-cargo tests are color-env insensitive

Every camel-cli test that spawns a nested `cargo` process and parses
its output SHALL be immune to `CARGO_TERM_COLOR=always` in the
environment (CI exports it workflow-wide). Immunity SHALL be achieved
by pinning `CARGO_TERM_COLOR=never` on the spawned cargo child and/or
stripping ANSI escapes at parse sites (the feature_profiles pattern,
bd rc-k6dln), or, where a test parses no cargo-transit output, by a
documented immunity verdict in the audit. The audit SHALL cover:
compile_command_test, compiled_artifact_test, job_one_shot_test,
job_signal_test, lint_test_doc_skip, new_test,
run_empty_discovery_test, run_exec_guard_test, run_signal_test.

#### Scenario: Color-always environment does not break output parsing

- **GIVEN** a camel-cli test from the audit set that parses child
  process output, run with `CARGO_TERM_COLOR=always` exported
- **WHEN** the test executes its assertions against captured stdout
- **THEN** the assertions pass without ANSI contamination (pin applied
  and/or escapes stripped), verified by running the full camel-cli
  suite with `CARGO_TERM_COLOR=always` exported

#### Scenario: Immunity verdicts are recorded

- **GIVEN** a test in the audit set whose parsed output never
  transits a cargo process
- **WHEN** the audit records its verdict
- **THEN** the bd rc-0omir closure note states the test as immune with
  the concrete reason (no cargo child in the parsed path, or camel's
  own piped output carries no ANSI under test capture)
