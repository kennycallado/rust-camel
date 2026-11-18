# Tasks: gateparser

Single-phase change (no `## Phase N` grouping; design.md has no `## Phases`
section). Task order is the delivery order: harness red → shared scanner →
registry hardening. The harness cases below extend
`scripts/gates-lanes-test.sh`; they follow its existing idiom exactly:
throwaway git repos (`fixture_new` / `fixture_init_repo` /
`fixture_base_commit`), the unmodified production script via `run_sweep`,
stub `cargo` on PATH (`install_stub_cargo`), and `assert_*` helpers. All new
registry fixtures use `GATES_LANES=0` (or `GATES_ONLY=<registry gate>`) so the
lane phase stays out of the assertion surface, except case 47 which asserts
both phases.

Registry fixtures need a REAL gates section, so they hand-write `AGENTS.md`
(`fixture_agents_preamble` provides the minimal one: real `## QUALITY GATES`
with gate `noop`/`run: 'true'`). Transport-proof fixtures use trace gates:
the command `printf '%s' "$BASH_EXECUTION_STRING" > "<trace-dir>/<gate>"`
captures the complete `bash -c` operand (NOT `$BASH_COMMAND`, which only
names the currently running command and truncates multi-line operands).
Trace dirs live under `$REPO` and are `cmp`-ed against expected byte files.

## scripts/gates-local.sh harness

### Task 1.1: Registry-parser harness section with red evidence

**Files:**
- `scripts/gates-lanes-test.sh` (modified — new cases appended, CASES array extended)

**Steps:**
1. Append a `## registry parser (## QUALITY GATES)` section header comment and
   the ten case FUNCTIONS `case_39` through `case_48` (the driver calls
   `"case_$(printf '%02d' "$index")"` exactly — two-digit, no suffix; the
   long names below are the description strings), then append ten matching
   description strings to the `CASES` array (order is the binding).
2. Reuse the EXISTING `fixture_agents_preamble` helper for minimal registry
   fixtures (do NOT add a duplicate — the preamble already is the minimal
   real gates section); hand-written fixtures supersede it where a case
   needs a different shape.
3. `case_39` (`CASES` description: `registry: fenced example does not
   hijack extraction`): fixture (hand-written, NOT preamble): doc example
   FIRST — a `## NOTES` heading, then a four-backtick `` ```` `` fence
   containing a complete-line `## QUALITY GATES` heading, a blank line, a
   three-backtick fence, the row `- name: fake` / `  run: 'true'`, the
   closing three-backtick fence, and the closing four-backtick fence — THEN
   the real section: `## QUALITY GATES`, three-backtick fence, `- name:
   real` / `  run: 'true'`, closing fence. Guard the fixture (grep): the
   fake heading occurs before the real heading and both are complete lines.
   `run_sweep GATES_LANES=0`. Assert: exit 0; stdout has `=== real` and
   `PASS real`; stdout lacks `=== fake`; `=== real` count is exactly 1.
4. `case_40` (`registry: hostile example above AND below, gates exact`):
   fixture: hostile example above (as case 39, fake gate `fakeabove`) AND
   below (a second `## NOTES` section after the real section, same
   four-backtick shape, fake gate `fakebelow`). Real section has two trace
   gates IN ORDER:
   `- name: g1` / `  run: printf '%s' "$BASH_EXECUTION_STRING" > "$T"/g1 #
   a|b"c` (single line; the quote-pipe bytes ride in a trailing shell COMMENT
   so nothing extra prints and the trace stays the exact operand), and
   `- name: g2` / `  run: |` block whose two continuation lines are
   `printf '%s' "$BASH_EXECUTION_STRING" > "$T"/g2; : 'x\n<TAB>y'` (the
   trace redirect captures ONLY the printf's output — `printf '%s'` has
   exactly one argument — and the no-op `:` command carries the literal
   backslash-n and TAB bytes inside its quoted argument, on the same line)
   and `printf 'tail' >> "$T"/g2`. `mkdir -p "$T"` with
   `T="$REPO/traces"` in the fixture, passed through via
   `run_sweep GATES_LANES=0 T="$T"` (run_sweep forwards env through `env`).
   Assert: exit 0; `PASS g1` and
   `PASS g2` present; `assert_out_order '=== g1' '=== g2'`; `=== fakeabove`/
   `=== fakebelow` absent; `cmp` of `$T/g1` against the expected one-line
   operand bytes and `$T/g2` against exactly the COMPLETE operand (L1
   including its `: 'x\n<TAB>y'` tail, newline, L2) then `tail` — `printf
   '%s' "$BASH_EXECUTION_STRING"` writes the operand with no trailing
   newline and L2's append adds `tail` with none, so the file is
   `L1\nL2tail`; TAB and `\n` literal intact; stdout has
   no second `=== g1`.
