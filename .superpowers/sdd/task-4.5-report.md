# Task 4.5 Report — Harness integration for T4a/T4b scenarios (bd rc-alhy)

**Status**: DONE
**Worker**: `@workers/w_heavy`
**Branch**: `feature/rc-f3g9-startup-benchmark`
**Base**: `c8fc89f7` (Task 4 review fixes)
**Plan reference**: bd `rc-alhy` (P1 bug — Task 5 BLOCKED analysis)

## Summary

Closed all 5 harness gaps from bd rc-alhy: T4a (xslt-bridge) + T4b
(xsd-validation-bridge) scenarios now resolve end-to-end through the
v3 harness with their asymmetric 4-artifact matrix, and the single
won't-measure cell (xslt-bridge/camel-quarkus-dsl-native) is registered
with a documented skip reason. Task 5 (measurement run) is unblocked.

**Test count unchanged** (104 tests, 100% pass — no Rust code touched).

## Commits (in order, base → HEAD)

| SHA | Subject |
| --- | --- |
| `09b9821b` | `fix(bench/v3): register T4a/T4b scenarios in harness` (Gaps 1, 2, 3) |
| `3c051110` | `fix(bench/v3): scenario-aware artifact resolution + T4a skip` (Gaps 4, 5) |
| `50bcd56c` | `fix(bench/v3): use env wrapper for rust-camel-lib argv` (env-prefix fix) |

## Per-gap resolution

### Gap 1: SCENARIO_MARKER (lines 83-92, before → after)

**Before** (`run.sh:83-87`):
```bash
declare -A SCENARIO_MARKER=(
    ["startup-minimal"]="BENCH_ROUTE_READY"
    ["t2-realistic-eip"]="BENCH_ROUTE_READY body=pong-bench"
    ["http-server"]="BENCH_ROUTE_READY"
)
```

**After**: added `["xslt-bridge"]="BENCH_ROUTE_READY"` and
`["xsd-validation-bridge"]="BENCH_ROUTE_READY"` with a comment block
documenting that the marker is emitted by rust-camel-lib main() /
rust-camel-cli wrapper / Java EventNotifierSupport on RouteStarted.

### Gap 2: SCENARIO_M2_PROTOCOL (lines 971-987)

**Before**: T4a/T4b were commented-out placeholders with WRONG names:
```bash
# ["t4a-xml-transform"]="B"
# ["t4b-xsd-validate"]="B"
```

**After**: real Protocol B entries registered:
```bash
["xslt-bridge"]="B"
["xsd-validation-bridge"]="B"
```

The placeholder names (`t4a-xml-transform` / `t4b-xsd-validate`) never
matched the actual scenario directory names. Both scenarios are
timer-driven (Protocol B service-time, NOT Protocol A request-response).

### Gap 3: Bridge activation case (lines 1536-1547)

**Before**: `case "$m2_scenario" in t4a-*|t4b-*)` — never matched the
actual scenario dir names (`xslt-bridge`, `xsd-validation-bridge`).

**After**:
```bash
case "$m2_scenario" in
    xslt-bridge|xsd-validation-bridge)
        METRIC_BRIDGE_TRACKING=true
        METRIC_RSS_SAMPLE=true
        ;;
esac
```

Direct scenario-name match (no glob — the scenario var holds the dir
name verbatim). The bridge activation now correctly fires for T4a/T4b
M2 cells; for all other scenarios the defaults (`METRIC_BRIDGE_TRACKING=false`,
`METRIC_RSS_SAMPLE=false`) are unchanged.

### Gap 4: Scenario-aware artifact resolution (lines 639-807 + 833-843)

**Before**: `resolve_all_cells` hardcoded the 8-artifact T1/T2/T3 layout
unconditionally. T4a/T4b only have 4 artifacts (camel-standalone-dsl,
camel-quarkus-dsl-native, rust-camel-lib, rust-camel-cli), so the
harness exited at `resolve_standalone_jar` for the missing
`camel-standalone-yaml/`.

**After**: 3 changes —

