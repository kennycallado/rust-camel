# Tasks: bench-era-2

## Phase 1: Zones

### Task 1.1: Attic moves and reference hygiene

**Files:**
- `benchmarks/spikes/` (moved to `benchmarks/attic/spikes/`)
- `benchmarks/spike-results.md` (moved to `benchmarks/attic/spike-results.md`)
- `benchmarks/quarkus-native-throughput-diagnosis.md` (moved to `benchmarks/attic/quarkus-native-throughput-diagnosis.md`)
- `benchmarks/results-published/` (moved to `benchmarks/attic/results-era-1/`)
- `benchmarks/builder/` (moved to `benchmarks/harness/builder/`)
- `benchmarks/attic/README.md` (new)
- `benchmarks/harness/run-all.sh` (modified: builder path reference fix)

**Steps:**
1. `mkdir benchmarks/attic` then `git mv benchmarks/spikes benchmarks/attic/spikes`
2. `git mv benchmarks/spike-results.md benchmarks/attic/spike-results.md`
3. `git mv benchmarks/quarkus-native-throughput-diagnosis.md benchmarks/attic/quarkus-native-throughput-diagnosis.md`
4. `git mv benchmarks/results-published benchmarks/attic/results-era-1`
5. `git mv benchmarks/builder benchmarks/harness/builder`
6. Write `benchmarks/attic/README.md` as exactly one line: `Era-1 artifacts (spikes, raw results, dead prose). Frozen evidence lives at git tag bench/era-1-final.`
7. Fix dangling references in LIVE code and scripts only (scope: `benchmarks/harness/*.sh`, `benchmarks/README.md`, `.github/workflows/ci.yml`): `rg -l "benchmarks/(spikes|spike-results|results-published|builder)" benchmarks/harness .github` and rewrite each hit to the new path. Do NOT edit files under `docs/benchmarks/`, `benchmarks/attic/`, or `openspec/` (era-1 reports are immutable).

**Tests:** (executable spec)
- `attic-level-audit`: repo at HEAD after moves → `ls benchmarks/` → output contains `attic`, `harness`, `scenarios`, `runner` and does NOT contain `spikes`, `results-published`, `builder`, `spike-results.md`, `quarkus-native-throughput-diagnosis.md`.
- `no-dangling-refs-live-scope`: `rg -l "benchmarks/(spikes|spike-results|results-published|builder)" benchmarks/harness .github` → exits 1 (zero hits).