5. `case_41` (`registry: unclosed fence before the heading is a named
   error`): fixture: preamble-style hand-write — a `~~~` fence line at the
   very top, an `intro` line, then (NO closing ~~~) the real
   `## QUALITY GATES` section with gate `real` / `run: 'true'`. Compute
   `open_line` = the file line number of the `~~~`. `run_sweep
   GATES_LANES=0`. Assert: exit 2; stderr matches
   `unterminated fence at line <open_line>`; stderr has
   `no ## QUALITY GATES section`; stdout has NO line matching `^=== `
   (nothing dispatched); argv log empty.
6. `case_42` (`registry: NUL in a run payload is rejected`): fixture: the
   preamble's gates fence EXTENDED — insert the bad gate INSIDE the existing
   three-backtick fence, BEFORE its closing line (appending after the fence
   would leave the row unparsed): after `- name: noop` / `  run: 'true'`,
   add `- name: bad` / `  run: 'tr'<NUL>'ue'` (printf-built, bytes
   `tr<NUL>ue`). Guard: the file carries a NUL. `run_sweep GATES_LANES=0`.
   Assert: exit 2; stderr matches `NUL byte at line <nul_line>`; stdout has
   NO line matching `^=== ` (the VALID first gate must not dispatch —
   validate-then-emit); argv log empty.
7. `case_43` (`registry: tab in a gate name cannot inject fields`): fixture:
   hand-written registry with one gate whose name carries a literal TAB:
   `- name: gate<TAB>inject` / `  run: 'true'` (printf-built). `run_sweep
   GATES_LANES=0`. Assert: exit 2; stderr names the file and matches
   `(line|row) +[0-9]+` AND has `name`; stdout has NO `^=== ` line; argv log
   empty.
8. `case_44` (`registry: grammar rejections are named, file+line`):
   parameterized over seven bad registries (printf-built, one gate unless
   stated): (a) second `run:` under one gate (second value `false`, first
   `true`); (b) four-space continuation line BEFORE any `run:`; (c) stray
   unindented line `not-a-gate-line` between name and run; (d) `- name:` with
   no value; (e) `  run: 'true'` line before any `- name:`; (f) gate with
   name but no `run:` at all; (g) `  run:` with empty value. Each sub-case:
   `run_sweep GATES_LANES=0`; assert exit 2; stderr names file +
   `(line|row) <n>`; stdout has NO `^=== ` line; argv log empty. Sub-case (g)
   additionally asserts stderr has `empty run`.
