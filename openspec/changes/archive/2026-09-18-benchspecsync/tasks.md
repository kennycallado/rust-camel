# Tasks: benchspecsync

## Spec coverage (docs change)

The delta spec text itself is already authored and spec-blessed; the
tasks below (a) implement the one live-doc clause the delta imposes
(Era-1 freeze → COVERAGE.md tag-retrieval note), (b) re-verify the
factual anchors of all 8 MODIFIED requirement blocks
(Payload-size axis, Ratio confidence intervals, Node contender
family, Zone contract, Era-1 freeze, Canonical full-matrix run, Warm
tick mode, axum-bare reference contender), and (c) run the gates.

## benchmark-docs

### Task 1.1: COVERAGE.md era-1 reference hygiene

**Files:**
- `benchmarks/scenarios/COVERAGE.md` (modified)

**Steps:**
1. Insert the note as its own paragraph — blank line, note, blank
   line — immediately BEFORE the table header line
   `| | M1 cold-start + RSS | M2 warm p99 | M3 sustained throughput | M4 memory growth |`
   (~line 121), keeping GFM rendering intact. The note text, as a
   SINGLE line:
   `Note: the docs/benchmarks/ reports referenced below are untracked (ef337c0f, bd rc-mq1sh); on-disk copies exist in the owner checkout, and in any clone every report is retrievable from the tag bench/era-1-final (e.g. git show bench/era-1-final:docs/benchmarks/history/2026-07-21-benchmark-v3.md).`
2. Replace the full two-line sentence at ~lines 224-225, which reads
   `Each coverage release (v1, v2, v3, ...) corresponds to a published report at
   \`docs/benchmarks/YYYY-MM-DD-benchmark-vN.md\`. Reports are immutable once published. The`
   with:
   `Each coverage release (v1, v2, v3, ...) corresponds to a published report; the reports were untracked by owner ruling ef337c0f (bd rc-mq1sh) and are immutable once published and retrievable via tag \`bench/era-1-final\` (on-disk copies remain in the owner checkout). The`
   Lines 226-227 ("matrix is the **index** ...") stay byte-identical.
3. Insert the same single-line note from step 1 as its own paragraph
   (blank line before and after) immediately BEFORE the references
   list entry `- v1 report:` (~line 243).
4. Do NOT delete or retarget any existing relative links (they
   resolve in the owner checkout); the notes carry the fresh-clone
   retrieval story.

**Tests:** (executable spec — name, arrange, act, assert)
- `era1-tag-note-present`: after edits → `grep -c 'untracked (ef337c0f, bd rc-mq1sh)' benchmarks/scenarios/COVERAGE.md` → exactly 2, and `grep -c 'immutable once published and retrievable via tag' benchmarks/scenarios/COVERAGE.md` → exactly 1
- `overall-tag-count`: after edits → `grep -o 'bench/era-1-final' benchmarks/scenarios/COVERAGE.md | wc -l` → exactly 5 occurrences (2 single-line notes × 2 mentions each + 1 in the rewritten sentence); the phrase test above failing below 2 catches a note split across lines
- `stale-live-path-sentence-removed`: after edits → `grep -n 'corresponds to a published report at' benchmarks/scenarios/COVERAGE.md` → zero hits (exit 1); same for `grep -n 'YYYY-MM-DD-benchmark-vN'`
- `links-preserved`: after edits → `grep -c '](\.\./\.\./docs/benchmarks/history' benchmarks/scenarios/COVERAGE.md` → exactly 6 (the pre-change baseline; notes add link-form-free mentions only)

**Acceptance:**
- All four grep tests above pass from the worktree root.
- `git diff --stat` shows exactly one modified file under `benchmarks/`.

- [x] 1.1

### Task 1.2: re-verify the 8 MODIFIED blocks' factual anchors

**Files:**
- none modified (verification pass; records findings, fixes nothing —
  a failed anchor escalates back to the conductor, not to silent
  spec edits)