**Acceptance:**
- `git status --porcelain` shows renames (R), the new `attic/README.md`, and at most path-fix modifications in `benchmarks/harness/*.sh` (live scope).
- Both tests above pass.
- `cargo build --workspace` exits 0 (`builder` move verified by `cargo metadata` resolving).
- `cargo test -p bench-loadgen` exits 0 — golden digests and harness tests pass unmodified (spec scenario "Harness moves without modification"; the scenario's move is `builder/` → `harness/builder/`, the only harness-zone relocation in this change).

- [x] 1.1

### Task 1.2: Glossary confinement, coverage index, quiet-host criteria

**Files:**
- `benchmarks/CONTEXT.md` (moved to `benchmarks/harness/CONTEXT.md`, content extended)
- `benchmarks/scenarios/COVERAGE.md` (moved from `benchmarks/COVERAGE.md`, links fixed)

**Steps:**
1. `git mv benchmarks/CONTEXT.md benchmarks/harness/CONTEXT.md`
2. `git mv benchmarks/COVERAGE.md benchmarks/scenarios/COVERAGE.md`
3. In `benchmarks/harness/CONTEXT.md`, append section `## Quiet-host criteria (canonical run)` defining the measurable gates: host 1-min load average below 3.0 for 10 consecutive minutes before start; devnull-baseline warmup within ±5% across its last 3 probes; no concurrent cargo/gradle/maven processes (`pgrep -c 'cargo|gradle|mvn'` returns 0). State that a run not meeting these is void.
4. In `benchmarks/scenarios/COVERAGE.md`: prepend one line `Technical coverage index for scenarios. Owner-facing view: benchmarks/README.md.`; then fix relative links that broke with the depth change — every link of the form `../docs/benchmarks/…` becomes `../../docs/benchmarks/…`, and bare report links `](2026-…​.md)` / `](consultation-….md)` (two known: `2026-07-18-benchmark-v2.md`, `2026-07-18-startup-minimal-benchmark.md`) become `../../docs/benchmarks/<same>.md` (temporary stopgap; Task 3.3 retargets them to `history/`).
5. Fix repo references to the old `benchmarks/CONTEXT.md` / `benchmarks/COVERAGE.md` paths in live scope (harness scripts, CI, README).

**Tests:**
- `glossary-moved`: `test -f benchmarks/harness/CONTEXT.md && test ! -f benchmarks/CONTEXT.md` → exit 0.
- `quiet-host-defined`: `rg -c "1-min load average below 3.0" benchmarks/harness/CONTEXT.md` → exits 0 with count ≥ 1.
- `coverage-links-valid`: for every markdown-link target matched by `rg -o "\]\([^)#]+\.md\)" benchmarks/scenarios/COVERAGE.md`: resolve relative to the file's dir → each target exists on disk (covers depth-fixed and formerly-bare links).

**Acceptance:**
- `ls benchmarks/*.md` lists exactly `README.md`.
- The quiet-host criteria section exists with all three numeric gates (load < 3.0, devnull ±5%, pgrep 0).
- All COVERAGE.md relative links resolve.

- [x] 1.2

### Task 1.3: README diet and records seed

**Files:**
- `benchmarks/README.md` (modified, rewritten)
- `benchmarks/records/index.json` (new)

**Steps:**
1. Rewrite `benchmarks/README.md` (English, under 40 lines) using only the five public words — run (corrida), scenario (escenario), contender (contendiente), date (fecha), record (registro): what a run is, how to start one (`bench run`), where records live (`records/`, indexed by date), and where technical depth lives (`harness/CONTEXT.md` — linked, not summarized). No metric-family codes, no T-family codes, no version numbers.
2. `mkdir benchmarks/records` and write `benchmarks/records/index.json` with the exact seed content: `{"index_schema_version": 1, "runs": []}` (valid JSON, trailing newline).

**Tests:**
- `readme-diet`: `rg -n "M[1-4]\b|T[1-4][a-z]?|v[0-9]|paired|bootstrap|payload axis" benchmarks/README.md` → exits 1 (zero hits).
- `records-seeded`: `python3 -c "import json; d=json.load(open('benchmarks/records/index.json')); assert d=={'index_schema_version':1,'runs':[]}"` → exit 0.

**Acceptance:**
- README passes the diet scan above.
- `benchmarks/records/index.json` exists, parses, and matches the seed exactly.

- [x] 1.3

### Task 1.4: bench facade with run passthroughs

**Files:**
- `benchmarks/bench` (new, executable bash script)
- `.github/workflows/ci.yml` (modified: bench-smoke job gains facade smoke steps)

**Steps:**
1. Write `benchmarks/bench` (bash, `set -euo pipefail`): `run` subcommand execs `harness/run.sh` forwarding `"$@"` and the environment unchanged (no variable munging); `run-all` subcommand execs `harness/run-all.sh` the same way (container orchestration path — consumed by Task 3.2); `summarize` and `publish` subcommands exit 2 with `not implemented until Phase 2` on stderr (implemented in Task 2.3); `help` prints usage for the five subcommands.
2. `chmod +x benchmarks/bench`.
3. In the CI bench-smoke job, add two steps before the criterion bench step: (a) `bash benchmarks/bench help` (facade present), (b) `bash benchmarks/bench run --scenarios=t2-json,split-aggregate --dry-run` (spec scenario "bench-smoke job" — the dry-run path requires no JDK and compiles only already-built workspace members); (c) update the job's `# Container full-matrix stays manual:` comment to point at `bench run-all` instead of `harness/run-all.sh` (design: "the job's comment pointing at run-all.sh updates to the facade").

**Tests:**
- `facade-help`: `bash benchmarks/bench help` → exit 0, stdout contains `run`, `run-all`, `summarize`, `publish`.
- `facade-dry-run-no-jdk`: env without JAVA_HOME → `bash benchmarks/bench run --scenarios=t2-json,split-aggregate --dry-run` → exits 0, output contains `resolved cells:` and `=== dry-run complete (no contender invoked)`, no JVM invocation (spec scenario "Dry run via facade").
- `facade-passthrough-parity`: run `bash benchmarks/harness/run.sh --scenarios=t2-json --dry-run` and then `bash benchmarks/bench run --scenarios=t2-json --dry-run`; capture full stdout of both → both exit 0, print identical `resolved cells: <N> (expected: <N>)` lines, and their numbered cell lines are identical as a SET after stripping the `  <i>. ` index prefix and sorting (run.sh shuffles the dry-run order with time entropy — raw line order is not comparable; verified: dry-run output lists cells only, it prints no digests). Spec scenario "Run passthrough parity" (dry-run proxy; byte-identical result digests are the full-run claim, discharged by the human v1 run in Phase 3).
- `zone-contract-full`: AFTER the parity test cleans up any `benchmarks/results/` it created (`rm -rf benchmarks/results` — gitignored, regenerated), run `ls -A benchmarks/ | sort` → output is exactly: `README.md`, `attic`, `bench`, `harness`, `records`, `runner`, `scenarios` (seven entries, nothing else).

**Acceptance:**
- All four tests pass.
- `bash -n benchmarks/bench` exits 0.
- CI bench-smoke job still fits `timeout-minutes: 10` (dry-run adds no new compile beyond existing workspace members).

- [x] 1.4

## Phase 2: Records layer

### Task 2.1: run.json schema document

**Files:**
- `benchmarks/records/SCHEMA.md` (new)

**Steps:**
1. Write `benchmarks/records/SCHEMA.md` documenting `run.json` v1 exactly: top-level fields `schema_version` (integer, 1), `run_id` (format `<YYYYMMDD>-v<N>` — date-first for lexicographic sortability; global sequence continuing era-1 numbering, first era-2 record is `20260905-v5` style), `era` (string: `"1"` or `"2"` — string not integer so the vocabulary can grow without type change), `date` (ISO-8601), `git_commit` (40-hex), `container_digest` (`sha256:<64hex>` or `null` for non-container runs), `host_provenance` (object: cpu model, cores, kernel, load snapshot, `containerized` boolean), `protocol` (object: `rounds`, `duration_secs`, `warmup_secs`, `order_seed`), `cells` (array of `{scenario, contender, variant, payload_class, metric, round_values[], median, unit, input_sha256}`), `ratios` (array of `{numerator, denominator, metric, point, ci_lo, ci_hi, method}`).
2. Document the forward-compatibility rule (papal note): consumers MUST ignore unknown fields; producers MUST bump `schema_version` on any breaking change; additive fields are minor (no bump within v1).
3. Document `index.json` as an OBJECT: `{"index_schema_version": 1, "runs": [entry, entry]}` with entries `{run_id, date, era, git_commit, subset, path}`; `runs` ordered by date ascending (papal note: versioned index shape).
4. Document the canonical public path rule: records are served verbatim under the docs site at `benchmarks/records/` (relative fetch: `records/index.json`).

**Tests:**
- `schema-doc-fields`: `rg -c "schema_version|run_id|container_digest|index_schema_version|git_commit" benchmarks/records/SCHEMA.md` → all five present.
- `schema-doc-fwd-compat`: `rg -c "MUST ignore unknown fields" benchmarks/records/SCHEMA.md` → ≥ 1.

**Acceptance:**
- SCHEMA.md exists with every field above, the forward-compat rule, the versioned-object index shape with `git_commit` entries, and the public-path rule.
- `openspec validate bench-era-2 --type change` still passes (spec untouched).

- [x] 2.1

### Task 2.2: summarize.py record builder

**Files:**
- `benchmarks/harness/summarize.py` (new, python3 stdlib only)
- `benchmarks/harness/test_summarize.py` (new, unittest stdlib)

**Steps:**
1. Implement `summarize.py` with functions reading the REAL run.sh output layout (verified: run.sh writes `$RUN_DIR/<cell>/m2-summary.json`, `m3-summary.json`, `m4-summary.json`, and raw `samples.txt` for m1 cold-start — there is NO m1 summary JSON): `load_cells(run_dir) -> list[dict]` reads each `<cell>/` dir — m2/m3/m4 from their `*-summary.json`, m1 from parsing numeric startup-ms lines in `<cell>/samples.txt` (median computed in summarize; if a samples file has non-numeric header lines, skip them explicitly); `build_record(run_dir, meta) -> dict` (assembles the `run.json` object per SCHEMA.md; medians computed as `statistics.median` of round values or samples; floats formatted via `repr(float(x))` for each numeric `x`), `emit_json(record, out_dir)` and `emit_summary(record, out_dir)` (markdown tables: one per metric across contenders; ratio table with point, ci_lo, ci_hi), and `compute_ratios(record) -> list[dict]` which SHELLS OUT to `bench-loadgen aggregate-ratios` (single bootstrap implementation, no duplicate math; parse its JSON stdout). The aggregate-ratios binary is located via env `BENCH_AGGREGATE_RATIOS_BIN` (default `cargo run -p bench-loadgen --bin bench-loadgen -- aggregate-ratios` split into argv).
2. Determinism: all dict emission uses `sort_keys=True`, `indent=2`; two invocations byte-identical.
3. `main(argv)`: `--run-dir`, `--meta path.json` (git_commit, container_digest, era, protocol, host_provenance), `--out-dir`; writes `run.json` + `summary.md`.
4. Tests in `test_summarize.py` with synthetic fixtures: crafted `cells/*.json` with known values (two contenders × two rounds, values [100.0, 102.0] → median 101.0), golden `run.json` asserted field-by-field; determinism test (emit twice, bytes equal); a `compute_ratios` test with env `BENCH_AGGREGATE_RATIOS_BIN` pointing to a stub script that prints a fixed ratio JSON.

**Tests:**
- `test_build_record_median`: synthetic cells values [100.0, 102.0] → `build_record` produces cell with `median == 101.0`.
- `test_determinism`: `emit_json` twice → files byte-identical.
- `test_schema_fields`: built record has all SCHEMA.md top-level keys with `schema_version == 1` and `era` a string.
- `test_ratio_delegation`: stub `BENCH_AGGREGATE_RATIOS_BIN` prints the full SCHEMA ratio shape `{"numerator": "rust-camel-lib", "denominator": "camel-standalone-dsl", "metric": "m3", "point": 1.5, "ci_lo": 1.4, "ci_hi": 1.6, "method": "bootstrap-paired"}` → record's `ratios[0]` equals it verbatim.
- Command: `cd benchmarks/harness && python3 -m unittest test_summarize -v`.

**Acceptance:**
- `cd benchmarks/harness && python3 -m unittest` passes all tests (discovery finds `test_*.py`).
- No third-party imports (`rg "^import |^from " benchmarks/harness/summarize.py` → stdlib only).
- `python3 -m py_compile benchmarks/harness/summarize.py` exits 0.

- [x] 2.2

### Task 2.3: publish, index, checksum guard

**Files:**
- `benchmarks/harness/summarize.py` (modified: add `publish` mode and `--check` mode)
- `benchmarks/bench` (modified: implement `summarize` and `publish`)
- `benchmarks/harness/test_publish.py` (new)
- `.github/workflows/ci.yml` (modified: records-guard step in bench-smoke job)

**Steps:**
1. Implement publish mode: read a validated run dir (must contain `run.json`), copy it to `records/<run_id>/`, regenerate `records/index.json` as `{"index_schema_version": 1, "runs": [entry, entry]}` (date-ascending, entries `{run_id, date, era, git_commit, subset, path}`), refuse if `run_id` already exists with different content (identical content is a no-op success).
2. Checksum guard: `summarize.py --check <records-dir>` regenerates every `summary.md` from its sibling `run.json` into a temp dir and diffs; any mismatch (hand-edited summary) exits 1 naming the file. A records dir with zero `run.json` files is a green no-op.
3. Wire `bench summarize` / `bench publish` to the above (`bench summarize --run-dir X --meta Y --out-dir Z`; `bench publish --run-dir Z`).
4. Add CI step in bench-smoke: `python3 benchmarks/harness/summarize.py --check benchmarks/records` (green no-op while records holds only `index.json` + `SCHEMA.md`; catches future drift).

**Tests:**
- `test_publish_appends_index`: temp records dir seeded with the Phase-1 index object → publish two synthetic runs → index has 2 entries in `runs`, ordered by date, each with `git_commit` and `path` pointing to its dir.
- `test_publish_refuses_duplicate`: publish same run_id with different content twice → second exits non-zero.
- `test_check_detects_hand_edit`: published record, append ` x` to summary.md → `--check` exits 1 naming that summary.
- `test_check_empty_records`: records dir with index seed only → `--check` exits 0.
- `bench-facade-wired`: `bash benchmarks/bench summarize` with no args → usage error exit 2 (not "not implemented").
- Command: `cd benchmarks/harness && python3 -m unittest test_publish -v`.

**Acceptance:**
- All tests pass (`cd benchmarks/harness && python3 -m unittest`).
- CI workflow passes YAML parse (`python3 -c "import yaml; yaml.safe_load(open('.github/workflows/ci.yml'))"`).
- Spec scenarios "Index updated on publish", "Schema rejects hand-typed drift" exercised by passing tests; "Static fetch" satisfied by `python3 -c "import json; json.load(open('benchmarks/records/index.json'))"` green.

- [x] 2.3

## Phase 3: Canonical run and freeze

### Task 3.1: Digest-pinned runner

**Files:**
- `benchmarks/runner/Dockerfile` (modified — the file exists; audit and adjust to the canonical toolchain below)
- `benchmarks/runner/pin.sh` (new)
- `benchmarks/runner/README.md` (new, one paragraph)
- `benchmarks/harness/summarize.py` (modified: `--check` also rejects `:latest` in any `container_digest`-bearing record)
- `benchmarks/harness/test_summarize.py` (modified: new rejection test)

**Steps:**
1. Audit existing `benchmarks/runner/Dockerfile`; adjust (or confirm) it builds the canonical runner: rust toolchain + temurin 21 + gradle/maven, mirroring the nix-shell environment used by validated smokes.
2. Write `pin.sh`: builds the image via `docker build --iidfile`, then resolves the image digest and writes it to `benchmarks/runner/DIGEST` as `sha256:<64hex>`. Digest source: the `--iidfile` image-ID digest — for a locally built image `docker image inspect --format '{{index .RepoDigests 0}}'` is EMPTY (`RepoDigests` is only populated by a registry pull; use `RepoDigests` only if the image was pushed). The script FAILS if the resolved reference is a mutable tag or does not match `sha256:<64hex>`.
3. Extend `summarize.py --check`: any record whose `container_digest` ends in `:latest` (or is empty/null while `host_provenance.containerized` is true) → exit 1 naming the record.
4. Write `benchmarks/runner/README.md` (one paragraph): the digest in `DIGEST` is the runner identity; tags are convenience only; canonical runs consume `DIGEST`.

**Tests:**
- `test_digest_rejects_latest`: crafted record with `"container_digest": "runner:latest"` → `--check` exits 1 (unit test in `test_summarize.py`).
- `pin-sh-syntax`: `bash -n benchmarks/runner/pin.sh` → exit 0.
- `pin-sh-no-latest`: `rg ":latest" benchmarks/runner/pin.sh` → zero hits.

**Acceptance:**
- `cd benchmarks/harness && python3 -m unittest` passes including `test_digest_rejects_latest`.
- Spec scenario "Digest recorded, tag rejected" exercised by the unit test.
- Actual `docker build` + `pin.sh` execution happens in Task 3.2 preparation (needs the human's docker daemon; not a unit gate).

- [x] 3.1

### Task 3.2: v1 run preparation (human-executed run)

**Files:**
- `benchmarks/runner/RUNBOOK.md` (new)
- `benchmarks/harness/run-all.sh` (modified: subset pinning, digest enforcement, results redirect)
- `benchmarks/harness/run.sh` (modified: `BENCH_RESULTS_ROOT` env override for `RESULTS_ROOT` — default value unchanged, purely additive plumbing; measurement semantics untouched)
- `.gitignore` (modified: add `benchmarks/harness/out/`)
- `docs/adr/0066-metrics-collector-binding-and-lifetime.md` (modified: gauge A/B citation appended — use this exact filename; `rg -l "ADR-0066" docs/adr/` matches two files because ADR-0012 cross-references it)

**Steps:**
1. In `run.sh`: honor `BENCH_RESULTS_ROOT` when set (`RESULTS_ROOT="${BENCH_RESULTS_ROOT:-$REPO_ROOT/benchmarks/results}"`) — the only change to run.sh in this change.
2. In `run-all.sh`: (a) add env `BENCH_SUBSET=v1` mapping to `startup-minimal,http-server,t2-json,split-aggregate` (the validated families) with `--print-subset` flag support; (b) enforce digest identity — default `IMAGE_NAME` resolves from `benchmarks/runner/DIGEST` (run by digest); if `IMAGE_NAME` is explicitly overridden to a mutable tag, exit 1 with a pointer to `pin.sh` (guard ordered BEFORE any docker call so `--print-subset` never reaches docker); (c) set `BENCH_RESULTS_ROOT` to `benchmarks/harness/out/<ts>/` for the container invocation (gitignored; keeps the seven-entry zone contract intact at level 1 during runs); (d) echo the resolved digest + `git_commit` + quiet-host snapshot into the run dir's `meta.json` for `bench summarize --meta`.
2. Write `RUNBOOK.md`: exact human procedure — (a) verify quiet-host criteria from `harness/CONTEXT.md` (commands included verbatim); (b) `docker build` + `bash benchmarks/runner/pin.sh`; (c) the single command `BENCH_SUBSET=v1 bash benchmarks/bench run-all` (facade dispatches to run-all.sh — the container orchestration path); (d) expected wall-clock (~3h) and expected artifacts under `benchmarks/harness/out/`; (e) after completion: `bash benchmarks/bench summarize --run-dir <out> --meta <out>/meta.json --out-dir <record-dir>` then `bash benchmarks/bench publish --run-dir <record-dir>`; (f) gauges ON note citing ADR-0066 and the A/B verdict 0.9890 [0.9785, 1.0126] (cost unresolved from zero).
3. Payload axis as COMPANION (papal note: decoupled, not a gate): document in RUNBOOK an optional second invocation `BENCH_SUBSET=v1 BENCH_PAYLOAD_AXIS=1 …` covering 2 reference contenders × 4 payload classes; state explicitly that its absence does NOT void the v1 record.
4. Verify ADR-0066 carries the 0.9890 citation (verified baseline today: it does not — `grep -c '0\.9890' docs/adr/0066-metrics-collector-binding-and-lifetime.md` returns 0); append one line + pointer to the v4 addendum (closes e_glm bless fix 4).
5. Dry-run gate in the worktree: `bash benchmarks/bench run --scenarios=startup-minimal,http-server,t2-json,split-aggregate --dry-run` exits 0 (JVM-less path).

**Tests:**
- `subset-maps`: `BENCH_SUBSET=v1 bash benchmarks/harness/run-all.sh --print-subset` → prints exactly the four families.
- `mutable-tag-refused`: `IMAGE_NAME=benchmark-runner:latest bash benchmarks/harness/run-all.sh --print-subset` → exits 1 with pin.sh pointer (print-subset short-circuits before docker, so the guard must run first — order the guard before any docker call).
- `runbook-complete`: `rg -c "quiet-host|pin.sh|bench run-all|summarize|publish|ADR-0066" benchmarks/runner/RUNBOOK.md` → all six present.
- `dryrun-v1`: the dry-run command above → exit 0 in the worktree.

**Acceptance:**
- All tests pass; dry-run green from the facade.
- RUNBOOK single-command execution for the human (`bench run-all`), with summarize/publish as the post-run pair.
- Spec scenarios "Human-invoked execution" (agent deliverable ends at preparation) and "v1 record lands" preconditions (digest enforcement, quiet-host criteria, meta capture) fully prepared.

- [x] 3.2

### Task 3.3: Era-1 freeze and records publication wiring

**Files:**
- `docs/benchmarks/history/` (new dir; 8 era-1 reports moved into it: `2026-07-18-benchmark-v2.md`, `2026-07-18-startup-minimal-benchmark.md`, `2026-07-21-benchmark-v3.md`, `2026-07-22-benchmark-v4.md`, `2026-08-29-benchmark-v4-addendum.md`, `consultation-v3-direction-2026-07-18.md`, `consultation-v3-followup-2026-07-18.md`, `e_opus-quarkus-native-and-bridge-analysis.md`)
- `docs/benchmarks/direction-check-bench-era-2-2026-08-30.md` (committed — currently untracked in the worktree; the era-2 evidence must not be lost on merge)
- `docs/benchmarks/history/README.md` (new, 3 lines)
- `benchmarks/scenarios/COVERAGE.md` (modified: report links retargeted to `history/`)
- `benchmarks/runner/RUNBOOK.md` (modified: final line — tag instruction)

**Steps:**
1. `mkdir docs/benchmarks/history` and `git mv` the eight era-1 reports into it; `git add docs/benchmarks/direction-check-bench-era-2-2026-08-30.md` (the orientation doc is already tracked). The two era-2 docs (`orientation-benchmark-restructure-2026-08-30.md`, `direction-check-bench-era-2-2026-08-30.md`) stay at `docs/benchmarks/`.
2. Prepend one line to each moved report: `> Era-1 report. Frozen at git tag bench/era-1-final. Live data: benchmarks/records/.`
3. Retarget `benchmarks/scenarios/COVERAGE.md` links that pointed at `../../docs/benchmarks/<report>.md` to `../../docs/benchmarks/history/<report>.md`.
4. Write `docs/benchmarks/history/README.md` (3 lines): tag name `bench/era-1-final`, immutability statement, pointer to `benchmarks/records/` for live data.
5. Append the freeze instruction as the final line of `benchmarks/runner/RUNBOOK.md`: `After this change lands on main (merge commit), create the tag: git tag bench/era-1-final && keep it local until push (pushing tags is the human's action with the branch push).`
6. Verify docs tooling unaffected: `rg "docs/benchmarks" docs/src/SUMMARY.md` → zero hits (no mdbook page references the moved files; if hits appear, update those page paths instead).

**Tests:**
- `history-moved`: `ls docs/benchmarks/` → contains `history/`, the two era-2 docs, and nothing else; `git ls-files docs/benchmarks/` includes both era-2 docs (tracked, not untracked).
- `banner-present`: `rg -l "^> Era-1 report\. Frozen" docs/benchmarks/history/*.md | wc -l` → 8 (anchored banner; the history README cannot trip it).
- `coverage-links-history`: `rg -o "\]\((\.\./)+docs/benchmarks/(?!history/)[^)]+\)" -P benchmarks/scenarios/COVERAGE.md` → zero hits; and every `history/` link resolves on disk.
- `runbook-has-tag-line`: `rg -c "bench/era-1-final" benchmarks/runner/RUNBOOK.md` → ≥ 1.

**Acceptance:**
- All tests pass.
- Spec scenario "Reports reachable after freeze" prepared: the human creates the tag post-merge per the RUNBOOK instruction (documented, not agent-executed — pushing is human-only).
- Docs build/config unaffected (SUMMARY.md check).

- [x] 3.3

### Task 3.4: Post-run record validation (human-gated)

**Files:**
- `benchmarks/runner/RUNBOOK.md` (modified: validation checklist section)

**Steps:**
1. Append a `## Post-run validation` section to RUNBOOK.md: after the human executes the v1 run and publish (Task 3.2 steps), the agent-side validation checklist is — (a) `python3 benchmarks/harness/summarize.py --check benchmarks/records` green; (b) `records/index.json` gained its first era-2 entry (`era: "2"`, `run_id` format `<YYYYMMDD>-v5`); (c) the published `run.json` conforms to SCHEMA.md (schema_version 1, all top-level keys, `container_digest` = sha256 from `runner/DIGEST`, `protocol.order_seed` present, every cell has `input_sha256`); (d) gauges ON evidence in cells (metric fields include gauge readings); (e) summary tables contain no number absent from `run.json` (guard proves it mechanically).
2. State in the section: the checkbox tick covers the CHECKLIST DELIVERABLE only; the record gate itself is human-gated and tracked by bd rc-f4po (tick does not assert a published v1 record).

**Tests:**
- `runbook-validation-section`: `rg -c "Post-run validation" benchmarks/runner/RUNBOOK.md` → ≥ 1; `rg -c "\-\-check benchmarks/records" benchmarks/runner/RUNBOOK.md` → ≥ 1.
- `check-green-on-seed`: `python3 benchmarks/harness/summarize.py --check benchmarks/records` → exit 0 (validates the guard stays green pre-run with the seeded index).

**Acceptance:**
- Both tests pass (the section and the guard).
- Spec scenario "v1 record lands" postcondition ownership is explicit: discharged by this checklist when the human's run lands — not by any earlier task.

- [x] 3.4