9. `case_45` (`registry: exotic command reaches bash byte-identically`):
   fixture: preamble with the noop gate replaced by one exotic trace gate —
   `run: |` block with FOUR continuation lines (exact bytes):
   line 1 `printf '%s' "$BASH_EXECUTION_STRING" > "$T"/exotic`
   line 2 `printf '%s\n' 'a<TAB>b' > sed.in && sed 's/a\nb/x/' sed.in | tee
   'out 1'.log` (the interior TAB byte rides inside the quoted printf
   argument — no exotic FILENAME exists: sed.in and 'out 1'.log are plain;
   the sed input is seeded by the gate's own first command, the tee output
   lands in the fixture repo)
   line 3 `printf 'x' --flag=é \` (é as UTF-8 bytes, trailing shell
   line-continuation backslash — mid-operand, so the shell joins line 4)
   line 4 four ASCII spaces then `--more` (a legal continuation under the
   pinned grammar; the operand carries it as `--more` — leading ASCII
   indentation is stripped by the parser, not transported).
   `mkdir -p "$T"`, `T="$REPO/traces"` via `run_sweep GATES_LANES=0 T="$T"`.
   Assert: exit
   0; `cmp` of `$T/exotic` against the exact FOUR-line expected operand
   bytes: line 1, newline, line 2 (TAB + `\n` literal intact), newline, line
   3, newline, `--more` (no trailing newline — the last continuation
   contributes no terminator).
10. `case_46` (`registry: scratch allocation failure is a named exit 2`):
    fixture: preamble (valid). Install a sweep-only failing `mktemp` stub: a
    `$BASE/stub-mktemp/mktemp` script that writes `stub mktemp: exploded on
    purpose` to stderr and exits 1 — pass its dir into run_sweep via
    `EXTRA_BIN` (run_sweep's PATH resolves `EXTRA_BIN` ahead of `$STUB_BIN`
    and the inherited PATH), the same mechanism `install_failing_git` uses.
    GUARD the stub before the sweep (the case_16/27 pattern): invoke it once
    directly and assert it exits 1. `run_sweep GATES_LANES=0` with that PATH.
    Assert: exit 2; stderr has `cannot create a scratch dir` AND `registry`;
    stdout has NO `^=== ` line; argv log empty.
11. `case_47` (`registry and lanes: same hostile input, both parsers win`):
    fixture: FULLY hand-written AGENTS.md (not preamble) — FIRST a `## NOTES`
    four-backtick example BEFORE BOTH real headings, containing BOTH fake
    headings (`## QUALITY GATES` with fake gate `fakereg`/`'true'` AND
    `## COMPONENT LANES` with fake row `camel-fake | local | true` inside
    their own inner three-backtick fences); THEN the real `## QUALITY GATES`
    section (gate `noop`/`'true'`); THEN the real `## COMPONENT LANES`
    section with row `camel-real | local | cargo test -p camel-real`.
    Guard: the fake headings both precede both real headings and sit inside
    the four-backtick example. `crate_seed crates/real camel-real`;
    `fixture_base_commit`; append a comment line to
    `crates/real/src/lib.rs` (the case_01 dirty-touch pattern — `touch`
    alone does not dirty git) and `commit_work`; `fixture_meta
    'camel-real:crates/real:lib'`. `run_sweep` with NO selector (both phases
    run). Assert: exit 0; stub argv log is EXACTLY
    `test -p camel-real` (lanes real row only); stdout has `PASS noop` and
    `PASS lane camel-real`; stdout lacks `fakereg` and `camel-fake`;
    `PASS lane camel-fake :: true` absent.
12. `case_48` (`registry: summary counts on pass/fail/skip`): fixture:
    preamble whose registry has three gates IN THIS ORDER: `ok1`/`'true'`,
    `bad`/`'false'`, `ok2`/`'true'`. `run_sweep GATES_LANES=0`. Assert: exit
    non-zero; stdout has the exact lines `passed:  2`, `failed:  1 — bad`,
    `skipped: 0`, `ran:     3 of 3`; then a second `run_sweep GATES_LANES=0
    GATES_SKIP="ok1 ok2 bad"` asserting `skipped: 3 — ok1 bad ok2` (skip
    labels print in registry order, `${skipped[*]}` append order) and
    `ran:     0 of 3` and exit 0.
