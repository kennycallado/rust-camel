# Tasks: bench-consol-tick

## Phase A: Fixture consolidation (structural)

## Consolidated lib contender crate

### Task 1.1: Create consolidated `rust-camel-lib` crate at `benchmarks/contenders/`

**Files:**
- `benchmarks/contenders/rust-camel-lib/Cargo.toml` (new)
- `benchmarks/contenders/rust-camel-lib/src/main.rs` (new)
- `benchmarks/contenders/rust-camel-lib/src/scenarios/startup-minimal.rs` (new)
- `benchmarks/contenders/rust-camel-lib/src/scenarios/http-server.rs` (new)
- `benchmarks/contenders/rust-camel-lib/src/scenarios/t2-json.rs` (new)
- `benchmarks/contenders/rust-camel-lib/src/scenarios/split-aggregate.rs` (new)
- `benchmarks/contenders/rust-camel-lib/src/scenarios/t2-realistic-eip.rs` (new)
- `benchmarks/contenders/rust-camel-lib/src/scenarios/xsd-validation-bridge.rs` (new)
- `benchmarks/contenders/rust-camel-lib/src/scenarios/xslt-bridge.rs` (new)
- `benchmarks/contenders/rust-camel-lib/.cargo/config.toml` (new)
- `Cargo.toml` (modified — workspace members)

**Steps:**
1. Pre-warm the worktree build caches from the main checkout via
   hardlinks (same FS, near-zero disk cost). The REAL wins are
   `benchmarks/.cache` (cargo/m2/gradle) and the quarkus `build/` dirs
   (native runners + fingerprints so quarkus cells skip rebuilds):
   `cp -al /home/kenny/dev/rust-camel/benchmarks/.cache <worktree>/benchmarks/.cache`
   and for each `benchmarks/scenarios/<scn>/camel-quarkus/camel-quarkus-*`:
   `cp -al <main>/<dir>/build <worktree>/<dir>/build`. The root
   `target/` hardlink pre-warm is included but expect partial
   fingerprint revalidation (path-dependent fingerprints) — the docker
   builder cache (shared daemon) carries most of the native rebuild
   cost anyway.
2. Create package `rust-camel-lib-fixture` (bin `rust-camel-lib-fixture`)
   with `[workspace]`-inheriting deps mirroring the union of the seven
   per-scenario fixture crates' `Cargo.toml` deps. CAREFUL: the fixture
   crates use RELATIVE path deps 4 levels up
   (`../../../../crates/camel-core`); the new location is 3 levels
   below root — rewrite every path dep to `../../../crates/…` (cargo
   fails loudly on stale depths; that failure is the check).
3. Port each scenario's route-building code from
   `benchmarks/scenarios/<scn>/rust-camel-lib/src/main.rs` into
   `src/scenarios/<scn>.rs` as `pub fn run() -> i32` — code moved
   verbatim where possible (route construction, marker emission
   contract, `BENCH_INPUT_SHA256` logging, `BENCH_PAYLOAD_BYTES` body
   building, `BENCH_LATENCY_FILE` reading where the scenario uses it —
   xsd-validation-bridge `main.rs:47,100` is the reference).
4. `src/main.rs`: `argv[1]` = scenario name, dispatch via a `match` to
   `scenarios::<scn>::run()`; unknown/missing scenario prints the valid
   names to stderr and exits 2. NO other work before the dispatched
   `run()` (dispatch-then-build guard, blessed design §Measurement
   integrity).
5. Copy `.cargo/config.toml` with `target-dir = "target"` verbatim from
   any existing fixture (fixture-local pin, `benchmarks/harness/CONTEXT.md`
   §2/§4).
6. Root `Cargo.toml`: remove the seven
   `benchmarks/scenarios/<scn>/rust-camel-lib` member paths, add
   `"benchmarks/contenders/rust-camel-lib"`, keep it OUT of
   `default-members` (member-non-default pattern).

**Tests:** (executable spec)
- `dispatch_builds_selected_only`: workspace built → `env -u CARGO_TARGET_DIR cargo build --release -p rust-camel-lib-fixture` once → binary exists at `benchmarks/contenders/rust-camel-lib/target/release/rust-camel-lib-fixture` → assert: single build serves all scenarios (no per-scenario crates referenced by `cargo metadata`).
- `unknown_scenario_exits_2`: run the binary with argv `nope` → assert exit code 2 and stderr lists the 7 valid scenario names.
- `marker_per_scenario`: for each of the 7 scenarios run the binary with the scenario's env (as wired in `benchmarks/harness/run.sh`) → assert stdout contains exactly 1 occurrence of the scenario's marker contract string (startup-minimal: `BENCH_ROUTE_READY`; t2-json: `BENCH_ROUTE_READY bytes=<len>`; bridge markers per their fixtures; NOTE: t2-realistic-eip emits the bare `BENCH_ROUTE_READY` substring twice plus `body=` once — key asserts on distinctive per-scenario strings, never the bare substring) AND zero occurrences of any OTHER scenario's marker string (non-selected builders never ran).

**Acceptance:**
- `env -u CARGO_TARGET_DIR cargo build --release -p rust-camel-lib-fixture` exits 0 in the worktree.
- `cargo metadata --no-deps` lists `benchmarks/contenders/rust-camel-lib` as a workspace member and NO `benchmarks/scenarios/*/rust-camel-lib` member remains.
- `cargo clippy -p rust-camel-lib-fixture -- -D warnings` exits 0.

- [x] 1.1

### Task 1.2: Delete the seven per-scenario lib fixture crates; fix references

**Files:**
- `benchmarks/harness/checks/extract-digests.py` (new — committed digest-oracle extractor)
- `openspec/changes/bench-consol-tick/pre-move-digests.json` (new — the committed oracle)
- `benchmarks/scenarios/startup-minimal/rust-camel-lib/` (deleted)
- `benchmarks/scenarios/http-server/rust-camel-lib/` (deleted)
- `benchmarks/scenarios/t2-json/rust-camel-lib/` (deleted)
- `benchmarks/scenarios/split-aggregate/rust-camel-lib/` (deleted)
- `benchmarks/scenarios/t2-realistic-eip/rust-camel-lib/` (deleted)
- `benchmarks/scenarios/xsd-validation-bridge/rust-camel-lib/` (deleted)
- `benchmarks/scenarios/xslt-bridge/rust-camel-lib/` (deleted)
- `benchmarks/harness/builder/build-all.sh` (modified — build the one crate)
- `benchmarks/harness/run.sh` (modified — binary resolution)

