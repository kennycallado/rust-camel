# Tasks: test-junit-filters

Single-phase driver change (design D1–D6). Tasks execute in order; T1
introduces the writer + report types, T2 the driver refactor, T3 filters,
T4 junit wiring, T5 CLI dispatch + docs.

## Task 1: junit report types and writer

- **ID**: junit-writer

**Files**:
- `crates/camel-cli/src/commands/test/junit.rs` (new)
- `crates/camel-cli/src/commands/test.rs` (modified: add `mod junit;` next
  to existing `mod beans; pub mod document; pub mod runner;`)

**Steps**:
1. Create `junit.rs` with `#![forbid(unsafe_code)]`-matching crate style:
   - `pub(crate) struct DocReport { pub path: PathBuf, pub rows:
     Vec<crate::commands::test::runner::EndpointResult>, pub doc_error:
     Option<String> }` — one attempted document (rows reuse the runner's
     `EndpointResult` so row labels are the PASS/FAIL labels verbatim).
   - `pub(crate) struct ExpansionReport { pub name: String, pub error:
     String }` — one expansion-level error; `name` is the path string
     from the error message.
   - `fn escape_xml(text: &str) -> String` — escape `& < > " '` as
     `&amp; &lt; &gt; &quot; &apos;`; REMOVE characters XML 1.0 forbids
     in content: control chars other than `\t` (0x09), `\n` (0x0A),
     `\r` (0x0D), plus the non-characters U+FFFE and U+FFFF. Do not
     escape the allowed trio.
   - `pub(crate) fn write_report(path: &Path, expansion:
     &[ExpansionReport], reports: &[DocReport]) -> Result<(), String>` —
     write!-based, de-facto JUnit shape per design D3:
     `<?xml version="1.0" encoding="UTF-8"?>` then
     `<testsuites tests=N failures=F errors=E>` where per suite
     `tests` = rows + doc_error testcase (0 or 1) + expansion testcase,
     `failures` = rows with `Err` outcome, `errors` = doc_error +
     expansion testcases. Each row renders
     `<testcase name="{endpoint}" classname="{path}">` and on failure
     `<failure message="{first line of detail}">{full detail}</failure>`
     (first line = text up to and excluding the first `\n`). Each suite
     with `doc_error` renders one extra
     `<testcase name="&lt;document&gt;" classname="{path}">` holding
     `<error message="{first line}">{full}</error>` (literal testcase
     name is `<document>`, escaped in output). Each expansion entry
     renders its own suite `name="{name}"` with a single
     `<testcase name="&lt;expansion&gt;">` + `<error>`. All text and
     attribute values pass through `escape_xml`. Trailing newline after
     `</testsuites>`.
2. Unit tests inside `junit.rs` (`mod tests`) — in-process, no I/O beyond
   a temp file for the write path.

**Tests** (all `command: cargo test -p camel-cli --lib commands::test::junit`):
- `escape_xml_escapes_five_and_strips_controls`
  - setup: none
  - action: call `escape_xml("a<b>&\"'c\u{0001}\u{0007}d\te\nf\u{FFFE}\u{FFFF}")`
  - assert: result is `a&lt;b&gt;&amp;&quot;&apos;cd\te\nf` (controls
    and U+FFFE/U+FFFF removed, tab/LF kept)
  - command: `cargo test -p camel-cli --lib commands::test::junit`
  - expected: fails before implementation exists
- `write_report_all_pass_golden`
  - setup: `DocReport { path: "a.test.yaml", rows: [EndpointResult{
    endpoint: "out", outcome: Ok(()) }], doc_error: None }`,
    expansion empty
  - action: `write_report` to a temp file; read bytes
  - assert: bytes equal the exact string
    `<?xml version="1.0" encoding="UTF-8"?>\n<testsuites tests="1" failures="0" errors="0">\n<testsuite name="a.test.yaml" tests="1" failures="0" errors="0">\n<testcase name="out" classname="a.test.yaml" />\n</testsuite>\n</testsuites>\n`
  - expected: fails before implementation
