# Proposal: camel-lint-directories

## Why

`camel lint <path>` accepts exactly one file. Linting a route tree today means
a shell loop that re-implements file discovery, reserved-document skipping,
and exit-code aggregation — and each copy drifts. Owners asked for a native
directory mode (bd rc-61fgd): point `camel lint` at `routes/` or `examples/`
and get one deterministic, aggregated result.

## What Changes

- `camel lint` accepts a directory path and recurses for eligible files
  (`.yaml`, `.yml`, `.json`, case-sensitive). Single-file mode is unchanged.
- The walker prunes hidden entries and `target/` directories below the root,
  never descends into symlinked directories, and lints symlinked files.
- Reserved documents (`*.test.yaml`, `*.test.yml`, `*.job.yaml`,
  `*.job.yml`) are skipped with the same info wording as single-file mode
  and counted in a summary.
- Diagnostics aggregate across files; the exit code is the highest of
  2 (root misuse or any per-file read failure) / 1 (any error-severity
  diagnostic) / 0.
- File and diagnostic order is deterministic: depth-first pre-order with
  per-directory name-byte sorting.
- Directory mode prints one summary line to stdout. Single-file output is
  byte-identical to today (no summary line).
- Docs: `docs/src/cli/index.md` and `docs/src/getting-started/cli.md`
  describe directory mode.

Explicitly excluded: reconciling the `tests/lint_corpus.rs` corpus walker
with this walker (decided in design; follow-up bd), `.gitignore`-aware
walking, parallel linting, and any new lint rule or engine change.

## Acceptance criteria

- A directory run recurses and lints every eligible file (test asserts the
  exact file set, two levels deep).
- Aggregated exit code is 1 when one file in the tree has an error-level
  diagnostic, 0 when none does.
- Ordering is stable: identical output across runs; sorted visit order.
- A fixture/reserved document inside the tree is not linted and is reported
  as skipped.
- Single-file behaviour is unchanged: existing tests pass unmodified and
  file-mode output stays byte-identical.
- Per-file read failures exit 2 while the walk continues and reports every
  file.
- `openspec validate camel-lint-directories --type change --json` is clean.

## Risk budget

Acceptable: camel-cli-local changes (`commands/lint.rs` + tests) and doc
edits. No new dependencies (walker is ~40 lines of `std::fs` recursion).
Out of bounds: engine, rule, or `camel-dsl` discovery changes; corpus-gate
baseline churn. The corpus gate must keep passing untouched.

Refs: bd rc-61fgd; CONTEXT-MAP route-lint capability; ADR-0062 (reserved
suffix contract).
