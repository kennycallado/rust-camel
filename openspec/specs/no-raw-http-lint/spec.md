# no-raw-http-lint Specification

## Purpose
TBD - created by archiving change pooled-race-harness. Update Purpose after archive.
## Requirements
### Requirement: Detection of uncompliant HTTP/1.1 response literals in test scope

The `lint-no-raw-http-test-server` xtask command SHALL scan every Cargo workspace
member tree (`crates/`, `scripts/`, `examples/`, `benchmarks/`, `fuzz/`; excluding
build output) and SHALL fail with an error when a configured member
root is missing, rather than silently skipping it. for string or byte-string literals that start with an HTTP/1.x status
line and neither carry a structurally valid `Connection: close` header nor are
supplied through the shared test harness. A status line is matched when, after the
`HTTP/1.0 ` or `HTTP/1.1 ` prefix, the status token begins with three digits OR with
an interpolation placeholder (`{`), covering both `HTTP/1.1 200 OK\r\n…`,
`HTTP/1.1 {status_line}\r\n…`, and `HTTP/1.1 {} {}\r\n…` construction shapes. Matching is
anchored at a line start (after a leading CRLF continuation or byte-order mark), so a
status-line-shaped substring in the middle of a larger literal does not match. A
literal is compliant only when it contains a **structurally valid** `Connection:
close` header: a header line whose name is `connection` (case-insensitive) and whose
value contains the `close` token, positioned before the blank line that terminates
the header block — the token appearing in a body or inside another header's value
does NOT make the literal compliant. The scan SHALL include literals inside macro
token streams (for example `format!`), SHALL exclude literals that are direct
arguments of `starts_with`, `ends_with`, or `contains` receiver calls, SHALL exempt
literals passed as arguments (directly or nested in constructor/tuple/vec arguments)
to `test_support::http` harness constructors (`HttpTestServer::start*`,
`Response::ok`, `Response::with_status`, `Response::with_status_and_headers`,
`Response::raw`, `Response::chunked*`,
`Response::staged`), and SHALL offer no file-wide
or import-based exemptions. Findings are restricted to test scope: `tests/**`
directories, files named `*_test.rs` or `*_tests.rs`, `#[cfg(test)]` modules, and
`#[test]`/`#[tokio::test]` functions.

#### Scenario: plain response literal without close is reported

- **GIVEN** a test-scope file containing `write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")`
- **WHEN** `cargo xtask lint-no-raw-http-test-server` runs over the workspace
- **THEN** that site is reported as a finding with file and line

#### Scenario: literal with a valid Connection close header is not reported

- **GIVEN** a test-scope response literal whose header block contains a
  `Connection: close` header line before the blank line
- **WHEN** the lint scans the file
- **THEN** the site produces no finding

#### Scenario: close token outside a header line does not exempt

- **GIVEN** a test-scope response literal where the text `Connection: close` appears
  only inside the response body or inside another header's value, with no Connection
  header line in the header block
- **WHEN** the lint scans the file
- **THEN** the site is still reported as a finding

#### Scenario: format macro literal is reported

- **GIVEN** a test-scope `format!("HTTP/1.1 302 Found\r\nLocation: {loc}\r\nContent-Length: 0\r\n\r\n")`
- **WHEN** the lint scans the file
- **THEN** the literal inside the macro token stream is reported

#### Scenario: dynamic whole-status placeholder is reported

- **GIVEN** a test-scope literal starting `"HTTP/1.1 {status_line}\r\n"` with no
  valid Connection close header
- **WHEN** the lint scans the file
- **THEN** the site is reported as a finding

#### Scenario: dynamic status-and-reason placeholders are reported

- **GIVEN** a test-scope literal starting `"HTTP/1.1 {} {}\r\n"` filled by `format!`
  with a status code and reason, with no valid Connection close header
- **WHEN** the lint scans the file
- **THEN** the site is reported as a finding

#### Scenario: assertion-site literal is not reported

- **GIVEN** a test-scope expression `head.starts_with(b"HTTP/1.1 200 OK\r\n")`
- **WHEN** the lint scans the file
- **THEN** the literal produces no finding because it is a direct argument of
  `starts_with`

#### Scenario: request literal is not reported

- **GIVEN** a test-scope client literal `"GET /x HTTP/1.1\r\nHost: h\r\n\r\n"`
- **WHEN** the lint scans the file
- **THEN** the literal produces no finding because it begins with a method token,
  not a status line

#### Scenario: harness-constructor argument is exempt

- **GIVEN** a test-scope literal without close passed as an argument to
  `Response::raw(...)` or `HttpTestServer::start(...)` from `test_support::http`
- **WHEN** the lint scans the file
- **THEN** the literal produces no finding because it is harness-supplied, and no
  file-wide or import-based exemption was involved

### Requirement: Marker escape hatch for adjudicated exceptions

A finding SHALL be suppressed when the line immediately above the literal carries a
`// allow-raw-http: <non-empty reason>` marker. The marker SHALL suppress only the
site it precedes. An empty reason SHALL NOT suppress the finding. The command output
SHALL list every marker location and the total marker count so gate reports show
adjudicated residuals.

#### Scenario: marked site is suppressed

- **GIVEN** a response literal without close preceded by
  `// allow-raw-http: benchmark loadgen CLI test; standalone std server`
- **WHEN** the lint scans the file
- **THEN** that site produces no finding

#### Scenario: marker with empty reason does not suppress

- **GIVEN** a response literal preceded by `// allow-raw-http:`
- **WHEN** the lint scans the file
- **THEN** the site is still reported

#### Scenario: marker locations and count appear in the report

- **GIVEN** a workspace with two suppressed sites
- **WHEN** the lint runs
- **THEN** the output lists both marker locations and reports a marker count of 2

### Requirement: Seeded monotone ratchet with fail-closed read

Enforcement SHALL use a soft count ratchet: `scripts/xtask/ratchet-raw-http-test-server.max`
holds the maximum allowed count of unadjudicated findings. The ceiling SHALL only
decrease over time (monotone); a run with findings above the ceiling SHALL fail, and
a missing or malformed ratchet file SHALL be an error (fail-closed). This change
SHALL seed the ratchet at the measured baseline when the lint lands and lower it to
0 before the change completes.

#### Scenario: findings above ceiling fail the gate

- **GIVEN** a ratchet ceiling of 1 and an unadjudicated finding count of 2
- **WHEN** the lint runs
- **THEN** the command exits non-zero and names the ratchet file

#### Scenario: missing ratchet file fails closed

- **GIVEN** `scripts/xtask/ratchet-raw-http-test-server.max` does not exist
- **WHEN** the lint runs
- **THEN** the command exits non-zero with an error about the missing ratchet

#### Scenario: count at or below ceiling passes

- **GIVEN** a ratchet ceiling of 1 and an unadjudicated finding count of 1
- **WHEN** the lint runs
- **THEN** the command exits zero

### Requirement: Gate registration in CI and AGENTS registry

The `lint-no-raw-http-test-server` command SHALL be registered as a quality gate in
the AGENTS.md `## QUALITY GATES` registry and SHALL run in CI alongside the other
xtask class lints.

#### Scenario: gate appears in the registry and CI

- **GIVEN** the repository's AGENTS.md and CI workflow files
- **WHEN** the gate registry and lint workflow are inspected
- **THEN** `cargo xtask lint-no-raw-http-test-server` is listed in both

