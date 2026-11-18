# Design: gateparser

## Approach

Both parsers live in `scripts/gates-local.sh` as `python3` heredocs. The hardening
keeps that shape but stops the two parsers from drifting: one shared scanner, two
row grammars.

**Shared scanner (reuse, not fork).** The fence scanner from the lanes parser —
`OPENER`/`CLOSER`/`HEADING` regexes, `opener()`, `closes()`, the physical-line
reader (`newline=""`, single trailing-CR strip) and the `search → heading → table`
state machine with its fail-closed errors — moves into a common block composed once
as `PY_COMMON` (quoted heredoc into a bash variable; the content stays literal).
Each phase composes its program as `"$PY_COMMON" + <phase grammar>` and runs it via
`printf '%s' "$program" | "$PY_BIN" - "$AGENTS"`, preserving today's
`python3 - <path>` argv convention. Sharing is possible in bash; the park does not
need the two-dialect fallback. The lanes parser's observable behavior (stderr
strings, exit codes, output bytes) does not change — the 38 harness cases pin it.

**Registry extraction.** The registry phase calls the same scanner with marker
`## QUALITY GATES`. Named errors (exit 2), mirroring the lanes dialect:
unterminated fence before the section (with the opening line number), no section,
heading without a fenced block, unterminated block fence, and an empty block. A
fenced example containing the heading is fence-interior text, never the section.

**Registry row grammar (validation).** Same `- name:` / `run:` grammar as
today, with the byte rules pinned. Heading match: a line outside fences
full-matching `## QUALITY GATES\s*`. `- name:` row: value = the text after the
`- name:` prefix and its ASCII space/tab separator; the inline-comment rule
(cut at the first `  #`, two ASCII spaces then `#`) applies ONLY to name
values and plain-scalar run values, never to continuation lines; trailing
ASCII spaces/tabs are syntax (stripped); any other byte is payload. `run:`
row: ≥1 leading ASCII space/tab, literal `run:`, then ASCII spaces/tabs
stripped BEFORE the indicator check (the live `run: |` rows depend on this
order — the separator must be consumed before `|` is looked at); `|` or `>` as
the first remaining character selects block mode (one command per continuation
line, newline-joined; the rest of the indicator line is ignored); anything
else is a plain scalar (space-folded across continuations). Continuation: a
line starting with four ASCII spaces while a gate is active; leading and
trailing ASCII spaces/tabs are stripped (syntax), interior bytes verbatim
(payload `#`, tabs, quotes preserved). Blank: empty or all-ASCII-spaces/tabs.
A comment line: ≤3 leading spaces then `#` (explicitly allowed; today
silently ignored). Exactly one `run:` per gate. New fail-closed checks, each
naming file and line: NUL byte anywhere in the block; `- name:` with an empty
name; a name containing a tab or control/exotic-separator character; `run:`
before any `- name:`; a SECOND `run:` under the active gate (today it silently
overwrites the first); an indented continuation line before any `run:` (today
silently dropped); a stray content line matching no rule (today silently
dropped — this is how a command line disappears); and a gate that ends with
an empty command (today `bash -c ""` passes). Scanner limit, pinned:
extraction takes the FIRST fenced block under the first complete-line
`## QUALITY GATES` heading found outside fences — a later registry-shaped
block is never read.

**Byte-exact transport with a checked handoff.** The parser validates the ENTIRE
registry before emitting anything (validate-then-emit: no partial gate list can
ever be dispatched), then writes NUL-delimited records `name<TAB>command<NUL>`
through `sys.stdout.buffer` into a scratch file. Bash reads that file with
`mapfile -d '' -t GATES` AFTER a checked producer exit (`if ! printf '%s' "$py"
| "$PY_BIN" - "$AGENTS" > "$file"; then exit 2; fi` under the script's
`pipefail`). Process substitution is NOT used for the registry parse: `mapfile
< <(…)` would swallow the producer's exit status and let a failed parser yield
an empty gate list. The scratch dir is allocated by the existing TMPDIR
ladder; the spool file is DELETED immediately after `mapfile` reads it and
before any gate dispatch (an in-crate scratch dir — the case_23 scenario —
must never expose a live spool to the lane phase's untracked-path scan), with
EXIT-trap cleanup kept as the error-path fallback. The `\n`-escape
encoding disappears, so a command containing a literal `\n` sequence (e.g.
`sed 's/a\nb//'`) survives byte-identically; multi-line `run: |` blocks travel
raw. Read/decode failures are exit 2 with named diagnostics — input read and
UTF-8 decode failures in `physical_lines` (`cannot read %s: %s`), spool
STAGING failure (`cannot stage the gate spool`), a checked `mapfile` read of
the spool (`cannot read the parsed gate spool`) — and the producer's
validate-then-emit means a malformed-last-row input never reaches the reader
because nothing is emitted after a validation failure.

**Field-injection surface.** The transport's shiftable field is the NAME (first
tab-split field): a tab or control/exotic-separator character in a name is a
named error (today silently rewritten to a space). A NUL inside a `run:`
payload is a named error (record-boundary split vector). A tab inside a run
payload is VALID command bytes — it carries no transport power (the command is
the last field), and byte-exactness forbids rewriting it; the harness pins this
with a tab-bearing payload that must reach `bash -c` unchanged.