1. **New `SCENARIO_ARTIFACT_SET` map** (lines 95-108): per-scenario
   flag distinguishing "full" (8 artifacts, T1/T2/T3) vs "bridge"
   (4 artifacts, T4a/T4b). Default (unset entry) = "full".

2. **New `resolve_bridge_scenario_cells` function** (lines 639-790):
   resolves the 4 bridge-scenario cells with per-cell argv that embeds:
   - Bridge binary path (`$REPO_ROOT/bridges/xml/build/native/xml-bridge`)
   - Per-scenario shared assets (payload + stylesheet for T4a, payload
     + schema for T4b)
   - Per-cell latency + PID file paths matching the M2 protocol-B
     reader's cell-safe naming (`/tmp/v3-protocol-b-<scenario>_<contender>.log`,
     `/tmp/v3-bridge-pid-<scenario>_<contender>.txt`)
   - Wrapper script invocation for rust-camel-cli (per-scenario
     `<scenario>-cli-wrapper.sh` with `--camel-bin`, `--config`,
     `--routes`, `--bridge-binary`, `--bridge-wrapper`,
     `--bridge-pid-file`, `--latency-file` flags)
   - Java system properties for camel-standalone-dsl + quarkus-native
     (`-Dbench.payload`, `-Dbench.stylesheet` / `-Dbench.schema`,
     `-Dbench.latency_file`)
   - Env-var-prefix argv for rust-camel-lib (uses `env` wrapper —
     see commit `50bcd56c`)

3. **Dispatch in `resolve_all_cells`** (lines 833-843): case statement
   on `${SCENARIO_ARTIFACT_SET[$scenario]:-full}` — "bridge" → call
   `resolve_bridge_scenario_cells` and `continue`; "full" or `*` →
   fall through to the existing 8-artifact path (T1/T2/T3 unchanged).

4. **Cell count assertion update** (lines 1424-1435): replaced the
   hardcoded `${#SCENARIOS[@]} * 8` with a scenario-aware loop that
   sums per-scenario counts (full=8, bridge=4).

### Gap 5: Won't-measure skip for xslt-bridge/camel-quarkus-dsl-native

**Before**: brief claimed "harness auto-skips" but it didn't. T4a
camel-quarkus-dsl-native is won't-measure per Task 4 (Xalan native
compile failure for XSLT 3.0).

**After**: 4 changes —

1. **New `CELL_WONT_MEASURE` map** (line 118): cell → human-readable
   skip reason. Absent entry = measurable.

2. **`add_cell` extended** (lines 766-783): optional 5th arg registers
   the cell as won't-measure with a documented reason.

3. **xslt-bridge/camel-quarkus-dsl-native registered as won't-measure**
   (lines 707-722): the cell IS registered (appears in dry-run output
   + cell count) but is skipped by measurement loops. T4b's
   camel-quarkus-dsl-native is measurable (Xerces-J native works for
   XSD validation). The xslt-bridge quarkus native BUILD is also
   skipped (no point compiling something we'll never measure).

4. **Skip logic in M1/M2 loops + summary section**: each loop now
   checks `CELL_WONT_MEASURE[$cell]` first; on hit it logs
   `SKIP: $cell — <reason>` and `continue`s. The summary section
   prints `SKIP — <reason>` instead of "no data".

### Plan-level finding acknowledgment

Per the Task 5 worker's note: T1/T2/T3 have 8 artifacts each (24
total); T4a/T4b have 4 artifacts each (8 total). The asymmetric matrix
is intentional and documented in the new `SCENARIO_ARTIFACT_SET` map
declaration. T4a/T4b were NOT expanded to 8 artifacts — bridge-tax
measurement only needs the 4 mandated artifacts (the YAML variants
add no bridge-tax signal since the bridge is the same gRPC subprocess
regardless of route authoring).

## Env-prefix fix (commit 50bcd56c)

