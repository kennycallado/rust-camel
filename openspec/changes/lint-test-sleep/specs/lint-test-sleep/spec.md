# lint-test-sleep Specification (delta)

## ADDED Requirements

### Requirement: Advisory detection of sleeps in test function bodies

The `lint-test-sleep` xtask command SHALL scan Rust source files in every
Cargo workspace member tree (crates/, scripts/, examples/, benchmarks/, fuzz/;
excluding build output) for functions annotated `#[test]` or `#[tokio::test]`
(including attribute arguments such as `flavor = "multi_thread"`) and SHALL
report every `tokio::time::sleep` or `std::thread::sleep` call that appears
directly in the test function body, as an advisory finding with file, line,
and a summary count.

#### Scenario: Blocking sleep in a plain test is reported

- **GIVEN** a source file containing `#[test] fn t() { std::thread::sleep(std::time::Duration::from_millis(100)); }`
- **WHEN** `cargo xtask lint-test-sleep` runs over the workspace
- **THEN** the report lists that file and line as a finding, the summary count includes it, and the command exits 0

#### Scenario: Async sleep in a tokio test is reported

- **GIVEN** a source file containing a `#[tokio::test(flavor = "multi_thread")]`
  function whose body awaits `tokio::time::sleep(Duration::from_millis(100))`
- **WHEN** the lint scans the file
- **THEN** the call is listed as a finding

#### Scenario: Sleep inside a nested closure is not reported

- **GIVEN** a test function whose body passes a closure containing
  `tokio::time::sleep(...)` to a route builder `.process(...)` or any other
  receiver
- **WHEN** the lint scans the file
- **THEN** the closure's sleep produces no finding, because simulate-work
  inside closures is legitimate

#### Scenario: Sleep inside an async block in the test body is reported

- **GIVEN** a test function whose body contains an async block (not a closure)
  with a direct `tokio::time::sleep(...)` call
- **WHEN** the lint scans the file
- **THEN** the async block is traversed and the sleep is listed as a finding

#### Scenario: Sleep inside a nested fn item is not reported

- **GIVEN** a test function whose body defines a helper `fn` containing
  `tokio::time::sleep(...)`
- **WHEN** the lint scans the file
- **THEN** the helper's sleep produces no finding, because only the test fn
  body itself is scanned

#### Scenario: Sleep in a non-test function is ignored

- **GIVEN** a plain function (no `#[test]`/`#[tokio::test]` attribute)
  containing `tokio::time::sleep(...)`
- **WHEN** the lint scans the file
- **THEN** no finding is produced

### Requirement: Sleep call resolution covers imported and aliased short forms

The lint SHALL resolve fully-qualified paths (`tokio::time::sleep`,
`std::thread::sleep`) and short forms introduced by `use` declarations visible
in the test fn's scope chain (file root module and enclosing modules),
including grouped and aliased imports. A locally-defined symbol with the same
name SHALL shadow the import and prevent flagging. The lint SHALL NOT flag
other timer APIs such as `sleep_until`.

#### Scenario: Short-form sleep via use declaration is reported

- **GIVEN** a test file with `use tokio::time::sleep;` and a test body calling
  `sleep(Duration::from_millis(50)).await`
- **WHEN** the lint scans the file
- **THEN** the call is reported as a finding

#### Scenario: Aliased import is reported

- **GIVEN** a test file with `use tokio::time::sleep as pause;` and a test
  body calling `pause(Duration::from_millis(50)).await`
- **WHEN** the lint scans the file
- **THEN** the call is reported as a finding

#### Scenario: Locally shadowed sleep symbol is not reported

- **GIVEN** a test file with `use tokio::time::sleep;` where the test fn
  defines a local helper `fn sleep(_: Duration) {}` (or binds a local variable
  named `sleep`) and the body calls `sleep(Duration::from_millis(50))`
- **WHEN** the lint scans the file
- **THEN** no finding is produced, because the local binding shadows the import

#### Scenario: sleep_until is not flagged

- **GIVEN** a test body calling `tokio::time::sleep_until(deadline).await`
- **WHEN** the lint scans the file
- **THEN** no finding is produced

### Requirement: Escape hatch suppresses a finding

The lint SHALL suppress a finding when the sleep call line carries a
`// allow-test-sleep: <reason>` comment with non-whitespace reason text,
mirroring the lint-unwrap escape hatch.

#### Scenario: Annotated sleep is suppressed

- **GIVEN** a reported sleep call whose source line ends with
  `// allow-test-sleep: simulates a slow consumer`
- **WHEN** the lint scans the file
- **THEN** that line produces no finding and is excluded from the summary count

#### Scenario: Empty marker does not suppress

- **GIVEN** a reported sleep call whose source line ends with
  `// allow-test-sleep:` (no reason text)
- **WHEN** the lint scans the file
- **THEN** the finding is still produced

### Requirement: Advisory exit semantics with trustworthy-failure reporting

When scanning completes, the command SHALL exit 0 even with findings. A file
that cannot be read or parsed SHALL produce a diagnostic qualified with the
file path and SHALL result in a non-zero exit, because an unparsable file
makes the advisory report untrustworthy. In this rollout phase the command is
not part of the CI quality gates.

#### Scenario: Findings do not fail the command

- **GIVEN** the workspace contains one or more reportable sleeps in test bodies
- **WHEN** `cargo xtask lint-test-sleep` runs
- **THEN** findings and summary are printed on stdout and the process exit
  code is 0

#### Scenario: Unparsable file fails the command

- **GIVEN** a workspace source file whose contents do not parse as valid Rust
- **WHEN** `cargo xtask lint-test-sleep` runs
- **THEN** a diagnostic naming that file path is printed and the process exit
  code is non-zero