13. RED EVIDENCE (conductor-side procedure, executed AFTER this task's cases
    exist and BEFORE Task 3.1 lands; artifacts stay OUT of the change dir —
    in the conductor's scratch under `/home/shared/tmp/gateparser-baseline/`
    and summarized in the park — adding files under the blessed change dir
    would drift the blessing hash): create a scratch dir; copy
    `openspec/changes/gateparser/baseline/gates-local-pre.sh` into it as
    `gates-local.sh` and copy THIS harness beside it (so the harness's
    `$SCRIPT_DIR/gates-local.sh` resolves to the pinned pre-fix parser).
    The production harness's EXIT trap deletes its scratch, so the capture
    hook lives IN THE COPY, not in production: patch the copied harness's
    `run_sweep` function with appended lines that persist each run's
    evidence to a case-and-run-keyed directory under an exported
    `RED_EVIDENCE_DIR` — `dest="$RED_EVIDENCE_DIR/${BASE##*/}/$RUN"`,
    `mkdir -p "$dest"`,
    `cp -f "$OUT" "$ERR" "$dest/"` and
    `printf '%s' "$SWEEP_CODE" > "$dest/code"`, each `|| true`-tolerated —
    keyed by case (`${BASE##*/}`) and run because `fixture_new` resets
    `RUN`, so a flat directory would overwrite out-1/err-1/code-1 across
    cases; export `RED_EVIDENCE_DIR` to the
    persistent conductor scratch before invoking the copy, and leave the
    production harness's cleanup unchanged. After the run, VERIFY the
    captures: for each targeted case (39/41/42/43), the expected
    `$RED_EVIDENCE_DIR/<case-dir>/<run>/` files must exist — the STDERR file
    must EXIST but MAY BE EMPTY (the pinned script legitimately writes zero
    bytes of stderr in some hostile cases; record emptiness explicitly as
    `stderr: empty` in red-evidence.txt), the STDOUT file must be NON-EMPTY
    and contain the expected `=== <gate>` dispatch headers, and the code
    file must hold a valid numeric exit status. A missing capture FAILS the
    red-evidence gate (rerun); it is never assumed. Run
    `bash scratch/gates-lanes-test.sh` capturing top-level stdout, stderr,
    and exit. Record in `red-evidence.txt`: the
    pinned-script identity, the date, the per-case PASS/FAIL list, each
    failing case's detail line, and — for cases 39/41/42/43 — the old
    script's observed behavior WITH the captured sweep evidence (from
    `$RED_EVIDENCE_DIR`): its exit status, its stderr, and the `=== <gate>`
    headers it wrongly dispatched
    (39 → `fake`/exit 0; 41 → `real`/exit 0, fence ignored; 42 → `bad`
    dispatched with the NUL dropped; 43 → `gate inject` with the tab
    rewritten). EXPECTED gate: cases 1–38 PASS; 39,40,41,42,43,44,45,46,47
    FAIL each FOR THE INTENDED REASON (assertion mismatch on the named
    expectation, not a fixture/crash error); case 48 PASS. A red run where a
    case fails for an unintended reason (fixture broken, wrong exit code
    shape) means the CASE is wrong — fix the case, not the expectation.

**Tests:** (the ten new cases above ARE the tests; command for all)
- `command`: `bash scripts/gates-lanes-test.sh`
- `expected` (before Task 3): in-tree run is RED for 39–47 per the red
  procedure; after Task 3: `48/48 cases green`, exit 0.

**Acceptance:**
- Red run recorded with per-case outcome, per-sweep exit/stderr/dispatch
  headers for 39/41/42/43, and the intended-reason gate satisfied.
- `bash scripts/gates-lanes-test.sh` exits 0 in-tree ONLY after Task 3; the
  pre-Task-3 in-tree run fails exactly on the new hostile cases.
- `shellcheck scripts/gates-local.sh` recorded as UNAVAILABLE on this host
  (command -v shellcheck empty) — noted in the park, not a failure.

- [x] 1.1

## scripts/gates-local.sh scanner

### Task 2.1: Extract the shared fence scanner (PY_COMMON), lanes parser consumes it

**Files:**
- `scripts/gates-local.sh` (modified)

**Steps:**
1. Above the registry `mapfile`, define `PY_COMMON` with a quoted heredoc
   assigned to a variable (`PY_COMMON="$(cat <<'PY'
   [scanner source: imports, regexes, physical_lines, opener, closes,
   fenced_section — exactly as enumerated below]
   PY
   )"`), containing exactly, in order: `import re`, `import sys`; the lanes
   parser's `OPENER`, `CLOSER`, `HEADING`, `BAD_FIELD` regexes; `physical_lines(path)` (open with
   `encoding="utf-8", newline=""`, read, split on `"\n"`, strip one trailing
   `"\r"` per line); `opener(line)` and `closes(line, char, length)` copied
   verbatim from the lanes heredoc; and a new `fenced_section(lines, marker,
   label)` implementing the lanes parser's `search → heading → table` state
   machine verbatim, parameterized so every stderr message keeps its CURRENT
   lanes wording with `marker` in place of the literal `## COMPONENT LANES`
   and `label` in place of the literal `lanes` — i.e. for the lanes parser the
   emitted strings are byte-identical to today's (`gates-local: lanes:
   unterminated fence at line %d in %s`, `gates-local: lanes: no ## COMPONENT
   LANES section in %s`, `gates-local: lanes: no fenced table under ## COMPONENT
   LANES (section at line %d) in %s`, `gates-local: lanes: unterminated fence
   at line %d under ## COMPONENT LANES in %s`), each `sys.exit(2)`. It returns
   `(content, table_lineno)` on success. The marker-specific heading regex
   (`LANE_HEADING` today) is built INSIDE `fenced_section` as
   `re.compile(re.escape(marker) + r"\s*")` with `.fullmatch`, and the `%s`
   file field in every message reads a caller-defined global `name` — each
   grammar block defines `name = os.path.basename(sys.argv[1])` before
   calling `fenced_section` (`import os` lives in each grammar block that
   needs it; the scanner imports only `re` and `sys`).