The initial Gap 4 implementation used bash's prefix-assignment syntax
(`BENCH_PAYLOAD=path /bin/binary`) for the rust-camel-lib argv. The
M1 smoke surfaced a real bug: GNU time re-parses `"${argv[@]}"` into
its own argv list, so `VAR=value` tokens become positional arguments
to `time` itself (not bash prefix-assignments — those only work BEFORE
the command word). Symptom: `error: could not resolve contender PID
for xsd-validation-bridge/rust-camel-lib (time_pid=...)` because
`time` was trying to exec `BENCH_PAYLOAD=path` as the command name.

**Fix**: wrap with `env` — `env BENCH_PAYLOAD=path ... /bin/binary`.
GNU time execs `env`, which sets the vars and execs the binary.
Portable across time/cmd wrappers.

## Verification output

### 1. Dry-run resolves all 32 cells (31 measurable + 1 SKIP)

Command (from brief verify step #1):
```bash
JAVA_HOME=/tmp/rc-f3g9-jdk21 \
GRADLE_BIN=/home/kenny/.gradle/wrapper/dists/gradle-8.10-bin/deqhafrv1ntovfmgh0nh3npr9/gradle-8.10/bin/gradle \
bash benchmarks/harness/run.sh --metric=m1+m2 \
  --scenarios=startup-minimal,t2-realistic-eip,http-server,xslt-bridge,xsd-validation-bridge \
  --dry-run
```

Cell-count arithmetic note: the brief said "~64 cell entries (63
measurable + 1 SKIP)" but that count double-counted M1 + M2 metric
pairs. The actual cell count is **32** (each cell appears once in
dry-run, annotated with its M2 protocol): T1+T2+T3 = 3×8 = 24, T4a+T4b
= 2×4 = 8, total 32 cells, of which 31 are measurable + 1 SKIP
(xslt-bridge/camel-quarkus-dsl-native). The brief's "~64" reflects
"cell × metric" pairs (32 cells × 2 metrics for `--metric=m1+m2`),
but the dry-run output lists cells, not cell-metric pairs.

Key output excerpt:
```
resolved cells: 32 (expected: 32)
=== dry-run: 32-cell shuffled order ===
  ...
  17. xsd-validation-bridge/rust-camel-lib (M1 + M2 protocol B)
  18. xslt-bridge/rust-camel-lib (M1 + M2 protocol B)
  ...
  27. xslt-bridge/camel-quarkus-dsl-native (SKIP: won't-measure: Xalan native compile failure for XSLT 3.0 (Task 4 / spec §4.8))
  ...
=== dry-run complete (no contender invoked) ===
```

All xslt-bridge + xsd-validation-bridge cells annotated with
"protocol B" (Protocol B service-time). xslt-bridge/camel-quarkus-
dsl-native shows the SKIP marker with the documented reason.

### 2. T4a camel-quarkus-dsl-native shows SKIP