**Sweep loop.** Unchanged: same SKIP/ONLY selection, `=== name` headers,
`bash -o pipefail -c` execution, summary lines, lane phase and its precedence.
Only the record decoding line changes (the `${command//\\n/…}` translation is
deleted, not replaced).

**Harness.** `scripts/gates-lanes-test.sh` gains a registry-parser section using
the existing fixture idiom (throwaway repos, unmodified production script,
real `true`/`false` gate commands, stub cargo for the lane phase). Cases:
fenced four-backtick example before the real section (fake gates never run);
hostile example above AND below with exact gate set; unclosed fence named
error; NUL in a `run:` payload; tab-in-name field injection (a shifted read
would run `true` where the real command is `false`); second-`run:` and
continuation-before-`run:` rejections; empty-run rejection; registry twin of
the lanes byte-exact exotic-command case; registry/lanes equivalence on the
SAME hostile fixture (both parsers see exactly their real section); selector
and summary-count behavior.

Byte-exactness is observed at the execution boundary, not inferred: each
transport case uses a fixture gate whose command is
`printf '%s' "$BASH_EXECUTION_STRING" > "$TRACE_DIR/<gate>"` — `$BASH_COMMAND`
only names the currently running command and would truncate multi-line
operands (verified by probe), while `$BASH_EXECUTION_STRING` carries the
complete `bash -c` operand, newlines and tabs included. Each trace file is
`cmp`-ed against the expected operand bytes. Failure cases additionally assert
NO dispatch: the stub argv log and every trace file stay empty, proving the
sweep read no spool content after a validation failure — stated as its true
invariant: the producer's exit is checked BEFORE `mapfile` runs, so a failed
parse structurally cannot reach the reader; the observable consequence is no
dispatch (empty argv log and traces). Named exit 2 is also required for
registry scratch-allocation failure; the fixture is a sweep-only `mktemp`
stub, placed first on PATH, that fails EVERY rung of the allocation ladder
(a file-valued `TMPDIR` alone cannot force this: the `-d` rung is bypassed
and `/home/shared/tmp` still allocates successfully). The spool-open/read
failure shares the same named-error guard as the scratch branch and is not
directly fixture-forced; the scratch fixture exercises the guard, and the
spool branch is its sibling (stated honestly rather than claimed as tested).

Red/green protocol: red evidence for the hostile cases (fenced-example hijack,
unclosed fence, NUL/injection) is captured by running the finished harness
AGAINST THE PINNED PRE-FIX SCRIPT — `baseline/gates-local-pre.sh` (committed
under the change dir) placed in a scratch dir beside a copy of the harness
(the case_33 nested-copy idiom), so `$SCRIPT_DIR/gates-local.sh` resolves to
the old parser. Red evidence records the old script's full observable behavior
(exit status, stderr, extracted/ executed gates), not merely an assertion
failure; the same fixtures are then green against the in-tree fixed script.

**Baseline protocol (pinned evidence under `baseline/` in the change dir;
currently untracked — committed together with the blessed spec).** Pins:
merge-base commit `42d95eac1bce58d2f51363a72e04cf825b699ad5`;
its AGENTS.md blob `d1bfe3c5c577a2ac9c1f796d063b80105fd641c0` (`git
hash-object`); the pre-fix script (`baseline/gates-local-pre.sh`); the legacy
parser verbatim (`baseline/registry-parser-pre.py`); its stdout
(`baseline/gates-pre.tsv`, 34 records, sha256 `8525c8c2…`); and the pre-change
sweep summary (`baseline/sweep-pre-summary.txt`): passed 30, failed 0,
skipped 4 — lint-commits lint-bridge-releases doc-guide-test cargo-audit,
ran 30 of 34, lanes 0/0/0, no-lane 0, exit 0 (2026-11-18, pinned commit,
`GATES_*` unset, `RUSTC_WRAPPER=sccache`). Comparison format: canonical
repeated `name<NUL>command<NUL>` records — the legacy side decoded exactly as
the old bash consumer decoded it (the `\n` escape back to newlines), the new
side split on the first tab after `mapfile -d ''` — and the two files
`cmp`-ed, so the proof is on decoded command bytes, not escape syntax.

**skip_specs.** `skip_specs: true` stands, with the rationale sharpened per
review: no capability spec governs the internal parser of the local sweep
script. The release guard spec (`openspec/specs/bridge-release-guard/spec.md`)
governs observable skip behavior of sweeps, not how a parser extracts gates;
AGENTS.md already wires the harness into the registry itself
(`lint-gates-lanes`), making the harness the normative behavioral record for
this surface. The gates extracted and their behavior are unchanged
(baseline-proven); this is tooling hardening, not a capability change.

## Affected crates

- None. `scripts/gates-local.sh` and `scripts/gates-lanes-test.sh` only; no
  `*.rs`, no `Cargo.toml`, no CI files. All Rust gates are
  "N/A — no Rust changed".

## Architecture boundaries

Tooling-only change under the mission's zone lease (`scripts/gates-local.sh` +
harness). The AGENTS.md registry remains the single source of truth; the sweep
still parses it rather than copying it. Lane phase contract (disabled phase
bypasses parsing; selector precedence; row-counted skips) is preserved and
re-pinned by the existing harness cases.