2. Replace the lanes heredoc's scanner head (regexes, `open/split`, `opener`,
   `closes`, state machine) with:
   `lines = physical_lines(sys.argv[1])` then
   `content, table_lineno = fenced_section(lines, "## COMPONENT LANES",
   "lanes")`, leaving the row-grammar loop (`rows`, NUL check, bad-row checks,
   `BAD_FIELD` crate/mode checks, `print("%s\t%s\t%s")`) untouched.
3. Move the lanes invocation to the composition pattern (a quoted heredoc
   cannot expand `$PY_COMMON`): capture the lanes row grammar — everything
   from `import os` through the final `print("%s\t%s\t%s")` row emission —
   into `lanes_grammar` with a quoted heredoc (`lanes_grammar="$(cat <<'PY'
   [the grammar source enumerated in step 2]
   PY
   )"`), build `lanes_py="$PY_COMMON"` + a newline + `"$lanes_grammar"`, and
   replace the lanes invocation with
   `lane_table="$(printf '%s' "$lanes_py" | "$PY_BIN" - "$AGENTS")" || exit 2`
   (keeping the existing `|| exit 2` guard shape). The lanes grammar's local
   `marker = "## COMPONENT LANES"` variable (used in its row-error messages)
   stays in the grammar block. Every stderr byte, exit
   code, and output row must be identical to today.
4. Regression-gate the refactor with the ORIGINAL 38-case harness copied
   beside the refactored script (the case_33 nested-copy idiom): extract the
   pre-change harness from the pinned commit (`git -C "$ROOT" show
   42d95eac1bce58d2f51363a72e04cf825b699ad5:scripts/gates-lanes-test.sh`) into
   a scratch dir as `gates-lanes-test.sh`, copy the REFACTORED
   `scripts/gates-local.sh` beside it, run `bash scratch/gates-lanes-test.sh`
   — 38/38 green required (this isolates the lanes-pinned behavior from the
   registry cases Task 1.1 added, which are expected red until Task 3.1).
   Additionally run the IN-TREE suite `bash scripts/gates-lanes-test.sh` and
   require: cases 1–38 green, cases 39–48 red is acceptable AT THIS TASK
   (registry not yet hardened) — record the outcome. No real-sweep smoke in
   this task; the production smoke is Task 3.1's full sweep.

**Tests:**
- copied-original-harness run (step 4): setup = refactored script + pinned
  38-case harness in a scratch dir; action = run it; assert `38/38 cases
  green`, exit 0.
- in-tree suite (step 4): assert cases 1–38 green; 39–48 may be red here.

**Acceptance:**
- The copied-original-harness run is 38/38 green against the refactored
  script.
- `git diff` shows NO change to: any lane row string, any stderr message
  string, the lane summary format, `SKIP`/`ONLY`/`GATES_LANES` handling.
- The scanner (regexes, `physical_lines`, `opener`, `closes`, state machine)
  exists ONCE in the file.

- [x] 2.1

## scripts/gates-local.sh registry

### Task 3.1: Harden the QUALITY GATES registry parser (scanner reuse + NUL transport + fail-closed grammar)

**Files:**
- `scripts/gates-local.sh` (modified)

