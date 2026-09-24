# Proposal: argdrop

## Why

`camel job --arg NAME=VALUE` is a pre-1.0 flag with zero users. Owner
decision 2026-09-24 (bd rc-rsvky): remove it now, before the interface
freezes. The removal is the ADJUDICATED direction (delta30 camel-cache
batch): the declared-args interface lands via rc-d5cgc A2 on the
job-document schema — the `args:` block, dynamic `--<name>` flags,
typed defaults, and `${arg:}` interpolation — not via this flag. The
legacy implicit-header path (`--arg` on documents without `args:`,
rc-uxvm6 last-wins/empty-value defects) dies with the flag and closes
rc-uxvm6 as moot. Landing this before the rc-d5cgc A2 filing keeps the
A2 back-compat clause clean ("undeclared `--arg` passes through as
send headers" must be reworded at filing time; no A1–A4 children exist
yet, so the window is open).

## What Changes

- Remove the `--arg` static flag from the `camel job` CLI surface:
  the `JobArgs::args` field, `parse_arg_pair`, tail re-parse pair
  recovery, and the clap registration.
- Remove every surface that exists ONLY for the flag: the legacy
  implicit-header injection path (`legacy_arg_headers`,
  `LEGACY_ARG_DEPRECATION`, `JobRun::cli_args`, send-time header
  injection), the `CrossFormConflict` lowering error, and the
  `UnknownArgumentName` document error (unreachable once pairs
  originate only from declared dynamic flags).
- Reword diagnostics that point at `--arg`: missing-required now
  says `pass --<name> <value>`; bool value-form and
  undeclared-document diagnostics drop the `--arg` remedy.
- Reserved argument names drop `arg` (keeps `help`, `config`,
  `report`).
- Help: the `Arguments:` spelling note drops the `--arg <name>=<value>`
  clause.
- Tests: declared-path tests convert to dynamic `--<name>` flags;
  legacy-path tests are removed (adjudicated; document
  `send.headers` remains the header mechanism and keeps its existing
  coverage).
- Spec delta on `cli-jobs`: 4 requirements REMOVED, 10 MODIFIED.
- Docs: `crates/camel-cli/CONTEXT.md` argument-validation rows.

## Impact

- Affected code: `crates/camel-cli` job command only (zone lease).
  The job-document schema (`args:` grammar) and `mode: batch`
  machinery are NOT touched. `camel compile` inherits the
  reserved-name list change through the shared declaration
  validation (in scope per the `reserved argument names`
  requirement).
- Breaking change (pre-1.0, zero users): any script passing `--arg`
  now gets clap's unknown-argument error, exit 2.
- Closes rc-uxvm6 as moot; unblocks the rc-d5cgc A2 filing.

## Acceptance criteria

- `grep -rn -- '--arg' crates/camel-cli` (excluding archive dirs)
  returns no live flag surface or diagnostic pointing at it.
- `cargo test -p camel-cli --lib` green; integration test binaries
  `job_one_shot_test` and `compiled_artifact_test` green.
- Mission gates green: fmt, clippy camel-cli (all legs),
  schema-check, doc gate.
- `openspec validate argdrop --type change` passes delta structure.

## Risk budget

Low. Removal-only in one command's CLI layer; the declared-args
document machinery (parse, coerce, interpolate) is exercised
unchanged through dynamic flags. Compile-behavior change is limited
to accepting `args: {arg: ...}` declarations that the reserved list
previously rejected.
