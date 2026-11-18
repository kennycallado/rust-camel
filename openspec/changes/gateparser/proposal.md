# Proposal: gateparser

## Why

`scripts/gates-local.sh` runs the AGENTS.md `## QUALITY GATES` registry by
parsing it out of the file, so the parser is the only thing standing between a
hostile or accidentally-broken markdown edit and the commands bash executes on
every local sweep. Mission 594 (`gatecov`, rc-nmgf3) hardened the COMPONENT
LANES parser against exactly this class — fence-state heading discovery,
physical-`\n` line splitting, byte-exact command transport, NUL/field-injection
rejection, metadata validation — but the inherited registry parser kept the old
algorithm: heading discovery by substring (`text.index("## QUALITY GATES")`),
a first-```` ``` ````-to-next-```` ``` ```` fence pairing, `splitlines()`
semantics, whitespace-stripped transport with `\n`-escaping, and no validation
at all.

Concretely, today: a fenced documentation example containing the literal
`## QUALITY GATES` heading hijacks gate extraction (example rows run as real
gates, or the real block truncates at the example); a U+2028 or NUL byte
rewrites executed commands silently; a tab inside a gate name is silently
replaced with a space; and a gate whose `run:` is missing or empty executes
`bash -c ""` and passes — a fail-open hole in a fail-closed tool.

The `e_gpt` ruling during `gatecov` placed this outside that change's Fix-2
boundary and asked for a separate track. This is that track (bd rc-yf6c9).

## What Changes

- Port the lanes scanner's algorithm to the registry parser: fence-aware
  heading discovery, physical-`\n` splitting (`newline=""`, CR tolerance only),
  byte-exact `run:` transport, explicit NUL and field-injection rejection with
  named errors, and row-grammar validation.
- Share, not fork: the fence scanner (opener/closer recognition, heading
  discovery, fail-closed section extraction) becomes one piece of code used by
  BOTH parsers inside the script; each parser keeps only its own row grammar.
- Switch the registry transport to NUL-delimited records (`mapfile -d ''`),
  removing the `\n`-escape encoding that corrupts any command containing a
  literal `\n` sequence.
- Fail closed: an unparseable registry is exit 2 with a named error, never an
  empty gate list; a gate with no command is a named error, never `bash -c ""`.
- Extend the harness (`scripts/gates-lanes-test.sh`) with a registry-parser
  section; hostile-input cases 1, 3 and 4 are proven red against the current
  parser, then green.

Excluded: the AGENTS.md gate list, the lanes table and every lane row, the
lanes parser's transport, any `*.rs` or `Cargo.toml`, CI workflows.

## Acceptance criteria

- A fenced example containing `## QUALITY GATES` does not hijack extraction.
- The real block's gates are found exactly (count, names, byte-identical
  `run:` payloads) with such an example above AND below the real block.
- An unclosed fence before the real heading is rejected with a named error.
- A NUL byte in a `run:` payload and a field-injection attempt (tab or control
  character in a gate NAME, the transport's shiftable field) are each rejected
  with a named error; a tab inside a run payload is valid command bytes and
  must reach `bash -c` unchanged (asserted by binary comparison of the actual
  `-c` operand).
- The current AGENTS.md parses to the same gate set and byte-identical
  commands as the pre-change baseline (captured before the parser changed:
  34 gates, sha256 `8525c8c22ee6d651dd952b1c3228038b6d687bab96c2f8f07f9130bf1a664bcc`).
- `bash scripts/gates-local.sh` reports the same
  `passed/failed/skipped/ran X of Y` summary as the pre-change baseline sweep.

## Risk budget

The transport rewrite is the risky part; it is guarded by the byte-exact
baseline diff, the full-sweep summary comparison, and the 38 existing harness
cases that must stay green. Acceptable: stricter rejections (named errors) on
inputs the current parser silently mangled, since the real registry triggers
none of them. Out of bounds: any behavior change of a real gate, any registry
or lanes-table edit, any new dependency.

Affected crates: none — `scripts/gates-local.sh` and its harness only.