**Steps:**
1. For each anchor below, run the cited grep/read and confirm the
   quoted content exists at (or within ±5 lines of) the citation:
   - `benchmarks/harness/run.sh` ~2768: `M3_EXCLUDED_CONTENDERS="node-fastify node-native"` (Canonical full-matrix D5 exclusion)
   - `benchmarks/harness/summarize.py`: `M3_M4_EXCLUDED_CONTENDERS` mirror exists (same requirement)
   - `benchmarks/harness/test_summarize.py`: `test_roster_mirror_no_drift` exists (same requirement)
   - `benchmarks/harness/run.sh` ~3337-3351: meta-hygiene jq rewrite of `.scenarios` to active roster (Canonical full-matrix meta clause)
   - `benchmarks/harness/run.sh` ~872-875: `notice: skipping inactive scenario` (meta clause)
   - `benchmarks/contenders/axum-bare/src/`: zero `BENCH_HTTP_REQUEST` matches across src+tests (axum-bare T3 clause)
   - `benchmarks/harness/test_minimal_bare_shape.py` ~71-80: `assertNotIn("BENCH_HTTP_REQUEST", src)` (axum-bare T3 clause)
   - `benchmarks/scenarios/http-server/smoke/run.sh` ~195-197 and ~395-400: WARN-only id lines citing `e_opus D2` that never touch `FAIL`/`FAILED_ARTIFACTS` (axum-bare smoke clause)
   - `benchmarks/harness/test_warmup_policy.py` ~42: `"docs-investigation-strategy.md"` in REQUIRED_PHRASES (Zone contract level-1 doc pin)
   - `git ls-files benchmarks/audits/` returns 4 tracked files; `git ls-files benchmarks/docs-investigation-strategy.md` returns exactly that file (Zone contract level-1 listing)
   - `docs/adr/0066-metrics-collector-binding-and-lifetime.md` ~238 quotes `0.9890` and `[0.9785, 1.0126]`; `benchmarks/runner/RUNBOOK.md` ~171 records the lever-study ratio with the same bounds (Era-1 gauge premise)
   - `benchmarks/harness/run.sh` ~1716-1722: rust-camel-cli cells launched with `BENCH_LATENCY_FILE` / `BENCH_LATENCY_MODE=route` argv env (Warm tick direct-plumbing prose)
   - `git tag -l bench/era-1-final` non-empty; `git show bench/era-1-final:docs/benchmarks/history/2026-08-29-benchmark-v4-addendum.md` succeeds (Era-1 freeze)
   - `benchmarks/runner/run-all.sh` absence check + `grep -rn BENCH_SUBSET benchmarks/harness/run-all.sh` zero hits (Canonical full-matrix no-subset)
   - `benchmarks/contenders/node/package.json` is the single family package.json; `find benchmarks/contenders/node/node-native benchmarks/contenders/node/node-fastify -name package.json -print | wc -l` returns 0 AND both directories exist (`test -d` each; a comment mentioning package.json inside an .mjs is not a manifest)
   - `benchmarks/harness/loadgen/src/ratios.rs` ~37-42: percentile-not-BCa rationale comment (Ratio CI prose)
   - `benchmarks/harness/loadgen/src/payload.rs` ~21: `VALID_PAYLOAD_SIZES` four-size table, and `grep -rn BENCH_PAYLOAD_BYTES benchmarks/scenarios/split-aggregate/ benchmarks/scenarios/t2-realistic-eip/` returns zero hits (Payload-size axis scoping)
   - `benchmarks/harness/checks/m1-tolerance.py` ~12: `max(0.15 * pre, 3.0)` tolerance (Warm tick gate)
   - `benchmarks/contenders/rust-camel-lib/src/scenarios/t2-json.rs` ~264-266: marker latch inside first tick (Warm tick prose)
2. Record pass/fail per anchor in the task result; any failure halts
   the change and is reported (the spec text must not outlive a
   broken anchor).

**Tests:**
- `anchors-resolve`: each grep above → quoted anchor found at cited location → all 19 anchors pass
- `zero-bench-execution`: after the pass → `git status --short` in the worktree → exactly the Task 1.1 dirty set (` M benchmarks/scenarios/COVERAGE.md` and untracked change-dir artifacts only); nothing else touched by this task

**Acceptance:**
- All 19 anchors verified present; result recorded per anchor.
- No bench member executed (static grep/read only).

- [x] 1.2

### Task 1.3: change validation and gates

**Files:**
- none modified (gate execution; findings route back to tasks 1.1/1.2
  or to the conductor)

**Steps:**
1. `openspec validate benchspecsync --type change --json` → valid true.
2. Run the 12 xtask lints from the AGENTS.md QUALITY GATES block,
   each invoked from the worktree root as
   `RUSTC_WRAPPER= cargo xtask lint-unwrap` style commands: lint-unwrap,
   lint-secrets, lint-non-exhaustive, lint-log-levels,
   lint-log-redaction, lint-ignore, lint-publish-cycles,
   lint-publish-registration, lint-component-deps,
   lint-gate-forwarding, lint-context-citations, lint-metric-labels.
3. `RUSTC_WRAPPER= cargo xtask schema --check`.
4. N/A gates (docs-only diff, no `.rs`/`Cargo.toml` touched):
   fmt, clippy (all three invocations), doc-build, lint-commits
   (conductor deviation — remote op), cargo-audit (not in the mission
   gate list; no dependency changes — record as not-run with reason).

**Tests:**
- `validate-clean`: `openspec validate benchspecsync --type change --json` → `.summary.totals.failed == 0`
- `xtask-lints-green`: each of the 12 lint commands → exit 0
- `schema-check-green`: `RUSTC_WRAPPER= cargo xtask schema --check` → exit 0

**Acceptance:**
- All three test groups pass; any failure loops back before review.
- Gate-coverage self-check enumerates every gate with pass/N-A status.

- [x] 1.3