**Steps:**
1. BEFORE deleting anything, capture the committed digest oracle: run
   an in-container `--metric=m1 --n=1` smoke over ALL 7 scenarios with
   the OLD fixtures still in place; extract every cell's
   `BENCH_INPUT_SHA256` line into
   `openspec/changes/bench-consol-tick/pre-move-digests.json`
   using the NEW committed script
   `python3 benchmarks/harness/checks/extract-digests.py <run_dir> openspec/changes/bench-consol-tick/pre-move-digests.json`
   (emits `{"<scenario>_<contender>": "<sha256>", …}`, 16 entries —
   digests exist ONLY for t2-json + split-aggregate, the only scenarios
   with a payload axis; the other 5 emit no `BENCH_INPUT_SHA256` by
   design, so a 52-entry oracle is impossible. Cell ROSTER stays 52;
   only the digest oracle is 16. Greps the scratch `.out` evidence for
   `BENCH_INPUT_SHA256=` lines)
   and commit script + json with this task. This is the ONLY digest
   oracle Task 1.7 compares against (COVERAGE.md and scenario READMEs
   carry no full digests).
2. `git rm -r` the seven fixture dirs.
3. `rg -n "scenarios/[^ ]*/rust-camel-lib" benchmarks/ docs/ openspec/ --glob '!**/archive/**'` — every live hit updated to `benchmarks/contenders/rust-camel-lib`; archived changes and `docs/benchmarks/history/**` stay untouched (history).
4. `build-all.sh`: replace the per-fixture cargo loop for lib fixtures with one `env -u CARGO_TARGET_DIR cargo build --release -p rust-camel-lib-fixture`.
5. `run.sh` lib binary resolution: single `cargo metadata` lookup of the new crate's target-dir (pattern already used — resolve with `CARGO_TARGET_DIR` unset), stored once in a var reused by all seven wiring sites.

**Tests:**
- `digest_oracle_committed`: `python3 -c "import json;d=json.load(open('openspec/changes/bench-consol-tick/pre-move-digests.json'));assert len(d)==16"`.
- `no_live_refs_to_old_paths`: `rg -n "scenarios/[a-z-]*/rust-camel-lib" benchmarks/ openspec/specs/` returns 0 hits (COVERAGE is updated in Task 1.6 to the new path).
- `build_all_produces_one_lib`: run `bash benchmarks/harness/builder/build-all.sh` → assert `benchmarks/contenders/rust-camel-lib/target/release/rust-camel-lib-fixture` exists and `find benchmarks/scenarios -path '*rust-camel-lib/target' -type d` returns nothing.

**Acceptance:**
- `git status` shows the seven deletions committed; no dangling references in live code.
- Harness dry-run (`--dry-run`, all scenarios) resolves every lib cell to the same binary path.

- [x] 1.2

## Node runtime dir consolidation

### Task 1.3: Move node contenders to a shared runtime dir

