# Proposal: lanes-read-guard

## Why

The registry grammar in `scripts/gates-local.sh` wraps its input read in a
named guard: an unreadable or non-UTF-8 `AGENTS.md` prints
`gates-local: registry: cannot read <path>: <error>` and exits 2. The lanes
grammar calls the same `physical_lines()` helper with no guard. A read or
decode failure there ends in a raw Python traceback instead of a named error.
The exit code is still 2 (the caller does `|| exit 2`), but the operator gets
no `gates-local: lanes:` line, which breaks the "every exit 2 is named"
contract the other lane rejections keep.

This is reachable: the registry phase parses `AGENTS.md` first, then runs the
registry gates, then the lanes phase parses `AGENTS.md` again. A registry gate
that rewrites or truncates `AGENTS.md` corrupts the file between the two reads.

Bd: rc-jayh3 (discovered-from rc-yf6c9).

## What Changes

- `scripts/gates-local.sh`: wrap the lanes grammar's `physical_lines(path)`
  call in the same `try/except (OSError, UnicodeDecodeError, ValueError)` the
  registry grammar uses. Wording differs only by the grammar label:
  `gates-local: lanes: cannot read <path>: <error>`, then `sys.exit(2)`.
- `scripts/gates-lanes-test.sh`: ONE new case. A registry gate corrupts
  `AGENTS.md` (invalid UTF-8) after the registry parse; the lanes phase must
  exit 2 with the named error, no traceback, and no lane dispatch.
- Excluded: any change to the registry grammar, the shared scanner, any
  `crates/**` file, or the lanes grammar's row validation.

## Affected crates

None. Shell tooling only (`scripts/`). Zero `crates/**` edits.

## Acceptance criteria

- A lane table that cannot be read or decoded prints
  `gates-local: lanes: cannot read ...` on stderr and exits 2.
- That path prints no `Traceback` and dispatches no lane command.
- `bash scripts/gates-lanes-test.sh` reports all cases green (the 49 existing
  cases plus the new one).
- `bash scripts/gates-local.sh` exits 0.

## Risk budget

Acceptable: a 5-line edit inside one quoted heredoc. Out of bounds: any
change to which inputs the lanes grammar accepts. Valid tables must parse
exactly as before.