- `write_report_failure_doc_error_expansion_golden`
  - setup: one report with `rows: [Err("mismatch: line1\nline2")]` on
    endpoint `<settle>` (literal), `doc_error: None`; one report with
    `rows: []`, `doc_error: Some("boot failed\ncause")`; one
    `ExpansionReport { name: "./empty", error: "no test documents found" }`
  - action: `write_report`; read bytes
  - assert: bytes contain `<failure message="mismatch: line1">mismatch: line1\nline2</failure>`
    inside a `<testcase name="&lt;settle&gt;"` element (attributes
    beyond `name` not asserted here), the doc-error suite holds
    `<error message="boot failed">boot failed\ncause</error>` inside a
    `<testcase name="&lt;document&gt;"` element, the expansion suite
    holds a `<testcase name="&lt;expansion&gt;"` element with an
    `<error` child; root attributes `tests="3" failures="1"
    errors="2"`
  - expected: fails before implementation
- `write_report_write_failure_is_err`
  - setup: path pointing inside a non-existent directory
  - action: `write_report`
  - assert: `Err(String)` mentioning the path
  - expected: fails before implementation

**Acceptance**:
- `cargo test -p camel-cli --lib commands::test::junit` passes
- `cargo clippy -p camel-cli -- -D warnings` exits 0
- `cargo fmt --check` clean

- [x] junit-writer

## Task 2: driver refactor — run_tests_full + byte-identity regression

- **ID**: driver-refactor

**Files**:
- `crates/camel-cli/src/commands/test.rs` (modified)

**Steps**:
1. Add `#[derive(Debug, Default)] pub struct TestRunConfig { pub files:
   Vec<PathBuf>, pub junit: Option<PathBuf>, pub filter_files:
   Vec<glob::Pattern>, pub filter_endpoints: Vec<String> }`
   (`Default` yields the no-flags config).
2. Refactor the body of `run_tests` into
   `pub async fn run_tests_full(config: &TestRunConfig, out: &mut dyn
   Write, err: &mut dyn Write) -> TestRunSummary`. T2 scope: config
   fields `junit`, `filter_files`, `filter_endpoints` may be present in
   the type but are IGNORED in this task (wired in T3/T4). Behavior with
   an empty config equals today's `run_tests` exactly: same expansion,
   same prints, same exit-code derivation.