**Steps:**
1. Compose `registry_py="$PY_COMMON"` + newline + `registry_grammar`
   (quoted heredoc) implementing, in order:
   a. `import os` then `name = os.path.basename(sys.argv[1])`;
      `try: lines = physical_lines(sys.argv[1])` with
      `except (OSError, UnicodeDecodeError, ValueError) as error:`
      stderr `gates-local: registry: cannot read %s: %s` (path, error),
      exit 2 — input read and UTF-8 decode failures are NAMED, never a raw
      traceback;
      `content, block_lineno = fenced_section(lines, "## QUALITY GATES",
      "registry")` — the shared scanner, label `registry`.
   b. NUL check over `content` BEFORE any grammar work: if any raw line
      contains `"\x00"`, stderr
      `gates-local: registry: NUL byte at line %d in %s ## QUALITY GATES`
      (line = the 1-based file line carried by `content` tuples), exit 2.
   c. Row grammar per design.md (pinned rules): `- name:` row (regex
      `^- name:[ \t]+(.*)$`): value = group(1); comment cut at first `  #`;
      strip leading/trailing ASCII `[ \t]`; empty after strip → stderr
      `gates-local: registry: bad row at line %d in %s ## QUALITY GATES: %s`
      (raw line), exit 2; name containing `[\t\x00-\x1f\x7f\x85\u2028\u2029]`
      → stderr
      `gates-local: registry: bad name at line %d in %s ## QUALITY GATES: %r`,
      exit 2. If a gate is already active → this is a NEW gate: flush first.
      `run:` row (regex `^[ \t]+run:[ \t]*(.*)$`): if no gate active →
      stderr `gates-local: registry: run before name at line %d in %s ##
      QUALITY GATES`, exit 2; if a run was already started for the active
      gate → stderr `gates-local: registry: second run at line %d in %s ##
      QUALITY GATES`, exit 2; strip `[ \t]` from the value START before the
      indicator check; if the value starts with `|` or `>` → block mode
      (`run_lines=[], run_sep="\n"`, remainder of the line ignored); else
      plain scalar (comment cut at `  #`, strip ASCII `[ \t]` both ends →
      first segment, `run_sep=" "`). Continuation (line starts with four
      ASCII spaces): requires an active gate AND a started `run:` — append
      `line.strip(" \t")` (empty → skip); a four-space line with an active
      gate but NO started `run:` is a named error via the bad-row catch-all
      below (baseline-safe: the real registry never has one). A continuation
      with NO active gate → stderr
      `gates-local: registry: continuation before name at line %d in %s ##
      QUALITY GATES`, exit 2. Blank (empty or all `[ \t]`) → skip. Comment
      (≤3 leading ASCII spaces then `#`) → skip. Anything else (including
      the four-space-without-run case above) → stderr
      `gates-local: registry: bad row at line %d in %s ## QUALITY GATES: %s`
      (raw), exit 2.
   d. Flush at end: join the collected segments with `run_sep` and strip
      ASCII `[ \t\r\n]` from both ends (the old parser's `.strip()`
      semantics restricted to ASCII; identical on the real registry). Empty
      command → stderr
      `gates-local: registry: empty run for gate '%s' at line %d in %s ##
      QUALITY GATES` (gate name + the `- name:` line number, tracked as
      `name_lineno`), exit 2.
   e. Empty block (zero gates): the shared scanner does NOT check for this
      (it returns empty content with `closed=True`); the bash zero-gate guard
      kept in step 3 is the empty-block backstop (exit 2, generic message).
      Do not delete that guard.
   f. Emit: `out = sys.stdout.buffer`; for each gate
      `out.write(name.encode() + b"\t" + command.encode() + b"\x00")` —
      validate-then-emit: ALL gates validated first, emission only at the end.
2. Replace the registry consumer: allocate a scratch dir BEFORE the parse
   using the SAME TMPDIR ladder as the lane phase (TMPDIR if a dir →
   /home/shared/tmp if a dir → mktemp default), e.g.
   `registry_tmp`; on total failure stderr
   `gates-local: registry: cannot create a scratch dir` + exit 2. Add
   `registry_tmp` to the EXIT trap cleanup (extend `lane_cleanup` or add a
   sibling `registry_cleanup` registered in the same trap).
