# Design: test-count-honesty-a

## Approach

`run_tests_full` keeps two count channels: rows it produced (`passed` /
`failed`) and the parse-error channel (`parse_error_names`). Document-level
failures produce no rows; the unit branch compensates by pushing the
document into `parse_error_names` (`test.rs:684`), which drives the
additive stdout segment, the stderr naming line, `TestRunSummary
.parse_errors`, and exit code 2. The scenario branch (`test.rs:643-652`)
does everything except that push.

The fix is one line in the scenario `doc_error` branch, mirroring the unit
branch:

```rust
if let Some(doc_error) = result.doc_error {
    had_apparatus = true;
    parse_error_names.push(path.display().to_string());
    let _ = writeln!(err, "{}: {doc_error}", path.display());
    doc_reports.push(junit::DocReport { ... });  // unchanged
    continue;
}
```

Everything observable except the stdout summary segment, the stderr naming
line, `summary.parse_errors`, and the derived exit code stays as-is: the
stderr diagnostic line, the empty row list, and the JUnit `<error>`
testcase are untouched. When `doc_error` is `None`, neither branch changes
behaviour, so passing documents keep byte-identical output.

Exit code: the branch already sets `had_apparatus`, forcing exit 2 before
this change; `had_parse_error` now also forces 2. No observable exit-code
change.

### Mission-order deviation: suite `tests` attribute is `1`, not `0`

The order asks the regression test to assert `tests="0"` for the failed
suite. That contradicts both the spec formula and the shipped writer:
`mock-testkit/spec.md:1092` defines `tests` = testcase count
(`passed + failed + errors`), and a doc-error suite carries exactly one
testcase — the `<document>` error — so the suite must read
`tests="1" failures="0" errors="1"`. The existing golden test
(`junit.rs::write_report_failure_doc_error_expansion_golden`) pins exactly
this shape, and `junit.rs:92` computes
`tests = rows.len() + doc_error.is_some()`. Emitting `tests="0"` would
break the spec formula and that golden test, and changing the JUnit shape
is explicitly step-B material (rc-ypk3e). The test therefore asserts one
`<error>` testcase named `<document>` and the spec-formula counts
`tests="1"`, `errors="1"`, matching the mission's substantive intent (the
JUnit contract agrees with stdout) rather than its literal digit.

### Summary wording with mixed kinds

The message template is `{n} parse-error {doc|docs} (skipped): {names}`
(`test.rs:744-758`). It is kind-agnostic: names are displayed paths in
occurrence order, so a list holding a scenario document reads correctly
(`1 parse-error doc (skipped): bad.test.yaml`). The spec
(`mock-testkit/spec.md:505`) requires exactly this additive segment for
parse-class failures as a class, without distinguishing unit from
scenario. No observable text changes; only the code comment above
`parse_error_names` gains a note that scenario boot failures now flow
through the same channel.

### Spec delta: none

`mock-testkit/spec.md:505` already classifies boot failures as
parse-class errors and mandates the additive segment; the scenario path
simply fails to comply. This change makes the implementation match the
spec, so no delta is written (`skip_specs: true`). Step B (declared case
counts, `not run` segment) will carry its own delta under rc-ypk3e.

### Regression test

Driver-level, in `driver_tests.rs` next to `junit_document_error_and_expansion`
and the `parse_error_*` family, following their style:

- Fixture: temp dir with (a) a minimal `Camel.toml` (`log_level = "info"`,
  same shape as `validate_only_partner_proxies_varying_query`), (b) a
  passing unit document (`write_passing`), and (c) a scenario document
  with a valid boot-selecting action and a dead route source:

  ```yaml
  routeFiles: [missing.yaml]
  scenario:
  - send:
      to: direct:start
  ```

  `direct` is in `BOOT_SCHEMES` for every feature configuration
  (`scenario.rs:32-83`), and the one non-fake wire ref whose scheme is a
  boot scheme selects the full-boot path (`scenario.rs:363-379`), so the
  boot fails in route discovery (`CamelError::Io` on `missing.yaml`)
  before any context start — deterministic, no real boot, and provably
  the full-boot branch rather than the smoke path.
- Act: `run_tests_full` with `junit: Some(report)`. Invoked under the
  crate's default features (`cargo test -p camel-cli` → `flavor-regular`
  → `integration-http`), like the existing unconditional driver scenario
  tests (`validate_only_partner_proxies_varying_query`).
- Assert: exit code 2; `summary.parse_errors == 1`; stdout ends
  `1 passed, 0 failed, 1 parse-error doc (skipped)`; the stderr diagnostic
  line contains `full-boot-failure` and names `missing.yaml`; the stderr
  naming line names the scenario document; JUnit keeps the scenario suite
  with one `<error>` testcase named `<document>` and the spec-formula
  counts `tests="1" failures="0" errors="1"`; the passing suite keeps its
  row.

## Affected crates

- `camel-cli`: `crates/camel-cli/src/commands/test.rs` (one push + comment),
  `crates/camel-cli/src/commands/test/driver_tests.rs` (one test).

## Architecture boundaries

Stays inside the CLI test driver (Runtime-adjacent tooling): no Runtime,
DSL, Component, Service, Language, or Function surface changes. The lean /
full tier split (ADR-0064) and the scenario boot path
(`run_scenario_doc`, ADR-0069 partner binding) are read, not modified.
The mock-testkit spec canon (`openspec/specs/mock-testkit/spec.md`) is
the normative reference; this change closes the gap between it and the
driver.

## Risk budget

One push and one test. The existing `parse_error_*` and `junit_*` tests
pin the summary and JUnit shapes; a clean run prints byte-identical
output. Failure modes are limited to count drift, which those tests
catch.