Visible in dry-run output above (entry #27) AND in M1 smoke output
(below). Both the measurement loops and the summary section emit the
skip reason verbatim.

### 3. Bridge activation fires for xslt-bridge / xsd-validation-bridge

The case statement at line 1536 now matches the actual scenario names
directly (`xslt-bridge|xsd-validation-bridge`). When the M2 loop
processes a T4a/T4b cell, `METRIC_BRIDGE_TRACKING=true` and
`METRIC_RSS_SAMPLE=true` are set before invoking
`m2_measure_protocol_b`. The M2 protocol B function (line 1054) reads
the bridge PID file at round boundaries and spawns the RSS sampler
against the contender PID tree. (M2 measurement itself wasn't smoke-
tested — it takes ~5-10 min per cell; the case-statement fix was
verified by code inspection + the dry-run's "protocol B" annotation.)

### 4. T1/T2/T3 unchanged (backward compat)

Dry-run with `--scenarios=startup-minimal,t2-realistic-eip,http-server`
resolves exactly 24 cells (3 × 8). The dispatch falls through to the
original 8-artifact resolution path; no T1/T2/T3 cell-name, argv,
marker, or protocol changes.

### 5. Quick M1 smoke on T4a + T4b (reduced params)

Command (from brief verify step #5):
```bash
JAVA_HOME=/tmp/rc-f3g9-jdk21 GRADLE_BIN=... \
bash benchmarks/harness/run.sh --metric=m1 \
  --scenarios=xslt-bridge,xsd-validation-bridge --n=2 --warmup=1
```

Output (3.1s total runtime — fingerprint-cached quarkus native build
skipped):
```
resolved cells: 8 (expected: 8)
--- M1 warmup (1 runs per cell, discarded) ---
SKIP: xslt-bridge/camel-quarkus-dsl-native (M1 warmup) — won't-measure: Xalan native compile failure for XSLT 3.0 (Task 4 / spec §4.8)
--- M1 measured runs (2 per cell) ---
SKIP: xslt-bridge/camel-quarkus-dsl-native (M1 round 0) — won't-measure: Xalan native compile failure for XSLT 3.0 (Task 4 / spec §4.8)
SKIP: xslt-bridge/camel-quarkus-dsl-native (M1 round 1) — won't-measure: Xalan native compile failure for XSLT 3.0 (Task 4 / spec §4.8)
=== scenario: xsd-validation-bridge ===
=== Pair A ===
  camel-standalone-dsl time: median=371.5 p95=373 n=2
  camel-standalone-dsl rss:  median=137494.0 p95=140352 n=2
  camel-quarkus-dsl-native time: median=20.5 p95=23 n=2
  camel-quarkus-dsl-native rss:  median=50656.0 p95=50656 n=2
  rust-camel-lib time: median=8.5 p95=9 n=2
  rust-camel-lib rss:  median=8708.0 p95=8708 n=2
=== Pair B ===
  rust-camel-cli time: median=29.5 p95=30 n=2
  rust-camel-cli rss:  median=3048.0 p95=3048 n=2
=== scenario: xslt-bridge ===
=== Pair A ===
  camel-standalone-dsl time: median=407.5 p95=410 n=2
  camel-standalone-dsl rss:  median=140550.0 p95=140592 n=2
  camel-quarkus-dsl-native: SKIP — won't-measure: Xalan native compile failure for XSLT 3.0 (Task 4 / spec §4.8)
  rust-camel-lib time: median=10.0 p95=11 n=2
  rust-camel-lib rss:  median=7944.0 p95=7944 n=2
=== Pair B ===
  rust-camel-cli time: median=29.0 p95=30 n=2
  rust-camel-cli rss:  median=3048.0 p95=3048 n=2
=== done ===
```

All 7 measurable cells produced raw samples. The xslt-bridge/camel-
quarkus-dsl-native SKIP appears in 3 places (warmup, each of 2
measured rounds) + the summary. The "no data" entries for Pair B
YAML contenders (camel-standalone-yaml, camel-quarkus-yaml, camel-
quarkus-yaml-native) are expected — those artifacts don't exist for
bridge scenarios; the summary's Pair A/B contender iteration is
shared across all scenarios and shows "no data" for absent artifacts.

**Sample bridge-tax numbers (M1, n=2 only — not the production run)**:
- rust-camel-lib T4a: 10ms cold-start / 7944 KiB (bridge spawns lazily AFTER marker; M1 measures time-to-marker which is pre-tick)
- rust-camel-lib T4b: 8.5ms / 8708 KiB
- rust-camel-cli T4a: 29ms / 3048 KiB
- camel-quarkus-dsl-native T4b (no bridge): 20.5ms / 50656 KiB

The bridge tax itself is the per-tick cost (M2 territory); M1 just
confirms the marker-emit path works for all 7 measurable cells.

### 6. Native build verification

The M1 smoke rebuilt the xsd-validation-bridge camel-quarkus-dsl-
native binary from scratch (60s native-image compile, 64MB runner
produced) on the first run because the fingerprint was new. Subsequent
runs skip the build via fingerprint cache. This validates the existing
`build_native_artifact` path works for bridge scenarios unchanged.

## Quality gates

All 5 quality gates from the brief pass (no Rust code touched — the
changes are bash-only to `benchmarks/harness/run.sh`):

| Gate | Result |
|---|---|
| `cargo fmt --check --all` | ✓ pass (EXIT 0) |
| `cargo clippy --workspace --all-features --exclude camel-cli --exclude camel-component-kafka --exclude security-keycloak --exclude security-wasm-policy -- -D warnings` | ✓ pass (EXIT 0, 2m 03s) |
| `cargo xtask lint-unwrap` | ✓ OK (no violations) |
| `cargo xtask lint-secrets` | ✓ OK (no violations) |
| `cargo xtask lint-log-levels` | ✓ OK (strict mode — 0 violations) |

## Concerns / Task 5 notes

1. **Dry-run cell count discrepancy**: the brief expected "~64 cell
   entries" but the actual cell count is 32. The brief's 64 number
   counted cell × metric pairs (32 cells × 2 metrics for
   `--metric=m1+m2`); the dry-run output lists cells once each. This
   is the correct behavior — measurement loops iterate cells once per
   metric, not once total.

2. **Summary "no data" entries for bridge scenarios**: the summary
   section iterates Pair A + Pair B contender lists (8 total)
   unconditionally per scenario. For bridge scenarios, only 4 of
   those contenders exist; the missing 4 show "no data" rather than
   being absent. This is the existing summary behavior (unchanged);
   a follow-up could make the summary scenario-aware (iterate only
   the contenders that exist). Out of scope for this bd.

3. **M2 measurement not smoke-tested**: M2 takes ~5-10 min per cell
   with default params; the brief's verify step #5 is M1-only. The
   M2 path was verified by code inspection: bridge activation case
   matches the actual scenario names, the M2 protocol B function
   constructs latency/PID file paths from the cell-safe name
   (`/tmp/v3-protocol-b-${cell//\//_}.log`) which matches the paths
   embedded in the cell argv, and the RSS sampler is invoked when
   `METRIC_RSS_SAMPLE=true`. Task 5 will exercise the full M2 path.

4. **http-server (T3) Protocol A URL not registered**: per Task 2
   concern #3, T3's Protocol A URL wiring is incomplete. The harness
   prints "protocol ?" for http-server cells in dry-run because T3
   isn't in `SCENARIO_M2_PROTOCOL` (commented out). This is a Task 3
   follow-up, not part of bd rc-alhy. T4a/T4b are not affected.

5. **`.bench-fingerprint` untracked**: the M1 smoke created
   `benchmarks/scenarios/xsd-validation-bridge/camel-quarkus/camel-quarkus-dsl-native/.bench-fingerprint`
   (fingerprint cache for the native build). This follows the
   existing T1/T2 pattern (those scenarios also have untracked
   `.bench-fingerprint` files). Not gitignored, not committed —
   transient build state. A future chore could add `.bench-fingerprint`
   to `.gitignore`.

6. **Bridge subprocess cleanup between M1 cells**: measure_once kills
   the contender after the marker but does NOT kill the bridge
   subprocess (which spawns lazily on the first tick, AFTER the
   marker). For M1 this is harmless — the bridge never spawns during
   the M1 measurement window. For M2, m2_measure_protocol_b kills
   the contender but the bridge may leak across cells. Task 5 should
   verify whether bridge accumulation affects M2 numbers; if so, a
   per-cell bridge cleanup is a follow-up.

## What is NOT in scope for Task 4.5

Per the bd rc-alhy fix scope, this task touched only the harness
integration layer. The following are explicitly out of scope:

- T3 Protocol A URL registration (Task 3 follow-up).
- Summary-section scenario-aware contender iteration (cosmetic).
- Per-cell bridge subprocess cleanup (Task 5 measurement concern).
- M2 measurement smoke (5-10 min/cell — Task 5 territory).
- COVERAGE.md cell-state updates (Task 5 report-time concern).

## Return contract

- **Status**: DONE
- **Commits**: `09b9821b`, `3c051110`, `50bcd56c`
- **Per-gap resolution**: all 5 gaps closed (see above)
- **Dry-run cell count**: 32 (31 measurable + 1 SKIP)
- **M1 smoke**: PASS (3.1s, 7/7 measurable cells produced raw samples)
- **Quality gates**: 5/5 pass
- **Concerns**: 6 items above (#1 count discrepancy is the most
  load-bearing — calls out a brief arithmetic error, doesn't affect
  correctness)

---

## Continuation — ea71de05 (C4 + C6 partial)

After the original Task 4.5 closed (commits `09b9821b` → `50bcd56c`),
continuation work landed commit `ea71de05`:

- **C4 (T3 Protocol A URL registration)**: registered
  `http-server` cells under `PROTOCOL_A_CELL_URL` + `PROTOCOL_A_CELLS`
  so calibration + measure-a cover the 8 T3 artifacts. Pre-ea71de05
  the harness printed "protocol ?" for T3 in dry-run and skipped M2
  measurement entirely.
- **C6 (bridge subprocess cleanup, partial)**: added
  `_kill_process_tree_recursive` (recursive `/proc/<pid>/task/<tid>/children`
  walk, kills descendants before parent, cycle-safe via visited set) +
  `cleanup_bridge_subprocess` (kills bridge PID read from
  `/tmp/v3-bridge-pid-<cell>.txt`, idempotent). Wired into
  `kill_contender`, `measure_once`, and `m2_measure_protocol_b`.
- **C3-partial (wrapper-script detection)**: extended
  `resolve_all_cells` to recognize the rust-camel-cli wrapper script
  path and treat it as the launch argv (rather than the bare `camel`
  binary, which can't emit `BENCH_ROUTE_READY` from its own process).

## rust-camel-cli Protocol A wrapper bug — root cause + verification

**Symptom**: `http-server/rust-camel-cli` cell failed M2 measurement
phase with "contender exited before marker" (calibration for same cell
succeeded). Wrapper's stderr showed:

```
Consumer error: Endpoint creation failed: Failed to bind 0.0.0.0:8080: Address already in use (os error 98)
Failed to start CamelContext: ... EADDRINUSE
error: child exited before printing 'CamelContext started' on stdout
```

**Root cause**: the rust-camel-cli wrapper uses `setsid camel run &`
to spawn the camel child in a new session. Pre-ea71de05
`kill_contender` sent SIGKILL only to the wrapper PID, which left
the setsid child alive (reparented to init, still holding port 8080).
The next Protocol A cell's launch hit EADDRINUSE on bind.

**Fix**: `_kill_process_tree_recursive(wrapper_pid)` walks
`/proc/<wrapper>/task/<tid>/children`. The kernel keeps the original
parent-children link even after `setsid` reparenting (until the
parent calls `wait()`), so the camel child IS reachable from the
wrapper's children entry. The recursive walk kills camel + tail
forwarders + wrapper in reverse order (descendants first).

**Direct verification (2026-07-20)**:

```
$ bash http-server-cli-wrapper.sh --camel-bin ... --config ... --routes ... &
wrapper PID=56740
$ cat /proc/56740/task/56740/children
56744                              # ← camel PID
$ ss -lntp | grep :8080
LISTEN ... users:(("camel",pid=56744,fd=9))
$ source <(sed -n '/^_kill_process_tree_recursive/,/^}/p' benchmarks/harness/run.sh)
$ _kill_process_tree_recursive 56740
$ ss -lntp | grep :8080          # ← empty, port released
$ ps aux | grep 'camel run'      # ← empty, no orphan
```

**Timeline of run evidence**:

| Run               | Timestamp           | State     | Notes                                |
| ----------------- | ------------------- | --------- | ------------------------------------ |
| `/tmp/tmp.pcsZACK7Ib` | 2026-07-19 23:25 | ❌ FAILED | Pre-fix working tree                 |
| `/tmp/tmp.S3szNPxJ4u` | 2026-07-19 23:41 | ✅ PASS   | Post-fix working tree (C6 applied)   |
| `/tmp/tmp.Vbx7VuQihk` | 2026-07-20 00:03 | ✅ PASS   | 1112 HTTP request logs in measurement file |

**Conclusion**: the bug as originally filed against Task 4.5 is
**resolved by `ea71de05`**. The "7/8 cells pass" worker report was
based on the pcsZACK7Ib run (pre-fix) and is stale; current state
is 8/8 cells measurable pending a fresh M2 smoke confirmation.