3. Replace the registry `mapfile -t GATES < <("$PY_BIN" - "$AGENTS" <<'PY'
   heredoc PY)` process substitution with the checked handoff:
   `if ! printf '%s' "$registry_py" | "$PY_BIN" - "$AGENTS" >
   "$registry_tmp/gates.nul"; then echo "gates-local: registry: cannot stage
   the gate spool" >&2; exit 2; fi` (the script's `set -uo
   pipefail` covers the pipeline; python's named errors already went to
   stderr; the named message covers spool-staging failure), THEN `if !
   mapfile -d '' -t GATES <
   "$registry_tmp/gates.nul"; then echo "gates-local: registry: cannot read
   the parsed gate spool" >&2; exit 2; fi`, THEN the spool scratch removal
   is REQUIRED before any dispatch — `set -uo pipefail` does not stop
   execution when `rm` fails, so:
   `rm -rf "$registry_tmp" || { echo "gates-local: registry: cannot remove
   the gate spool scratch" >&2; exit 2; }` — removal is checked AND
   STRUCTURALLY BEFORE the sweep loop (no gate header prints and no gate
   runs before this line succeeds, so case 23's in-crate isolation never
   depends on cleanup being merely assumed; the EXIT trap remains the
   error-path fallback), THEN the zero-gate guard stays (`parsed 0
   gates` exit 2, now unreachable except for a structurally empty registry).
4. In the sweep loop, replace the entry split + decode: keep
   `name="${entry%%$'\t'*}"`; `command="${entry#*$'\t'}"`; DELETE the
   `${command//\\n/$'\n'}` translation line (commands are raw bytes now).
   Everything else in the loop (SKIP/ONLY selection, `=== name`, `bash -o
   pipefail -c`, PASS/FAIL accounting, summary lines) unchanged byte-for-byte.
5. Run the whole harness: `bash scripts/gates-lanes-test.sh` → 48/48 green.
6. Baseline byte-identity evidence (conductor-side; artifacts live in the
   conductor's scratch `/home/shared/tmp/gateparser-baseline/` and are
   summarized in the park — NOT committed under the change dir, which would
   drift the blessing hash): fixture repo under the suite scratch dir
   containing the REAL `AGENTS.md` copied from the worktree, with its blob
   VERIFIED first (`git hash-object` == `d1bfe3c5c577a2ac9c1f796d063b80105fd641c0`,
   abort the procedure otherwise); resolve the real bash ONCE before touching
   PATH (`REAL_BASH="$(command -v bash)"` — do NOT assume `/bin/bash`, this
   host is nix-shaped); run the NEW `scripts/gates-local.sh` from inside the
   fixture repo via `env PATH="<stub-dir>:$PATH" "$REAL_BASH"
   scripts/gates-local.sh` (the stub dir FIRST so the sweep's inner
   `( cd "$ROOT" && bash -o pipefail -c "$command" )` resolves the stub, while
   the outer interpreter is the pre-resolved real bash); the stub `bash`
   appends its `-c` operand (argument after `-c`) NUL-terminated to a log file and
   exits 0; run with `GATES_SKIP=""` (empty string is set, so nothing is
   skipped) and `GATES_LANES=0`; then assert the count PAIR: exactly 34
   `=== <name>` stdout lines and exactly 34 logged operands (a mismatch
   aborts before cmp); pair each `=== <name>` line with the next logged
   operand; canonicalize both sides to repeated `name<NUL>command<NUL>`
   records — legacy side from `baseline/gates-pre.tsv` decoded exactly as
   the old consumer decoded it (unescape `\n` → newline; names as printed) —
   and `cmp` the two canonical files: byte-identical REQUIRED (34/34).
7. Summary-tuple evidence (conductor-side, into the park): run the full
   sweep with the three selectors explicitly UNSET
   (`env -u GATES_ONLY -u GATES_SKIP -u GATES_LANES bash
   scripts/gates-local.sh`, worktree, matching the pinned wrapper's
   environment) — exit 0 REQUIRED; compare the EIGHT summary lines
   (`passed:` / `failed:` / `skipped:` / `ran:` / `lanes passed:` /
   `lanes failed:` / `lanes skipped:` / `no-lane:`) plus the exit status
   against `baseline/sweep-pre-summary.txt`: identical counts (passed: 30,
   failed: 0, skipped: 4 — lint-commits lint-bridge-releases
   doc-guide-test cargo-audit, ran: 30 of 34) and identical lanes block
   (0/0/0, no-lane 0). (The full sweep is also STAGE 4's mandatory gate;
   record its artifacts once and reference them from both.)

**Tests:**
- `bash scripts/gates-lanes-test.sh`: assert `48/48 cases green` (cases 39–48
  per Task 1.1; cases 1–38 must stay green — the lanes behavior is pinned).
- stub-bash canonical cmp (step 6): assert byte-identical, 34 records.
- full sweep (step 7): assert exit 0 and the summary tuple equals the
  baseline tuple.

**Acceptance:**
- 48/48 harness green; red cases 39–47 now green (red evidence on record).
- Canonical cmp: 34/34 byte-identical vs pre-change baseline.
- Full sweep exit 0 with the pre-change summary tuple (recorded, both kept).
- `git diff` shows NO change to: AGENTS.md, the lanes rows, the lanes stderr
  strings, the sweep loop's SKIP/ONLY/summary logic, any `*.rs`/`Cargo.toml`.
- `shellcheck scripts/gates-local.sh`: recorded as unavailable on this host.

- [x] 3.1