**Files:**
- `benchmarks/contenders/node/package.json` (new)
- `benchmarks/contenders/node/node-native/<scn>.mjs` ×7 (moved from `benchmarks/scenarios/<scn>/node-native/`)
- `benchmarks/contenders/node/node-fastify/<scn>.mjs` ×7 (moved)
- `benchmarks/scenarios/<scn>/node-native/` (deleted, 7×)
- `benchmarks/scenarios/<scn>/node-fastify/` (deleted, 7×)
- `benchmarks/harness/run.sh` (modified — node wiring + npm step)
- `benchmarks/harness/builder/build-all.sh` (modified — npm install once)
- `.gitignore` (modified — `benchmarks/contenders/node/node_modules/`; today's pattern only covers `benchmarks/scenarios/**/node_modules/`)

**Steps:**
1. `git mv` each scenario's node entry scripts into
   `benchmarks/contenders/node/node{,-fastify}/<scn>.mjs` (script bodies
   unchanged — they already read env for payloads/markers/latency;
   EXCEPTION: the 4 XML-bridge scripts' in-script default anchors to
   `shared/` assets re-point `../shared/…` →
   `../../../scenarios/<scn>/shared/…` — node cells get no per-cell env,
   in-script defaults ARE the runtime asset mechanism).
2. Create ONE `package.json` with the union of deps (`fastify`,
   `saxon-js`, `xmllint-wasm`, `xslt3` — versions copied verbatim from the
   existing per-fixture `package.json` files) and ONE `npm install`
   (pinned via `package-lock.json`; if per-fixture lockfiles disagree on
   a version, take the pins from the existing per-fixture
   `package.json`s: fastify 5.12.1, xmllint-wasm 5.3.0, saxon-js 2.7.0
   — `benchmarks/runner/pin.sh` records only docker/node-tarball pins,
   not npm deps).
3. Delete the old per-scenario node dirs (keep `README.md` files by
   moving their content into `benchmarks/contenders/node/README.md`,
   merged).
4. `build-all.sh`: single `npm ci --prefix benchmarks/contenders/node`.
5. `run.sh` node wiring: entry-script resolution per scenario points at
   the new paths; `NODE_BIN` resolution and env plumbing unchanged.

**Tests:**
- `single_node_modules`: after build, `test -d benchmarks/contenders/node/node_modules` and `find benchmarks/contenders/node -maxdepth 2 -name node_modules | wc -l` = 1 (some npm-published packages ship their own nested `node_modules` — ajv, light-my-request, thread-stream; a bare recursive find counts those. maxdepth 2 counts OUR installs only).
- `node_scripts_mark`: for each of 7 scenarios, `node benchmarks/contenders/node/node-native/<scn>.mjs` with the scenario's env → marker emitted exactly once.
- `fastify_dep_installed_once`: `grep -c fastify benchmarks/contenders/node/package.json` = 1 (single declaration).

**Acceptance:**
- `bash benchmarks/bench run --dry-run` resolves all 14 node cells from the new location.
- `node_modules` size ≈ 15 MB (fastify once), not 7×.

- [x] 1.3

## Guard re-keying

### Task 1.4: Re-key completeness guard + expected cells on the registered roster

**Files:**
- `benchmarks/harness/run.sh` (modified)
- `benchmarks/harness/run-all.sh` (modified — forward the debug env)

**Steps:**
1. In `assert_family_completeness` (~`run.sh:1469-1520`): replace the
   `[[ -d scenarios/<scn>/<member> ]]` presence checks with iteration
   over the registered `CELLS` / `CELL_SCENARIO` roster (populated by
   `add_cell`), grouped per family. Trigger condition: family has ≥1
   registered cell among selected active scenarios.
2. Expected-cell count (~`run.sh:3085-3116`): derive from STATIC
   config, NOT the registered roster (roster-keyed expected would be
   tautological and go dark if a family's wiring dies entirely —
   design.md §Guard re-keying): expected = `FAMILY_COMPLETENESS`
   families × selected active scenarios × `SCENARIO_ARTIFACT_SET`
   asymmetry (bridge reduction: 4 core + node×2 = 6; others 8) → 52
   for the full matrix. Assert REGISTERED count == EXPECTED count
   (mismatch = hard error listing the delta).
3. Move the guard invocation to AFTER cell wiring completes (today it
   runs pre-wiring at `run.sh:1558-1563` when the node roster is still
   empty).
4. Keep behavior: inactive-scenario fixture → warning only; partial
   family → hard error listing family + each missing scenario.
5. RED-proof hook: `BENCH_DEBUG_DROP_CELL=<member>/<scenario>` env
   (recognized ONLY after all wiring, before the guard) makes the guard
   see that family's cell as unregistered — a deterministic way to
   prove RED without editing fixture trees.
6. `run-all.sh`: the `docker run` `-e` list is fixed — add
   `-e BENCH_DEBUG_DROP_CELL` (forward-if-set semantics: only when the
   host exported it) so the hook reaches the container.

**Tests:**
- `guard_red_on_partial_family`: full wiring + `BENCH_DEBUG_DROP_CELL=node-native/t2-json` → run aborts (exit ≠ 0) with stderr naming `node` family and `t2-json` as missing.
- `guard_green_on_full_matrix`: full wiring, no debug env → guard passes; expected cells = 52.
- `inactive_scenario_still_warns`: with `multi-step` present on disk → warning mentions it; run continues.

**Acceptance:**
- The three tests above pass in-container (`bash benchmarks/bench run-all --metric=m1 --n=1 --dry-run` variants exit as specified).
- No `[[ -d .*scenarios/.*]]` evidence remains inside the guard function body.

- [x] 1.4

## Meta hygiene

### Task 1.5: meta.json `scenarios` lists ACTIVE scenarios only

**Files:**
- `benchmarks/harness/run.sh` (modified)

**Steps:**
1. After scenario registration completes in `run.sh` (post
   auto-discovery, post guard), rewrite
   `$BENCH_RESULTS_ROOT/meta.json`'s `scenarios` field to the sorted,
   comma-joined ACTIVE `SCENARIOS` roster (the registered set — excludes
   `spike-*` by discovery and `multi-step` by non-registration). Use a
   targeted `python3` or `awk` in-place edit that preserves the rest of
   the JSON.
2. `run-all.sh` keeps writing the discovery list at launch (pre-run
   provenance); `run.sh` corrects it once the registered set is known.
3. NOTE (r_glm review of 1.2): the inactive-scenario discovery skip
   already landed in task 1.2 (`run.sh:845-853`); the
   `inactive_scenario_still_warns` test asserts that notice text.

**Tests:**
- `meta_excludes_inactive`: run with `multi-step` present on disk and 7
  registered scenarios → `python3 -c "import json;print(json.load(open('$META'))['scenarios'])"`
  contains exactly the 7 active names, no `multi-step`.
- `meta_json_still_valid`: `python3 -m json.tool $META` exits 0.

**Acceptance:**
- After a dry-run + registration, meta `scenarios` == the active 7.

- [x] 1.5

## Coverage, zone audit, smoke-parity gate

### Task 1.6: COVERAGE requirements + Zone contract audit

**Files:**
- `benchmarks/scenarios/COVERAGE.md` (modified)
- `benchmarks/README.md` (modified — contenders zone mention)
- `benchmarks/scenarios/README.md` (modified — template step 1 contradicted step 5 post-1.2)
- `benchmarks/harness/CONTEXT.md` (modified — stale fixture binary path)

**Steps:**
1. COVERAGE gains a "Consolidated builds" section stating as
   requirements (not plans): lib crate location
   `benchmarks/contenders/rust-camel-lib/` (one build, 7 scenarios,
   argv dispatch); node runtime dir `benchmarks/contenders/node/`
   (shared node_modules); exemptions with reasons — `rust-camel-cli`
   (single workspace build + per-scenario YAML, the copied pattern),
   `camel-standalone-*` (classpath-isolation fairness,
   `benchmarks/harness/CONTEXT.md` §3), `camel-quarkus-*-native`
   (AOT bakes the route; per-scenario artifacts ARE the measurement).
2. Zone audit: `ls benchmarks/` == exactly `README.md`, `bench`,
   `contenders/`, `harness/`, `scenarios/`, `runner/`, `records/`,
   `attic/` (+ `.gitignore`, gitignored `out/`, `.cache`); record the
   listing in the task notes.
3. `benchmarks/README.md`: add `contenders/` to the zone inventory with
   a one-line description.
4. `benchmarks/scenarios/README.md` ~:30-31: template step 1 still
   instructs creating `scenarios/<name>/rust-camel-lib/` — rewrite to
   the consolidated crate (add module under
   `benchmarks/contenders/rust-camel-lib/src/scenarios/` + argv
   dispatch entry), consistent with step 5.
5. `benchmarks/contenders/node/node-fastify/*.mjs` headers (×7): they
   cite `../node-native/route.mjs` — pre-1.3 name; update to
   `../node-native/<scn>.mjs` (comment-only change).
6. `benchmarks/harness/CONTEXT.md` ~:106: stale fixture binary path
   `…/rust-camel-lib/target/release/startup-minimal` →
   `benchmarks/contenders/rust-camel-lib/target/release/rust-camel-lib-fixture`
   (keep the `env -u CARGO_TARGET_DIR` gotcha).

**Tests:**
- `level1_audit`: `ls benchmarks/ | LC_ALL=C sort | paste -sd,` equals
  the expected zone list (allowing `.cache`, `out`).
- `contenders_holds_builds_not_data`: `find benchmarks/contenders -name 'bench-payload*' -o -name '*.xsd' -o -name '*.xsl' -o -name '*golden*'` returns nothing (scenario data lives only under `benchmarks/scenarios/`).

**Acceptance:**
- COVERAGE section present with the three exemptions + two consolidated
  locations; audit listing committed in the task's commit message body
  or notes.

- [x] 1.6

### Task 1.7: Phase A exit gate — smoke parity of all 52 cells

**Files:**
- `benchmarks/harness/checks/extract-digests.py` (new, committed by Task 1.2)
- `benchmarks/harness/checks/parity-52.py` (new — asserts 52 sample dirs, non-empty, marker-once per cell; per-scenario cross-contender digest equality and oracle equality for the 16 digest-bearing entries)

**Steps:**
1. In-container full wiring smoke:
   `bash benchmarks/bench run-all --metric=m1 --n=3` (uses the
   digest-pinned image, worktree repo mirror; hardlink pre-warm from
   Task 1.1 keeps natives/caches cheap).
2. From the run output scratch, for EVERY one of the 52 cells: exactly
   one marker line; for every scenario, all its cells logged the SAME
   `BENCH_INPUT_SHA256` value (cross-contender parity, per scenario).
3. Compare each scenario's digests against the committed oracle
   `openspec/changes/bench-consol-tick/pre-move-digests.json` (16
   entries — the digest-bearing cells; captured by Task 1.2 with the
   OLD fixtures in place). The canonical digests are properties of the
   per-scenario shared payload files, which did not move — equality is
   expected for all 16; the 52-cell COUNT and marker asserts are
   independent of the digest oracle.

**Tests:**
- `parity_52_cells`: `python3 benchmarks/harness/checks/parity-52.py <run_dir> openspec/changes/bench-consol-tick/pre-move-digests.json` exits 0 (asserts 52 sample dirs, non-empty, per-scenario cross-contender digest equality, digest equality with the committed oracle).
- `gauges_and_order_survive`: the run's `meta.json` carries
  `protocol.order_seed` (integer) and the invocation used no
  gauge-disable flag (grep the run log for any `--no-gauges`-style
  flag → zero hits).
- `markers_once`: same script asserts marker count == 1 per cell from
  the scratch `.out` evidence.

**Acceptance:**
- All 52 cells produce ≥3 samples with markers; zero digest mismatches.
- `bash benchmarks/bench run-all --metric=m1 --n=1 --dry-run` exits 0
  with 52 expected cells (guard + expected-count from Task 1.4).

- [x] 1.7

## Phase B: Warm tick mode + completeness (behavioral)

## Baseline

### Task 2.1: Capture pre-tick M1 baseline (24 cells)

**Files:**
- `openspec/changes/bench-consol-tick/pre-tick-baseline.json` (new, committed)
- `openspec/changes/bench-consol-tick/pre-move-digests.json` (new, committed by Task 1.2)

**Steps:**
1. In-container targeted run:
   `bash benchmarks/bench run-all --metric=m1 --n=30 --scenarios=t2-json,split-aggregate,t2-realistic-eip`.
2. Compute per-cell MEDIAN time-to-marker from `samples.txt` (first
   column) and write
   a JSON object of the shape `{"cells": {<cell-name>: {"median_ms":
   <float>}}}` holding all 24 `<scenario>_<contender>` entries, to
   `pre-tick-baseline.json`; commit it with this task.

**Tests:**
- `baseline_has_24_cells`: `python3 -c "import json;d=json.load(open('openspec/changes/bench-consol-tick/pre-tick-baseline.json'));assert len(d['cells'])==24"`.

**Acceptance:**
- Baseline file committed; every median from n=30 samples.

- [x] 2.1

## Tick loops (reference implementation: xsd-validation-bridge fixtures)

### Task 2.2: Tick mode in the consolidated lib crate (3 scenario branches)

**Files:**
- `benchmarks/contenders/rust-camel-lib/src/scenarios/t2-json.rs` (modified)
- `benchmarks/contenders/rust-camel-lib/src/scenarios/split-aggregate.rs` (modified)
- `benchmarks/contenders/rust-camel-lib/src/scenarios/t2-realistic-eip.rs` (modified)

**Steps:**
1. Port the tick pattern from the xsd-validation-bridge branch
   (`main.rs` reference: latency file read at :47, emission
   `BENCH_LATENCY {id} {duration_ns}\n` at :100): change the t2-json /
   split-aggregate / t2-realistic-eip routes from one-shot timer
   (`repeatCount=1&delay=0`) to the repeating form the references use
   verbatim — `timer:bench?period=10&repeatCount=10000` — keep the SAME
   body-building/parity code per exchange, and add the post-`.to()`
   emission of `BENCH_LATENCY <id> <duration_ns>` to
   `$BENCH_LATENCY_FILE` per exchange.
2. Marker stays exactly once, BEFORE the first tick emission (marker on
   route-start path unchanged).
3. No dispatch/build work moves before the marker.

**Tests:**
- `tick_after_marker`: launch branch with `BENCH_LATENCY_FILE=/tmp/x.log` → marker line appears in stdout before the first line of /tmp/x.log (compare timestamps/line order via the harness scratch evidence).
- `protocol_b_parses`: `bench-loadgen parse-protocol-b --log=<file>` exits 0 with n>0 records after ≥2 s of runtime.

**Acceptance:**
- In-container m2 smoke for the 3 lib cells parses n>0.

- [x] 2.2

### Task 2.3: Tick mode for rust-camel-cli (3 scenarios)

**Files:**
- `crates/camel-cli/src/commands/bench_instrument.rs` (modified — route-bracket mode, gated on BENCH_LATENCY_MODE=route)
- `benchmarks/scenarios/t2-json/rust-camel-cli/routes/t2-json.yaml` (modified)
- `benchmarks/scenarios/split-aggregate/rust-camel-cli/routes/split-aggregate.yaml` (modified)
- `benchmarks/scenarios/t2-realistic-eip/rust-camel-cli/routes/t2-realistic-eip.yaml` (modified)
- `benchmarks/harness/run.sh` (modified — cli argv gains `env BENCH_LATENCY_FILE=…` for the 3 scenarios)

**Steps:**
1. CONDUCTOR RULING (test-design-gap, 2026-09-01): `bench_instrument`
   wraps ONLY top-level `BuilderStep::To` — none of the 3 T2 yamls has
   one (split-aggregate's `to:` is nested inside `split`), so the
   original premise is false and the module writes 0 records. Fix:
   EXTEND `bench_instrument` (crates/camel-cli/src/commands/
   bench_instrument.rs) with an explicit route-bracket mode — env
   `BENCH_LATENCY_MODE=route` wraps the WHOLE route body (start
   timestamp at route entry, `BENCH_LATENCY <id> <duration_ns>` after
   the last step) = the same full-pipeline window the JVM/quarkus
   latency-writer bean and the lib crate (2.2) measure. Default mode
   (no env) stays BIT-IDENTICAL for the xsd cli cell. The three
   scenarios register the BARE argv (`run.sh` ~:1735-1753) with no
   env — fix BOTH sides:
   a. Route YAMLs: change the timer URI to `timer:bench?period=10&repeatCount=10000`
      (verbatim syntax the existing references use — NOT an open-ended
      "drop repeatCount"), keeping all other steps unchanged.
   b. `benchmarks/harness/run.sh`: embed
      `env BENCH_LATENCY_FILE=/tmp/v3-protocol-b-<scenario>_rust-camel-cli.log`
      AND `env BENCH_LATENCY_MODE=route` into the three cli cell argv
      strings (same pattern as the node wiring at `run.sh:1615`) so
      protocol-B relaunches carry both. Markers in the yamls gate on
       `idempotent_consumer` (first tick reserves; later ticks skip) —
       the DSL analogue of 2.2's AtomicBool latch.
    AMENDMENT (review r_glm, 2026-09-01): Route-bracket = timer-sourced
    routes ONLY (`from: timer:…`) — consumer routes' work is already
    inside the main record's window (synchronous direct dispatch); one
    record per tick, cross-runtime parity with the lib crate and the
    JVM beans.

**Tests:**
- `cli_cells_tick`: in-container m2 run of the 3 cli cells → `parse-protocol-b` n>0 each.
- `marker_once_cli`: m1 smoke n=3 → marker count 1 per cell (tick does not duplicate markers).

**Acceptance:**
- 3/3 cli warm cells parse records; M1 smoke clean.

- [x] 2.3

### Task 2.4: Tick mode for camel-standalone JVM fixtures (3 scenarios × 2 pairings)

**Files:**
- `benchmarks/scenarios/t2-json/camel-standalone/camel-standalone-dsl/src/main/java/com/rustcamel/bench/App.java` (modified)
- `benchmarks/scenarios/t2-json/camel-standalone/camel-standalone-yaml/src/main/java/com/rustcamel/bench/AppYaml.java` (modified)
- `benchmarks/scenarios/split-aggregate/camel-standalone/camel-standalone-dsl/src/main/java/com/rustcamel/bench/App.java` (modified)
- `benchmarks/scenarios/split-aggregate/camel-standalone/camel-standalone-yaml/src/main/java/com/rustcamel/bench/AppYaml.java` (modified)
- `benchmarks/scenarios/t2-realistic-eip/camel-standalone/camel-standalone-dsl/src/main/java/com/rustcamel/bench/App.java` (modified)
- `benchmarks/scenarios/t2-realistic-eip/camel-standalone/camel-standalone-yaml/src/main/java/com/rustcamel/bench/AppYaml.java` (modified)
- `benchmarks/scenarios/t2-json/camel-standalone/camel-standalone-yaml/src/main/resources/routes.yaml` (modified)
- `benchmarks/scenarios/split-aggregate/camel-standalone/camel-standalone-yaml/src/main/resources/routes.yaml` (modified)
- `benchmarks/scenarios/t2-realistic-eip/camel-standalone/camel-standalone-yaml/src/main/resources/routes.yaml` (modified)

**Steps:**
1. DSL pairing (App.java, 3 scenarios): port the tick emission from
   the xsd-validation-bridge standalone fixture — repeating timer URI
   `timer:bench?period=10&repeatCount=10000` + post-`.to()` processor
   writing `BENCH_LATENCY <id> <duration_ns>` to the
   `BENCH_LATENCY_FILE` env path.
2. YAML pairing (AppYaml.java + routes.yaml, 3 scenarios): the timer
   URI lives in `src/main/resources/routes.yaml` (change to
   `period=10&repeatCount=10000`); NO yaml tick reference exists
   anywhere (bridges have no yaml artifacts), so the emission is
   written fresh: a latency-writer processor bean defined in
   `AppYaml.java` (a class implementing Camel's `Processor` that
   appends `BENCH_LATENCY <id> <duration_ns>` to the
   `BENCH_LATENCY_FILE` path — mirror the xsd dsl processor logic)
   and referenced from `routes.yaml` via `<bean ref=…>`/bean-name on
   the final route step.
3. Marker emission unchanged in code-path position, latched to
   the FIRST COMPLETED exchange (first `BENCH_LATENCY` record
   strictly after the marker) — the blessed 2.2 idiom; do NOT move
   it to startup.
4. Rebuild jars (build-all.sh maven section covers it).

**Tests:**
- `standalone_tick_6_modules`: in-container m2 run of the 6 cells → parse-protocol-b n>0 each.

**Acceptance:**
- 6/6 standalone warm cells parse records; jars rebuild clean.

- [x] 2.4

### Task 2.5: Tick mode for quarkus native fixtures (3 scenarios × 2 pairings)

**Files:**
- `benchmarks/scenarios/t2-json/camel-quarkus/camel-quarkus-dsl/src/main/java/com/rustcamel/bench/BenchRoute.java` (modified)
- `benchmarks/scenarios/t2-json/camel-quarkus/camel-quarkus-yaml/src/main/resources/camel/routes.yaml` (modified)
- `benchmarks/scenarios/t2-json/camel-quarkus/camel-quarkus-yaml/src/main/java/com/rustcamel/bench/BenchBeans.java` (modified)
- `benchmarks/scenarios/split-aggregate/camel-quarkus/camel-quarkus-dsl/src/main/java/com/rustcamel/bench/BenchRoute.java` (modified)
- `benchmarks/scenarios/split-aggregate/camel-quarkus/camel-quarkus-yaml/src/main/resources/camel/routes.yaml` (modified)
- `benchmarks/scenarios/split-aggregate/camel-quarkus/camel-quarkus-yaml/src/main/java/com/rustcamel/bench/BenchBeans.java` (modified)
- `benchmarks/scenarios/split-aggregate/camel-quarkus/camel-quarkus-yaml/src/main/java/com/rustcamel/bench/ListAppendStrategy.java` (untouched — reflection guard, listed so the rebuild keeps `@RegisterForReflection`)
- `benchmarks/scenarios/t2-realistic-eip/camel-quarkus/camel-quarkus-dsl/src/main/java/com/rustcamel/bench/BenchRoute.java` (modified)
- `benchmarks/scenarios/t2-realistic-eip/camel-quarkus/camel-quarkus-yaml/src/main/resources/camel/routes.yaml` (modified)
- `benchmarks/scenarios/t2-realistic-eip/camel-quarkus/camel-quarkus-yaml/src/main/java/com/rustcamel/bench/LatencyBean.java` (new — the latency-writer Processor bean this module lacks; mirrors Task 2.4's AppYaml bean)

**Steps:**
1. Same tick port as Task 2.4 for the quarkus pairings. NOTE the
   source-sharing layout (fingerprint contract,
   `benchmarks/harness/CONTEXT.md` §2): `camel-quarkus-dsl-native/`
   contains only `build.gradle.kts`; `camel-quarkus-yaml-native/`
   additionally carries `src/main/resources/application.properties` —
   both build their route sources from the JVM sibling modules. So the
   tick code lands in the SIBLING modules (dsl `BenchRoute.java`; yaml
   `routes.yaml` + bean) and both the JVM and native subprojects pick
   it up on rebuild. Per-scenario shape (verified on disk): t2-json and
   split-aggregate yaml modules already have `BenchBeans.java` — add
   the latency-writer bean method there; `t2-realistic-eip`'s
   `camel-quarkus-yaml` has NO java dir at all — create the new
   `LatencyBean.java` (Files entry above) and reference it from its
   `camel/routes.yaml` (its `timer:bench` URI becomes
   `period=10&repeatCount=10000` + bean ref on the final step).
2. Native rebuilds are fingerprint-gated (source change → rebuild,
   ~1-2 min each with the docker builder cache warm from Task 1.1
   pre-warm); ensure `@RegisterForReflection` stays intact on the
   split-aggregate strategy class (commit 139cb70e regression guard).

**Tests:**
- `quarkus_tick_6_modules`: in-container m2 run of the 6 native cells → parse-protocol-b n>0 each.
- `reflection_kept`: split-aggregate quarkus cells emit markers (the 139cb70e failure mode = missing marker/ClassNotFoundException in stderr).

**Acceptance:**
- 6/6 quarkus warm cells parse records; markers exactly once.

- [x] 2.5

### Task 2.6: Tick mode for node fixtures + remove the t2-realistic-eip m2 skip

**Files:**
- `benchmarks/contenders/node/node-native/t2-json.mjs` (modified)
- `benchmarks/contenders/node/node-native/split-aggregate.mjs` (modified)
- `benchmarks/contenders/node/node-native/t2-realistic-eip.mjs` (modified)
- `benchmarks/contenders/node/node-fastify/t2-json.mjs` (modified)
- `benchmarks/contenders/node/node-fastify/split-aggregate.mjs` (modified)
- `benchmarks/contenders/node/node-fastify/t2-realistic-eip.mjs` (modified)
- `benchmarks/harness/run.sh` (modified — m2 skip list)

**Steps:**
1. Port the tick loop from the xsd-validation-bridge node fixtures
   (`BENCH_XSD_TICK` pattern: reads `BENCH_LATENCY_FILE`, writes
   `BENCH_LATENCY <id> <duration_ns>` per iteration on a 10 ms cadence)
   into the six t2-json / split-aggregate / t2-realistic-eip scripts,
   for both node-native and node-fastify (fastify branch ticks without
   a listener in protocol-B, mirroring its boot contract).
2. Marker emission is latched to the FIRST COMPLETED exchange (first
   latency record strictly after the marker) — the blessed 2.2-2.5
   idiom.
3. Remove `t2-realistic-eip` from the Protocol B m2 skip list
   (`benchmarks/harness/run.sh` ~:2325-2330) and confirm the
   `SCENARIO_M2_PROTOCOL` map marks all three scenarios `B`.

**Amendment (conductor ruling, r_glm review fix):** the blessed
"verbatim URI" `timer:bench?period=10&repeatCount=10000` is WRONG for
JVM Camel (Apache Camel timer `delay` defaults to 1000 ms; rust-camel's
own timer defaults to 0). T2 JVM fixtures whose markers latch to the
first exchange inherit +1000 ms in M1. Amended contract:
- `timer:bench?period=10&repeatCount=10000&delay=0` on the 12 T2 JVM
  sites (standalone dsl/yaml ×3 scenarios, quarkus dsl/yaml ×3).
- rust lib/cli URIs unchanged (delay already 0 by engine default).
- Bridge fixtures unchanged (startup-based markers, delay-insensitive).
- Node T2 scripts fire the first tick IMMEDIATELY (t0=now, first
  iteration now, then fixed 10 ms cadence) — cross-runtime first-fire
  parity. Node t2-json/split-aggregate bodies are built ONCE at startup
  (seq frozen at the lib's CANONICAL_SELFTEST_TICK constant; digest
  byte-identical), so the measured window contains only exchange
  processing.

**Tests:**
- `node_tick_6_scripts`: in-container m2 run of the 6 node cells → parse-protocol-b n>0 each.
- `no_eip_skip`: `rg -n "t2-realistic-eip" benchmarks/harness/run.sh` shows no m2-skip context (only wiring/protocol-map contexts).

**Acceptance:**
- 6/6 node warm cells parse records; skip list gone.

- [x] 2.6

## Records completeness + fail-closed publish

### Task 2.7: Expected-roster completeness + fail-closed `bench publish`

**Files:**
- `benchmarks/harness/summarize.py` (modified)
- `benchmarks/harness/test_summarize.py` (modified — roster serialization coverage)
- `benchmarks/harness/test_publish.py` (modified — the four+one publish-gate tests)
- `benchmarks/records/SCHEMA.md` (modified — document `expected_cells` as a sorted identity list)

**Steps:**
1. Add a warm-applicability map to `summarize.py`:
   `WARM_APPLICABLE = {"http-server","t2-json","split-aggregate","t2-realistic-eip","xsd-validation-bridge","xslt-bridge"}`;
   `startup-minimal` cold-only (`n/a`).
2. Expected-roster derivation: for `meta["scenarios"]`, expected cells =
   for each scenario: 8 contenders except bridge scenarios (6: core 4
   + node 2) — mirroring the harness asymmetry. Persist the roster as
   IDENTITIES, not a count: `"expected_cells": ["<scenario>/<contender>",
   …]` (sorted list derived deterministically from `meta["scenarios"]`
   at build time; a bare count cannot name a missing cell, and if
   every cell of one scenario is absent the publisher must still
   reconstruct and name them).
3. Publish gate in `--publish`: compute missing = expected roster −
   observed cells (a wholly absent cell = missing) + warm-applicable
   cells lacking m2 data; if any missing → print every
   `scenario/contender/metric` line to stderr and exit 1. Complete →
   proceed (index update as today).
4. Publish-gate tests live in `benchmarks/harness/test_publish.py`
   (the suite that owns publish behavior — verify its existing
   conventions and follow them): complete-publishes-clean;
   missing-metric-rejects; wholly-missing-cell-rejects;
   wholly-absent-SCENARIO-rejects (all that scenario's cells named);
   warm-n/a-no-complaint. `test_summarize.py` keeps/adds coverage for
   roster serialization (`expected_cells` list in `run.json`).

**Tests:**
- `test_publish_rejects_missing_metric`: craft run dir with m1-only for a warm-applicable cell → `--publish` exit 1, stderr names the cell + `m2`.
- `test_publish_rejects_wholly_absent_cell`: remove one expected cell dir entirely → exit 1 naming that cell (not just observed-cells validation).
- `test_publish_rejects_wholly_absent_scenario`: remove EVERY cell dir of one scenario → exit 1 naming each of that scenario's expected cells (roster identities, not a count).
- `test_publish_clean_on_complete`: full roster with m1+m2 → exit 0, no completeness stderr.
- `test_startup_warm_na_not_gap`: startup-minimal cells m1-only → exit 0.

**Acceptance:**
- `python3 -m unittest test_summarize test_publish` (run from `benchmarks/harness/`) all green including the five publish-gate tests.
- `run.json` carries `expected_cells` as a sorted list of `<scenario>/<contender>` identities.

- [x] 2.7

### Task 2.7.1: Protocol-A m2 harvest (plan gap found in 2.7 review; bd rc-c5k3)

**Files:**
- `benchmarks/harness/summarize.py` (modified)
- `benchmarks/harness/test_summarize.py` (modified)

**Steps:**
1. In `_load_m2_round_cells`: first-level dirs that fail the two-level
   protocol-B walk are FLAT protocol-A cell dirs
   (`<scenario>_<contender>/protocol-a-summary.txt` — run.sh writes
   `cell_safe="${cell//\//_}"`). Split via the existing `_split_flat_dir`
   (longest-prefix), parse the `BENCH_MEASURE_A_RESULT` sentinel line's
   `round_p99s_ns=[…]`, feed the same `values` merge map protocol B
   uses (rounds merge identically). Loud warn + leave the gap on
   missing/unparseable sentinel.
2. Roster drift guard (review minor): a test that greps run.sh's
   `SCENARIO_ARTIFACT_SET` keys + registered per-set contenders and
   asserts equality with summarize's tuples — the bash↔python mirror
   must fail loudly on drift.
3. One fixture test shaped like the REAL dir (flat
   `http-server_<contender>` + text summary; assert 2 rounds merge).

**Tests:**
- `protocol_a_merges_like_b`: 2-round protocol-A fixture → same
  run.json m2 fields as the protocol-B equivalent.
- `roster_mirror_no_drift`: run.sh SCENARIO_ARTIFACT_SET == summarize
  tuples.

**Acceptance:**
- `python3 -m unittest test_summarize test_publish` all green; the
  full 52-cell canonical roster can go green on publish (http-server
  m2 harvested from the flat dirs).

- [x] 2.7.1

## Phase C: Follow-up completion (HUMAN DIRECTIVE 2026-09-01)

The merge gate was rejected with the ruling: the change's objective is
a COMPLETE benchmark system — discovered follow-ups must land inside
the change, not behind it. Sources: bd rc-sgmk (p2), rc-tpig (p3),
rc-ld1o (p3); rc-paeg triaged as dup of rc-dh7t (closed).

### Task 3.1: Freeze cli tick bodies (rc-sgmk) + re-run gates

**Files:**
- `benchmarks/scenarios/t2-json/rust-camel-cli/routes/t2-json.yaml` (modified)
- `benchmarks/scenarios/split-aggregate/rust-camel-cli/routes/split-aggregate.yaml` (modified)
- `benchmarks/scenarios/t2-realistic-eip/rust-camel-cli/routes/t2-realistic-eip.yaml` (modified)
- `benchmarks/runner/RUNBOOK.md` + `benchmarks/harness/CONTEXT.md` (modified — remove the bias disclosure once fixed)

**Steps:**
1. Hoist the per-tick body construction out of the measured window in
   the 3 cli yamls, mirroring the peers: body/array built ONCE at
   startup (rhai constant or equivalent yaml mechanism; seq frozen,
   digest unchanged), `set_body` reuses the constant per tick. The
   `BENCH_INPUT_SHA256` log moves to once-per-process (startup path),
   like lib/JVM. The `idempotent_consumer` marker gate stays.
2. Re-run the Phase B gates for the changed cells: warm gate (24/24,
   in-container m2 rounds=5 — the 3 cli cells must parse n>0) and the
   M1 tolerance gate (24/24 vs pre-tick-baseline.json).
3. Remove the cli-bias disclosure from RUNBOOK §4 / CONTEXT tick row
   (now false); note the fix; close rc-sgmk.

**Tests:**
- `cli_window_parity`: cli t2-json m2 p50 now within the same order of
  magnitude as its peers (lib 72µs / JVM ~726µs band — report where it
  lands; no body-build tax inflating it).
- Gates 2.8 green: warm 24/24, m1-tolerance exit 0.

- [x] 3.1

Task notes (3.1): mechanism — t2-json uses the cache-EIP first-tick
latch (`cache` step, memory repo, constant key, `coalesce_misses:
true`; on_miss = rhai build + input assert + digest log; per-tick hit
reconstructs the stored Body::Text) because the body is
size-parameterized (BENCH_PAYLOAD_BYTES sed axis) and the DSL has no
lifecycle hook; split-aggregate uses a yaml literal `set_body: value:`
(xsd precedent, fixed 591-byte array) + a second constant-key
`idempotent_consumer` once-gate for the digest log; t2-realistic-eip
needed no change (body was already a frozen literal, no digest log).
True-startup prefill via a second one-shot timer route was rejected:
no route-start ordering guarantee + bench_instrument route-bracket
wraps every timer-sourced route (spurious latency record) + a timer
delay would break M1 tolerance. Gates: warm 24/24 (cli n=802/438/802),
m1 24/24 (cli deltas -1.0/-1.0/+0.0 ms); t2-json cli m2 p50 4171µs →
2617µs; digests byte-identical (a0db69e1… / 123444b4…), 1 line per
launch across 33 launches per cell.

### Task 3.2: Adaptive m2 sampling window (rc-tpig)

**Files:**
- `benchmarks/harness/run.sh` (modified — m2 window/min computation)

**Steps:**
1. Replace the fixed window formula (warmup + samples-per-round ×10ms
   assuming 10ms ticks) with an adaptive per-cell window: sample until
   the cell collects `samples-per-round` records OR a generous cap
   (e.g. 6× the nominal window), whichever first. The minimum-samples
   health check then asserts against the NOMINAL expectation but only
   HARD-fails on n=0 (slow-but-alive cells report their real n with a
   note, not status=failed). Design for: cells whose genuine tick
   period exceeds 10ms (split-aggregate rust-camel-cli ~20-25ms).
2. Re-run warm gate: split-aggregate cli cell reaches samples-per-round
   (no insufficient-samples status) with the adaptive window.

**Tests:**
- `slow_cell_full_samples`: split-aggregate cli m2 collects ≥
  samples-per-round records under the adaptive window; status not
  failed.
- `dead_cell_still_fails`: a cell emitting 0 records still hard-fails
  (RED proof, e.g. BENCH_DEBUG-style or a crafted fixture).

**Acceptance:**
- Warm gate 24/24 with zero insufficient-samples statuses; rc-tpig
  closable.

- [x] 3.2

Task notes (3.2): mechanism — the nominal window loop is untouched
(fast cells measure exactly as before); after it, a still-alive cell
short of the nominal record count enters a 1s-poll extension that
recounts the tmpfs latency log until it reaches samples-per-round or
the cap (6× nominal, bounded by the existing 600s runaway guard — when
the nominal window already sits at 600s the extension is a no-op).
Health check: hard-fail ONLY on observed=0 (legacy status line shape
kept, `observed=0`; warm-24/summarize both treat it as a genuine
gap); 0<n<min keeps the data — parse + summaries emitted, plus a
`note=slow-tick …` line appended AFTER the parse-protocol-b output
(the parse rewrites m2-summary.txt; first-line status scanners and
JSON-first readers unaffected). The RSS window stays nominal (window-
boundary property). Dead-cell RED proof: new `BENCH_DEBUG_SILENCE_CELL`
env (run.sh ln -sf /dev/null on one cell's latency file; run-all.sh
forwards it like BENCH_DEBUG_DROP_CELL) — the cell still launches and
runs, only its records vanish; the first-success probe aborts at 30s
(return 1, warn-continue, no m2-summary, exit-codes.txt probe reason)
and warm-24 reads n=0 FAIL. The record-check observed=0 branch itself
is belt-and-braces (the probe intercepts n=0 first — documented in
run.sh). The note path fires only for a cell that dies mid-window with
partial data or hits the 6× cap (a live slow cell always reaches
nominal under the extension, so no note appears in healthy runs).
Tests: slow_cell_full_samples — split-aggregate only, warmup=3
samples=800 (nominal 11s): cli cell observed=686<800 at nominal end →
extension engaged once (no other cell) → observed=845 at 14s → final
n=847≥800, summary json n=847, no failed status. dead_cell_still_fails
— BENCH_DEBUG_SILENCE_CELL=t2-json/rust-camel-lib, samples=10 rounds=1:
probe hard-fail + no summary + warm-24 n=0 for that cell. Warm gate
regression — full 3-scenario gate (warmup=3 samples=300 rounds=5,
out/20260902T075334Z): warm-24 24/24 PASS exit 0, ZERO
insufficient-samples files in the run dir; split-agg cli n=422-437
across 5 rounds. SCHEMA m2_attempted_cells row + RUNBOOK §4 + warm-24
docstring updated to the new semantics.

### Task 3.3: Standalone jar freshness gate (rc-ld1o)

**Files:**
- `benchmarks/harness/builder/build-all.sh` (modified — jar freshness)

**Steps:**
1. Before the maven build of each standalone module, rebuild if ANY
   source/resource file under `src/` is newer than the packaged jar
   (`find src -newer target/*.jar` shape); skip only when fresh. Kills
   the stale-jar trap that bit task 2.6's fix (stale 1605ms markers).

**Tests:**
- `stale_jar_rebuilds`: touch an App.java → build-all → jar rebuilt
  (mtime newer than the touch).
- `fresh_jar_skips`: untouched → build skips (fast no-op).

**Acceptance:**
- Both tests pass; rc-ld1o closable.

- [x] 3.3

### Papal review findings (e_opus, 2026-09-02 — APPROVE-WITH-FINDINGS, none blocking)

- A (bd rc-2k33): roster triple-authored run.sh/summarize/warm-24 —
  drift tests guard it; single-source derivation is the follow-up.
- B (fixed): `&delay=0` added to all 5 cli timer URIs (engine default
  is 0 — papal traced camel-timer lib.rs:56 — uniform intent kills the
  default-change footgun; t2-json smoke re-verified marker+digest).
- C (fixed): summary.md gains a standing rust-camel-cli reader caveat
  (interpreted-YAML + script-engine per-tick cost — authoring-layer
  tax, not the compiled engine) whenever a cli cell is in the record.

- [x] papal findings

## Phase B exit gate

### Task 2.8: Quantitative exit gate — warm cells + M1 unperturbed

**Files:**
- `benchmarks/harness/checks/warm-24.py` (new — asserts 24/24 tick-scenario cells with n>0 parsed protocol-B records)
- `benchmarks/harness/CONTEXT.md` (modified — glossary "Marker"/§2 "fixtures carry zero self-timing" now stale after tick mode: add the tick-mode exception row — warm fixtures append `BENCH_LATENCY` per tick; cold-start remains zero-self-timing)
- `benchmarks/harness/checks/m1-tolerance.py` (new — reads pre-tick-baseline.json + a post run dir; exits 0 iff every cell's |post-pre| <= max(0.15*pre, 3.0) ms; prints the offending cell on failure)
- `benchmarks/runner/RUNBOOK.md` (modified — tick note)

**Steps:**
1. Warm gate: in-container
   `bash benchmarks/bench run-all --metric=m2 --scenarios=t2-json,split-aggregate,t2-realistic-eip`
   → all 24 cells parse n>0 protocol-B records.
2. M1 gate: rerun m1 n=30 for the same 3 scenarios; compare per-cell
   medians against `pre-tick-baseline.json` with the blessed tolerance:
   pass iff `abs(post-pre) <= max(0.15*pre, 3.0)` ms for EVERY cell;
   any violation blocks the phase (report the cell).
3. RUNBOOK §4 gains one paragraph: tick fixtures loop after the marker;
   warm data for the three tick scenarios comes from that loop.
4. Record both gate outputs (pass/fail + numbers) in the task notes.

**Tests:**
- `m1_gate_script`: `python3 benchmarks/harness/checks/m1-tolerance.py openspec/changes/bench-consol-tick/pre-tick-baseline.json <post_run_dir>` exits 0 iff all 24 cells within tolerance.
- `warm_gate_script`: `python3 benchmarks/harness/checks/warm-24.py <run_dir>` exits 0 iff 24/24 cells have n>0 parsed records.

**Acceptance:**
- Both gates pass; numbers recorded; RUNBOOK updated.

- [x] 2.8
