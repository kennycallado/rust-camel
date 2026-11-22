# Design: lanes-read-guard

## Approach

Copy the registry guard (`scripts/gates-local.sh`, `registry_grammar`) into the
lanes grammar, replacing only the label:

```python
try:
    lines = physical_lines(path)
except (OSError, UnicodeDecodeError, ValueError) as error:
    # Input read and UTF-8 decode failures are NAMED, never a raw traceback.
    sys.stderr.write("gates-local: lanes: cannot read %s: %s\n" % (path, error))
    sys.exit(2)
```

`path` is `sys.argv[1]`, the same value the registry guard prints.
Exception set is identical to the registry's, so both grammars catch the same
failures: file missing/unreadable (`OSError`), invalid UTF-8
(`UnicodeDecodeError`, a `ValueError` subclass), and `ValueError` generally.

## Probe result (reachability)

Probed before design, not left to the expert:

- A corrupt `AGENTS.md` present from the start is rejected by the REGISTRY
  guard first (the registry parse runs before the lanes parse), so a test that
  starts with a corrupt file never reaches the lanes grammar.
- The lanes grammar IS reached with a corrupt file when a registry gate
  corrupts `AGENTS.md` after the registry parse. `GATES_ONLY="<gate> lanes"`
  runs that gate and still selects the lane phase. The harness case uses this
  route: no production code is bypassed or stubbed.
- Before the fix, that route ends with a Python traceback on stderr and exit 2
  (from `lane_table=$(...) || exit 2`); after the fix, the named line replaces
  the traceback.

## Affected crates

None (`scripts/gates-local.sh`, `scripts/gates-lanes-test.sh` only).

## Architecture boundaries

Not applicable: dev tooling outside the Runtime / DSL / Components layers.

## Alternatives considered

- **Factor the guard into `PY_COMMON`** (one guard, label parameter). Rejected:
  larger diff, changes the registry's reviewed wording, and the order asks for
  parity, not refactoring.
- **Test by feeding a corrupt `AGENTS.md` directly.** Rejected: the registry
  guard fires first, so the case would prove the registry guard again, not the
  lanes guard (see probe result).
