# Design: test-junit-filters

## Context

Driver-layer ergonomics for the unit tier defined by ADR-0064 (two-tier
testing contract). The change touches only the `camel-cli` driver:
`commands/test.rs` (args, expansion, driver loop) and a new pure module
`commands/test/junit.rs`. No camel-core, component, or DSL changes; the
hexagonal boundary is untouched (the driver already owns printing and exit
codes — this extends the same control-plane responsibility).

## D1 — Flag surface (clap `TestArgs`)

- `--junit <FILE>`: `Option<PathBuf>`. Report path; `-` is NOT special
  (a plain file; stdout stays the human channel).
- `--filter-file <GLOB>`: `Vec<String>`, repeatable. Compiled once with
  `glob::Pattern::new` (dep already in camel-cli) and matched with
  `require_literal_separator: true` (see D2). Invalid pattern is a
  misuse error: message to stderr, exit 2, no documents run, no report.
- `--filter-endpoint <NAME>`: `Vec<String>`, repeatable. Exact string vs
  `expects` keys (the bare mock endpoint name, URI suffix after `mock:`).

## D2 — Filter semantics

Applied after `expand_test_paths` (expansion errors keep their existing
meaning; a zero-document directory is still misuse). Order:

1. `--filter-file`: `Pattern::matches_with(path, MatchOptions {
   require_literal_separator: true, ..MatchOptions::new() })` (options
   by value — glob 0.3 signature)
   — the crate's default `matches()` lets `*` cross `/`, which would
   contradict the specified semantics (`*` does not cross `/`; `**`
   still does — the crate special-cases the double-star component).
   Matched against the ENTIRE displayed-path
   string (the same string the driver prints — note a `.` argument yields
   `./a.test.yaml`, so globs must account for the `./` prefix). Applied
   BEFORE reading: file-filtered-out documents are never read or parsed,
   so their potential parse errors do not surface. Any-of (OR) across
   repeats.
2. `--filter-endpoint`: applied per file-admitted document AFTER parse. A
   document runs iff its `expects` map contains at least one given name
   (OR across repeats). A file-admitted document that fails to parse
   still reports its error and sets exit 2 regardless of the endpoint
   filter — broken documents stay visible, and it counts as a survivor
   for zero-survivor accounting.
3. Both kinds present: AND.
4. Zero surviving documents (and at least one filter given): misuse error
   naming the filters, exit 2.

Filtered-out documents produce no stdout lines, no counts, and no junit
rows — they did not run.

## D3 — JUnit mapping

De-facto JUnit schema (as ingested by Jenkins/GitLab/GitHub annotations):

```
<testsuites tests=N failures=F errors=E>
  <testsuite name="<doc path>" tests=... failures=... errors=...>
    <testcase name="<endpoint|reply[i] <to>>" classname="<doc path>">
      [<failure message="first line of detail">full detail</failure>]
    </testcase>
    [<testcase name="<document>" classname="<doc path>">
      <error message="first line of detail">full detail</error>]
  </testsuite>
</testsuites>
```

- Row identity reuses the driver's row labels verbatim (`endpoint` field,
  `reply[i] <input.to>`, `<settle>`), so annotations match stdout lines.
- `message` attribute = first line of the detail text (CI preview);
  element text = full detail.
- Document-level errors (read, parse, boot, route load, input delivery)
  map to one `<error>` testcase named `<document>`; the testsuite still
  appears with `errors="1"`.
- Expansion-level errors (unreadable directory entry, zero-document
  directory) map to one synthetic testsuite named by the path in the
  error message with a single `<error>` testcase named `<expansion>` —
  CI annotates them like any other error.
- Attribute totals on both levels: `tests` = testcase count,
  `failures` = `<failure>` count, `errors` = `<error>` count
  (`tests = passed + failed + errors` against the summary counters).
  `time` omitted (no timing infrastructure; the attribute is optional in
  the schema).
- Write policy: flags validate first (invalid glob pattern → stderr
  message, exit 2, NO report written, no documents run). After
  validation, the report is written on exit-0, exit-1, and exit-2 runs
  alike. Report write failure: stderr message, exit 2.

## D4 — Writer

Hand-rolled `write!`-based writer with a single `escape_xml()` helper
(`& < > " '` plus REMOVAL of characters XML 1.0 forbids in content:
control characters other than tab, LF, CR — failure details are arbitrary
route/matcher text and must never break well-formedness), ~70 lines, no
new dependencies. Rejected `quick-xml`: the writer is smaller than the
integration. Correctness locked by a golden byte-level unit test plus an
escaping unit test (the real-world `<settle>` label guarantees angle
brackets in failure text).

## D5 — Driver refactor

- New `run_tests_full(config: &TestRunConfig, out, err) -> TestRunSummary`
  where `TestRunConfig = { files, junit: Option<PathBuf>, filter_files:
  Vec<glob::Pattern>, filter_endpoints: Vec<String> }`. The loop collects
  `Vec<DocReport>` (path, rows with outcome detail, doc error) while
  printing exactly as today; junit.rs consumes the reports after the
  loop. `expand_test_paths` returns structured `(PathBuf, String)` pairs
  (formatted at the print site, byte-identical stdout) which feed
  synthetic `<expansion>` reports. The ONLY no-report path is
  `config_from_args` failure (invalid glob, exits before the run);
  zero-survivor and expansion-error runs write the report too.
- `run_tests(files, out, err)` remains as a thin wrapper (no filters, no
  junit) so the ~20 existing driver unit tests compile unchanged.
- `main.rs` dispatch ALWAYS goes through `run_tests_full`; with no flags
  the config is empty and the code path is the wrapper's. Signature
  retention alone does not prove CLI byte-identity — an exact-byte
  regression test pins it: a fixed multi-document corpus (pass + fail +
  document-error) asserting the complete expected stdout and stderr
  strings with no flags set. The "file glob narrows the run" spec
  scenario doubles as the separator-semantics regression:
  `*.test.yaml` must NOT match `sub/b.test.yaml` under
  `require_literal_separator: true`.

## D6 — Documentation

`docs/src/testing/index.md` gains a "CI output and filters" section
(flag surface, glob semantics, junit mapping, sharding recipe with one
shard example). `crates/camel-cli/CONTEXT.md` notes the new flags if its
surface list mentions the test command.

## Phases

Single-phase change (driver-only, ~5 tasks, no subsystem splits).
