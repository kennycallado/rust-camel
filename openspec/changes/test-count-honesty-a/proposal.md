# Proposal: test-count-honesty-a

## Why

`camel test` counts only rows it produced. A document that fails at the
document level produces no rows, so it contributes nothing to `passed` /
`failed` — and, for scenario documents, to no total at all. The unit path
records such documents in the parse-error channel
(`crates/camel-cli/src/commands/test.rs:683-704` pushes into
`parse_error_names`), but the scenario path
(`crates/camel-cli/src/commands/test.rs:643-652`) sends the error to stderr
and the JUnit `<error>` only. stdout then prints the bare
`N passed, M failed`, and `TestRunSummary.parse_errors` stays 0.

Consequences, both real:

- `openspec/specs/mock-testkit/spec.md:505` already classifies boot failures
  as parse-class errors and requires the additive summary segment
  `, N parse-error docs (skipped)`; the scenario path violates the spec
  today.
- The JUnit contract `tests = passed + failed + errors`
  (`mock-testkit/spec.md:1092`) no longer matches stdout, which is what CI
  annotations read.

A scenario document that never booted therefore looks green to anything
reading stdout or the counts. bd: rc-19b09 (step A; step B — declared case
counts, `not run` segment — is rc-ypk3e and out of scope here).

## What Changes

- The scenario `doc_error` branch in `run_tests_full` also records the
  document in the parse-error channel, exactly like the unit branch:
  one `parse_error_names.push(...)`. The stderr line and the JUnit
  `<error>` stay as they are; the row list stays empty. The unit path is
  untouched.
- A regression test at the driver level (`driver_tests.rs`): one passing
  unit document plus one unbootable scenario document (missing route file),
  asserting exit code 2, the stdout summary with the parse-error suffix
  naming the scenario document, and the JUnit suite keeping one `<error>`
  testcase with the spec-formula suite counts.
- No spec delta: the spec already requires this exact classification; the
  implementation is brought into compliance. `skip_specs: true` in
  `.openspec.yaml`.

Explicitly excluded (step B, rc-ypk3e): declared case counts, a `not run`
segment, any JUnit shape change, changes to `run_scenario_doc`, the settle
row, or the scenario runner's early-stop behaviour.

## Acceptance criteria

- A boot-failed scenario document makes stdout carry the parse-error
  suffix and stderr name the document; exit code stays 2.
- A clean run (no parse errors) prints byte-identical output as before.
- Passing documents are unaffected: no behaviour change in either branch
  when `doc_error` is `None`.
- `cargo test -p camel-cli` green including the new regression test.

## Risk budget

Small and surgical: one push in the driver, one test. Risk is limited to
count/summary drift, which the existing summary tests pin. Acceptable:
`TestRunSummary.parse_errors` counts scenario boot failures (intended).
Out of bounds: step B work, JUnit shape changes, scenario runner changes.

Affected crates: `camel-cli` (`crates/camel-cli/src/commands/test.rs`,
`crates/camel-cli/src/commands/test/driver_tests.rs`).
