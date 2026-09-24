# Proposal: parsecount

## Why

External report from the camel-cache team (bd rc-ornrp). In a multi-doc
`camel test` run, a document with a YAML/parse error prints its diagnostic
to stderr, but the final summary line ignores it. A run of 218 passing
endpoints plus one unparseable document prints `218 passed, 0 failed`. The
exit code is 2, so scripts that check the exit code are safe. Scripts that
gate on the summary line alone are blind: the skipped document is
invisible. Their case was `proxy-radar-compose.test.yaml` with a comment
line missing `#`.

## What Changes

- The `camel test` summary line gains an additive segment when parse-class
  failures occurred: `218 passed, 0 failed, 1 parse-error doc (skipped)`.
  The segment is absent when no parse-class failure occurred; clean runs
  print `N passed, M failed` exactly as before.
- stderr names each parse-error entry on one line immediately before the
  summary line. The per-failure stderr diagnostics stay byte-identical.
- `TestRunSummary` gains a count field for parse-class failures.
- Spec delta: `Exit codes, reporting, and multi-document execution`
  (mock-testkit) gains the additive-count clause.

Excluded: exit codes (precedence 2 > 1 > 0 unchanged, per 396415e8,
v0.48.0), JUnit report format, `passed`/`failed` counting, misuse and
apparatus classes, settle/notification machinery, wasm registry files.

## Acceptance criteria

- Multi-doc run, one parse-error doc: summary shows
  `N passed, M failed, 1 parse-error doc (skipped)`, stderr names the doc
  before the summary, exit 2.
- All-good run: summary reads `N passed, 0 failed`, no parse-error
  segment, no naming line.
- Parse-error doc only: `0 passed, 0 failed, 1 parse-error doc (skipped)`.
- Existing exit-code and stderr behavior unchanged elsewhere.

Affected crates: camel-cli. bd: rc-ornrp.

## Risk budget

Output-format consumers that parse the summary line strictly could see
the new segment. Acceptable: the segment appears only on runs that
already exit 2. Out of bounds: any change to exit codes, JUnit output, or
existing diagnostic lines.