3. Change `expand_test_paths` to return errors as structured
   `(PathBuf, String)` pairs (path + message) instead of pre-formatted
   strings; format `"{path}: {msg}"` at the print site so stdout stays
   byte-identical. During the loop, collect `Vec<junit::DocReport>`
   (one per attempted document, in order) and
   `Vec<junit::ExpansionReport>` (one per expansion error, `name` =
   the pair's path rendered with `.display()`). NOTE: `EndpointResult`
   is NOT `Clone` — print each row by reference while counting
   (`match &er.outcome`), then MOVE `result.endpoint_results` into the
   `DocReport` after the print loop. `doc_error` strings clone as
   today. Collection is side-effect free.
4. `run_tests(files, out, err)` becomes a thin wrapper constructing an
   empty `TestRunConfig` (files only) and calling `run_tests_full`. The
   ~20 existing tests in `mod tests` and `tests/test_runner.rs` /
   `tests/test_replies.rs` / `tests/test_beans.rs` /
   `tests/test_intercepts.rs` / `tests/test_repository_stubs.rs` compile
   and pass UNCHANGED.
5. Add the exact-byte regression (design D5): fixed 3-document corpus —
   `a.test.yaml` passing (1 endpoint), `b.test.yaml` failing (1
   endpoint), `bad.test.yaml` unparsable — run via `run_tests`, assert
   the COMPLETE stdout equals
   `PASS {a}#out\nFAIL {b}#out — MockEndpoint 'out': expected 2 exchanges, got 1\n1 passed, 1 failed\n`
   (that is the real assert.rs wording; b declares `count: 2` and
   receives 1) and stderr equals `{bad}: {serde_yaml parse text}\n`
   (capture the actual serde_yaml error text once from a scratch run
   and pin it verbatim), exit code 2.

**Tests** (command: `cargo test -p camel-cli --lib commands::test`):
- `no_flags_output_is_byte_identical`
  - setup: temp dir with the 3-document corpus above (a: `to: mock:out`
    + `expects: {mock:out: {count: 1}}`; b: same routes but
    `count: 2` so it fails; bad: `{{{ not yaml`)
  - action: `run_tests(&[dir], &mut out, &mut err)`
  - assert: exact stdout/stderr strings above; the corpus is invoked
    via the temp DIRECTORY argument, so displayed paths are the
    absolute temp-path strings — pin the actuals; exit 2
  - expected: fails before the regression string is pinned (new test)
- existing suite unchanged: `cargo test -p camel-cli --lib
  commands::test` all green; `cargo test -p camel-cli --test
  test_runner --test test_replies` green

**Acceptance**:
- `cargo test -p camel-cli --lib` passes (includes pre-existing ~206 lib
  tests)
- `cargo test -p camel-cli --test test_runner --test test_replies` passes
- `cargo clippy -p camel-cli -- -D warnings` exits 0

- [x] driver-refactor

## Task 3: filters

- **ID**: filters

**Files**:
- `crates/camel-cli/src/commands/test.rs` (modified)

**Steps**:
1. In `run_tests_full`, after expansion and BEFORE reading any document:
   when `filter_files` is non-empty, keep a document iff its ENTIRE
   displayed-path string matches ANY pattern via
   `pattern.matches_with(displayed, glob::MatchOptions {
   require_literal_separator: true, ..glob::MatchOptions::new() })` —
   glob 0.3 takes `MatchOptions` by value.
   If any filter (files or endpoints) is non-empty and the post-file-filter
   set is empty → stderr misuse error naming all filter values (same
   format as the post-loop check), then fall through to the normal tail
   (summary line `0 passed, 0 failed` still prints, exit 2); no
   documents run.
2. Per file-admitted document: read + parse as today. Parse failure →
   report error (stderr), mark `had_parse_error`, COUNT AS SURVIVOR
   (produce its DocReport with doc_error; no zero-survivor error from
   it). Parse success + `filter_endpoints` non-empty → run iff
   `doc.expects` keys contain ANY given name; filtered-out documents
   produce no rows, no counts, no DocReport.
3. After the loop: if any filter was given and NO document was a
   survivor (a survivor = file-admitted document that either failed to
   parse, or parsed and passed the endpoint filter), emit the misuse
   error to stderr and set exit 2. Track survivorship with an explicit
   `any_survivor: bool` — do not infer it from row counts (a survivor
   document may legally produce zero rows when it declares no
   `expects`).
4. Track survivor-visibility so the misuse error names filters exactly:
   format `no test documents matched --filter-file {f1} {f2}
   --filter-endpoint {e1}` with only the given kinds.

**Tests** (command: `cargo test -p camel-cli --lib commands::test`):
- `filter_file_separator_semantics` (spec scenario: file glob narrows)
  - setup: two parseable docs `a.test.yaml` and `sub/b.test.yaml`
    passed as explicit file args (plain paths — no `./` prefix)
  - action: `run_tests_full` with `filter_files: [Pattern("*.test.yaml")]`
  - assert: stdout contains `a.test.yaml#out` and NOT
    `sub/b.test.yaml`; summary `1 passed, 0 failed`; exit 0
  - command: `cargo test -p camel-cli --lib commands::test`
  - expected: fails before implementation
- `filter_file_applies_before_reading` (spec scenario)
  - setup: one doc `bad.test.yaml` (unparsable) + filter `'other*'`
  - action: `run_tests_full` with `filter_files: [Pattern("other*")]`
  - assert: stderr holds ONLY the zero-survivors misuse error (no read
    or parse error for bad.test.yaml — the file was never opened),
    stdout ends with the `0 passed, 0 failed` summary, exit 2
  - command: `cargo test -p camel-cli --lib commands::test`
  - expected: fails before implementation
- `filter_endpoint_selects_expects_keys` (spec scenario)
  - setup: two parseable docs; only one declares
    `expects: {orders: {count: 1}}`
  - action: `run_tests_full` with `filter_endpoints: ["orders"]`
  - assert: only the orders doc produces rows; the other silent
    (no lines, no counts); exit 0
  - command: `cargo test -p camel-cli --lib commands::test`
  - expected: fails before implementation
- `filters_compose_and` (spec scenario)
  - setup: docs `sub/one.test.yaml` (expects orders) and
    `two.test.yaml` (expects orders), invoked via a literal `.`
    directory arg so displayed paths carry the `./` prefix
  - action: `run_tests_full` with `filter_files: [Pattern("./sub/**")]`
    + `filter_endpoints: ["orders"]`
  - assert: only `./sub/one.test.yaml` runs (its PASS line appears;
    `two.test.yaml` absent from stdout)
  - command: `cargo test -p camel-cli --lib commands::test`
  - expected: fails before implementation
- `zero_survivors_is_misuse` (spec scenario)
  - setup: one parseable doc
  - action: `run_tests_full` with `filter_endpoints: ["nosuch"]`
  - assert: stderr names `--filter-endpoint nosuch`; stdout holds the
    usual trailing summary `0 passed, 0 failed` (misuse follows the
    expansion-error pattern: stderr error + normal summary + exit 2);
    exit code 2
  - command: `cargo test -p camel-cli --lib commands::test`
  - expected: fails before implementation
- NOTE: the "invalid glob is misuse" spec scenario is covered at the
  argument-validation layer in T5 (`config_from_args` unit +
  `invalid_glob_e2e_writes_no_report` binary-level test); a compiled
  `TestRunConfig` can never carry an invalid pattern, so T3 has no
  duplicate test for it.
- `parse_error_survives_endpoint_filter` (spec scenario)
  - setup: `bad.test.yaml` (unparsable) + `ok.test.yaml` (expects
    orders)
  - action: `run_tests_full` with `filter_endpoints: ["orders"]`
  - assert: bad's parse error IS on stderr, the ok doc's rows are on
    stdout, and no zero-survivor error appears; exit 2
  - command: `cargo test -p camel-cli --lib commands::test`
  - expected: fails before implementation

**Acceptance**:
- `cargo test -p camel-cli --lib commands::test` passes
- `cargo clippy -p camel-cli -- -D warnings` exits 0

- [x] filters

## Task 4: junit wiring

- **ID**: junit-wiring

**Files**:
- `crates/camel-cli/src/commands/test.rs` (modified)

**Steps**:
1. In `run_tests_full`, after the loop and exit-code derivation: when
   `config.junit` is `Some(path)`, call
   `junit::write_report(path, &expansion_reports, &doc_reports)`. On
   `Err(e)`: stderr `failed to write {path}: {e}`, force exit code 2
   (override only if current code < 2). Write happens on exit-0/1/2 runs
   alike — the report is written AFTER the human summary line, from the
   collected reports.
2. Write policy: the ONLY no-report path is T5's `config_from_args`
   failure (invalid glob — main exits before `run_tests_full`).
   Zero-survivor misuse and expansion-error runs DO write the report:
   zero-survivors yields `<testsuites tests="0" failures="0"
   errors="0">`, expansion errors yield their synthetic suites
   (spec: report written on exit-0/1/2 runs alike; expansion errors
   appear as suites).
3. Report totals contract (proposal AC): root `tests` attribute equals
   `passed + failed + error-testcases`; `failures` equals `failed`;
   `errors` equals error-testcase count.

**Tests** (command: `cargo test -p camel-cli --lib commands::test`):
- `junit_all_pass_report` (spec scenario)
  - setup: one passing doc, `junit: Some(tmp/"r.xml")`
  - action: `run_tests_full`
  - assert: file exists; bytes start with the XML declaration; contains
    `tests="1" failures="0" errors="0"`; exit 0
  - command: `cargo test -p camel-cli --lib commands::test`
  - expected: fails before implementation
- `junit_failure_detail` (spec scenario)
  - setup: one failing doc (count mismatch), junit set
  - action: `run_tests_full`
  - assert: `<failure` element present with the mismatch text as element
    body and first-line `message=`; exit 1; report written
  - command: `cargo test -p camel-cli --lib commands::test`
  - expected: fails before implementation
- `junit_document_error_and_expansion` (spec scenarios: document error
    lands + counts + expansion suite)
  - setup run 1: `ok.test.yaml` passing + `bad.test.yaml` unparsable,
    invoked via file args; `junit` set
  - action run 1: `run_tests_full`
  - assert run 1: report holds the passing suite + an `errors="1"`
    suite with `<testcase name="&lt;document&gt;"` and `<error`;
    exit 2
  - setup run 2: directory arg on an EMPTY dir (expansion error:
    `no test documents found`) with `junit` set to a second path
  - action run 2: `run_tests_full`
  - assert run 2: report EXISTS, holds exactly one synthetic suite
    named by the directory's displayed path with a single
    `<testcase name="&lt;expansion&gt;"` + `<error` child; root
    `tests="1" errors="1"`; exit 2
  - command: `cargo test -p camel-cli --lib commands::test`
  - expected: fails before implementation
- `junit_filtered_documents_have_no_rows` (spec: filtered-out documents
    produce no JUnit rows)
  - setup: two parseable docs, `filter_endpoints: ["orders"]` admits
    one; `junit` set
  - action: `run_tests_full`
  - assert: report holds exactly ONE suite (the survivor's); no element
    referencing the filtered-out document's path; exit 0
  - command: `cargo test -p camel-cli --lib commands::test`
  - expected: fails before implementation
- `junit_escapes_settle_label` (spec scenario: escaping, end-to-end)
  - setup: one doc whose matcher-mismatch failure detail contains `<`
    and `&` (e.g. `bodies: [{equals: "<a&b>"}]` against a different
    received body)
  - action: run with `junit: Some(tmp/"r.xml")`
  - assert: exit 1; report written; element body contains `&lt;` and
    `&amp;` where the detail carried `<` and `&`; no raw `<a&b>`
    sequence outside tag structure (byte containment: `&lt;a&amp;b&gt;`
    present)
  - command: `cargo test -p camel-cli --lib commands::test`
  - expected: fails before implementation
- `junit_zero_survivors_writes_empty_report` (write policy)
  - setup: one parseable doc, `filter_endpoints: ["nosuch"]`,
    `junit: Some(tmp/"r.xml")`
  - action: `run_tests_full`
  - assert: exit 2 (zero-survivor misuse) AND `r.xml` EXISTS holding
    `<testsuites tests="0" failures="0" errors="0">` with no suite
    children (the report is written on exit-2 runs; only the
    invalid-glob preflight skips it)
  - command: `cargo test -p camel-cli --lib commands::test`
  - expected: fails before implementation
- `junit_write_failure_forces_exit_2` (write policy)
  - setup: a passing document; `junit: Some(<nonexistent-dir>/r.xml)`
  - action: `run_tests_full`
  - assert: stderr holds a `failed to write` message naming the path;
    exit code 2 (overridden from the would-be 0); stdout still printed
    the PASS lines and summary before the failure
  - command: `cargo test -p camel-cli --lib commands::test`
  - expected: fails before implementation
- `junit_absent_writes_nothing` (spec scenario)
  - setup: the SAME 3-document corpus as T2's byte-identity regression
    (pass + fail + doc error), run twice: once plain `run_tests`, once
    `run_tests_full` with `junit: None`
  - action: both runs, capturing output
  - assert: no file exists at the would-be path after either run; both
    runs' stdout and stderr are byte-identical to each other and to the
    pinned T2 strings
  - command: `cargo test -p camel-cli --lib commands::test`
  - expected: fails before implementation

**Acceptance**:
- `cargo test -p camel-cli --lib commands::test` passes
- `cargo clippy -p camel-cli -- -D warnings` exits 0

- [x] junit-wiring

## Task 5: CLI flags, dispatch, docs

- **ID**: cli-docs

**Files**:
- `crates/camel-cli/src/commands/test.rs` (modified)
- `crates/camel-cli/src/main.rs` (modified: dispatch lines ~169–174)
- `docs/src/testing/index.md` (modified)
- `crates/camel-cli/CONTEXT.md` (modified — extend the "camel test
  failure modes" table unconditionally)

**Steps**:
1. Extend `TestArgs`: `#[arg(long)] pub junit: Option<PathBuf>`,
   `#[arg(long = "filter-file", value_name = "GLOB")] pub filter_files:
   Vec<String>`, `#[arg(long = "filter-endpoint", value_name = "NAME")]
   pub filter_endpoints: Vec<String>`.
2. Add `pub(crate) fn config_from_args(args: &TestArgs) ->
   Result<TestRunConfig, String>` (pub(crate) so `main.rs` can call it):
   compile each glob with `glob::Pattern::new`; on failure
   return `Err("invalid --filter-file pattern {glob}: {e}")`. Build the
   config (files, junit, compiled patterns, endpoint names).
3. `main.rs` dispatch: build config via `config_from_args`; on `Err(e)`:
   `eprintln!("{e}")`, `std::process::exit(2)` — this happens BEFORE
   any document runs and before any report path is touched. Otherwise
   call `run_tests_full(&config, &mut out, &mut err)` and exit with the
   summary code. `run_tests` wrapper remains for
   library/in-process callers and tests.
4. `docs/src/testing/index.md`: add a `### CI output and filters`
   subsection INSIDE `## Declarative camel test`, after the `###
   Repository stubs` subsection: (a) `--junit <FILE>`
   semantics — one suite per document, per-row testcases with the same
   labels as PASS/FAIL lines, `<failure>`/`<error>` mapping, written on
   exit 0/1/2, not written on invalid-filter misuse; (b)
   `--filter-file <GLOB>` with glob semantics warning (`*` does not
   cross `/`, `**` does; directory args display `./`-prefixed paths);
   (c) `--filter-endpoint <NAME>` exact match on expects keys; (d) a
   sharding recipe: split by `--filter-file './src/**/shard-N*'` style
   with one example command; (e) note filters AND across kinds, OR
   within repeats, zero survivors exits 2. (f) a note that annotating
   PRs from the report requires the CI platform's JUnit publisher or
   report-ingest integration (GitHub Actions example: actions/upload-
   artifact + a JUnit action, one sentence, no specific action
   endorsement).
5. `crates/camel-cli/CONTEXT.md`: it carries a "camel test failure
   modes" table this change extends — add rows for zero-survivor
   misuse (exit 2, names filters) and junit write failure (stderr +
   exit 2), and mention the three flags wherever the test-command
   surface is described.

**Tests**:
- `invalid_glob_config_is_misuse` (spec scenario: invalid glob)
  - setup: `TestArgs { files: vec!["."], junit: None, filter_files:
    vec!["[".into()], filter_endpoints: vec![] }`
  - action: `config_from_args(&args)`
  - assert: `Err` containing `invalid --filter-file pattern`
  - command: `cargo test -p camel-cli --lib commands::test`
  - expected: fails before implementation
- `valid_flags_build_config`
  - setup: args with `filter_files: vec!["*.test.yaml".into()]`,
    `filter_endpoints: vec!["orders".into()]`, `junit: Some("r.xml")`
  - action: `config_from_args`
  - assert: `Ok` config carries one compiled pattern, one endpoint,
    junit path set
  - command: `cargo test -p camel-cli --lib commands::test`
  - expected: fails before implementation
- `invalid_glob_e2e_writes_no_report` (spec scenario: invalid filter
    flag writes no report)
  - setup: spawn the built `camel` binary (sibling harness convention)
    with `<corpus-dir> --junit <tmp>/r.xml --filter-file '['`
  - action: run to completion
  - assert: exit code 2; stderr contains `invalid --filter-file
    pattern`; `<tmp>/r.xml` does NOT exist
  - command: `cargo test -p camel-cli --test test_junit_flags`
  - expected: fails before implementation
- `no_flag_dispatch_byte_identical_e2e`
  - setup: spawn `env!("CARGO_BIN_EXE_camel")` with args
    `<corpus-dir>` (the T2 3-doc corpus: pass + fail + bad), no flags
  - action: run to completion; capture stdout/stderr/exit
  - assert: stdout equals the pinned T2 string (absolute displayed
    paths), stderr equals the pinned parse-error line, exit code 2 —
    proving the main.rs dispatch through `run_tests_full` preserves
    no-flag CLI byte-identity end-to-end
  - command: `cargo test -p camel-cli --test test_junit_flags`
  - expected: fails before implementation (file lands with T5)
- `dispatch_help_lists_flags` (CLI-surface smoke)
  - setup: none
  - action: `std::process::Command::new(env!("CARGO_BIN_EXE_camel"))
    .args(["test", "--help"])` (the bin name is `camel`, pinned by
    camel-cli's `[[bin]]` and the sibling harness)
  - assert: stdout contains `--junit`, `--filter-file`,
    `--filter-endpoint`
  - command: `cargo test -p camel-cli --test test_junit_flags` (new
    integration file)
  - expected: fails before the file lands with this task
- docs: manual verification step in Acceptance (grep), not a test

**Acceptance**:
- `cargo test -p camel-cli --lib` passes; `cargo test -p camel-cli
  --test test_junit_flags` passes
- `grep -c 'filter-file' docs/src/testing/index.md` ≥ 1
- `cargo clippy -p camel-cli -- -D warnings` exits 0
- `cargo fmt --check --all` clean

- [x] cli-docs
