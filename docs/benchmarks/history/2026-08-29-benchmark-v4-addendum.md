> Era-1 report. Frozen at git tag bench/era-1-final. Live data: benchmarks/records/.
# Benchmark v4 addendum — metric-family lever study (M3, rust-camel-cli)

> **RESULT STATUS: SUPERSEDED.** The first attempt of this study ran
> under heavy concurrent external load and its numbers are INVALID.
> A clean re-run per the disclosed protocol (quiet host, load below 3,
> no concurrent builds, stable devnull baselines) replaced them. Read
> only the "Clean re-run" section below. The contamination history
> stays under "Contaminated first attempt" as a record of what ran.
> The original INVALID banner said: re-run on a quiet machine only
> (load average below ~2–3, no concurrent builds, devnull baseline
> validation stable before and after both arms); the container runner
> still owns canonical reruns.

Date: 2026-08-30. Change: `openspec/changes/bench-missing-cells`, task 3.1.
Design: D5 ("Metric-family overhead A/B") in that change's `design.md`.
This addendum extends `docs/benchmarks/2026-07-22-benchmark-v4.md`. It does
not change any v4 result. The lever study is **not a contender row**; it
answers one question: what does per-request metric-family emission cost the
`rust-camel-cli` T3 http-server cell?

## Method

One contender, two arms, same machine, same day.

- **Cell**: `http-server/rust-camel-cli` (Pair B, `camel run` + YAML route,
  T3 http-server).
- **Arm A (`Camel.toml.metrics-on`)**: Prometheus exporter on
  `127.0.0.1:18191` plus every gateable metric family on
  (`[default.observability.metrics]` with `enabled = true`, `exchange`,
  `duration`, and `components` all true). This is maximal per-request
  emission per the ADR-0066 levers
  (`docs/adr/0066-metrics-collector-binding-and-lifetime.md`).
- **Arm B (`Camel.toml.metrics-off`)**: same Prometheus exporter on the same
  port, master metrics gate off (`enabled = false` only). With the master
  gate off, the exchange, duration, and component families do not flow. The
  `camel_errors_total` family has no lever and fires in both arms
  (ADR-0066 D5); it is part of the constant background, not the lever.
- **Backend held constant**: both arms register the Prometheus exporter.
  The A/B therefore isolates family-emission cost with the backend present
  in both arms. It does NOT measure collector-versus-NoOp cost; that
  question is out of scope here (noted in D5).
- **Arms differ in one file only.** Both arm files are copies of the
  fixture `Camel.toml` plus the observability blocks and one shared
  prerequisite: the ADR-0061 public-exposure acknowledgment
  (`[default.binds."0.0.0.0:8080"]`) without which the T3 route refuses to
  start. The acknowledgment is identical in both arms; it is a fixture
  prerequisite, not a lever.
- **Harness wiring**: `http-server-cli-wrapper.sh` now honors
  `BENCH_CAMEL_TOML`. When set, the wrapper passes that file as the child's
  `--config` argument. When unset, the default fixture path applies, so
  existing invocations are unchanged. A one-line info echo of the spawned
  `camel run` argv supports the `wrapper_env_passthrough` check.
- **Workload**: M3 sustained throughput, 5 rounds of 30 s per arm with a
  10 s warmup per round, payload and load generation per the v4 report
  ("M3 — Sustained throughput", "CPU topology and affinity (v4 pin)",
  "Seeded randomized cell order"). The harness's loopback baseline check
  ran per the v4 report ("Baseline validation (devnull)").
- **Ratio**: the two arms are separate runs, so same-run pairing cannot
  hold. `bench-loadgen aggregate-ratios --independent` resamples each
  arm's round means separately and prints an `UNPAIRED` tagged line
  (design D5 calls unpaired cross-run pairing the only honest form here;
  default seed 0, 2000 resamples, deterministic for identical inputs).

## Result

### Clean re-run (quiet host, 2026-08-30)

Re-run per the protocol disclosed in the first attempt's INVALID banner.
Same host, same day, same command form. The loadgen drove the same
default CPU split in both arms (server cores 0-2 plus SMT siblings
6-8, loadgen cores 3-5 plus 9-11); no re-split occurred.

Host state and harness baseline validation:

- 1-minute load average before arm A: 2.14. Before arm B: 0.46
  (threshold: below 3). No concurrent builds. After both arms the
  load settled at 1.08.
- devnull baseline, pre-matrix → post-matrix: arm A 104,384.2 →
  103,963.8 req/s (−0.4%); arm B 104,502.5 → 99,224.8 req/s (−5.0%).
  No degradation warning, no re-split, no FATAL in either arm.
- All six cells reused the artifacts built during the first attempt
  (fingerprint cache hits); no rebuild ran inside either arm.

| Arm | Run | Median M3 throughput | Round means spread |
|---|---|---|---|
| A (metrics-on) | `20260830T102854Z` | 62,518.2 msg/s | 61,856.5–64,011.0 (±2.4%) |
| B (metrics-off) | `20260830T105055Z` | 63,215.7 msg/s | 62,232.9–63,617.8 (±2.2%) |

Command and output, verbatim:

```
$ bench-loadgen aggregate-ratios \
    benchmarks/results/20260830T102854Z/http-server_rust-camel-cli/m3-summary.json \
    benchmarks/results/20260830T105055Z/http-server_rust-camel-cli/m3-summary.json --independent
RATIO http-server_rust-camel-cli/http-server_rust-camel-cli point=0.9890 lo=0.9785 hi=1.0126 UNPAIRED
```

All three sanity gates from the re-run protocol pass:

- Round means sit within ±2.4% of their arm medians (gate: ±15%).
- The point ratio does not exceed 1.05, so the "metrics-on is faster"
  implausibility signal is absent.
- Both devnull baselines held (values above).

Read. The metrics-on arm measured 0.989x the metrics-off arm. The
interval (0.9785 to 1.0126) includes 1.0. The lever cost is therefore
not resolved at this resolution: per-request metric-family emission
costs at most about 1% for this cell under the M3 workload, and the
study cannot separate that from zero. The five-round, unpaired design
sets the interval width; the first attempt's spread (±15% swings) is
gone on a quiet host.

### Contaminated first attempt (history)

The first attempt ran while a user session executed parallel `rustc`
builds (load average ~14). Tell-tale signs recorded at run time:
arm-A repeats spread 15,364–72,601 msg/s, the retained ratio read
1.1630 (families "on" appearing faster than "off", which is
physically implausible), and the harness's devnull baseline degraded
mid-matrix, which triggered its documented re-split. The retained
pair was arm A
58,099.2 msg/s (run `20260830T083652Z`, first matrix pass) against
arm B 49,954.3 msg/s (run `20260830T073335Z`), RATIO
point=1.1630 lo=1.0913 hi=2.2830 UNPAIRED. Keep it as a record of
what ran; do not cite it. The protocol worked as written: the re-run
above satisfied every gate it demanded.

## Interpretation bounds

- **Isolates**: family-emission cost at the request path, with the
  Prometheus backend registered in both arms and all other fixture,
  route, and hardware conditions held as equal as separate runs allow.
- **Does not isolate**: collector-versus-NoOp cost. Both arms ran the
  collector. That study would arm one side with no backend at all; D5
  marks it out of scope.
- **Not paired**: the UNPAIRED tag marks a cross-run ratio. A same-run
  paired ratio (the v4 contender table) needs both cells in one run;
  arms cannot share a run because they differ in configuration, not in
  contender identity.
- **Topology note**: both clean-re-run arms used the same default CPU
  split, and neither arm tripped the harness's loadgen-headroom
  warning or re-split. The first attempt's re-split passes cannot pair
  with anything; they are part of the contamination history only.
- **Run-state variance on this host**: quiet-host round means spread
  ±2.4%, and the lever interval is correspondingly tight. Under
  external build load the same cell swung 15,364–72,601 msg/s. Host
  state, not the lever, dominated the first attempt's numbers. CI and
  container reruns still own canonical numbers.

## Raw artifacts

`benchmarks/results/` is gitignored and not committed. The runs behind
this addendum lived at these local paths on the authoring host:

- Clean re-run arm A: `benchmarks/results/20260830T102854Z/`
  (`http-server_rust-camel-cli/m3-summary.json`).
- Clean re-run arm B: `benchmarks/results/20260830T105055Z/`
  (`http-server_rust-camel-cli/m3-summary.json`).
- First attempt (contaminated, history only): arm A
  `benchmarks/results/20260830T083652Z/`, arm B
  `benchmarks/results/20260830T073335Z/`.
- Discarded (topology mismatch or replicate only):
  `20260830T065337Z`, `20260830T075338Z`, and the build/shakeout run
  `20260830T101246Z` (1-second rounds, not a measurement).

CI and container runs own canonical reruns of this study. The arm files
and the wrapper passthrough are committed, so any host can reproduce it:

```
BENCH_CAMEL_TOML=<scenario>/Camel.toml.metrics-on  M3_DURATION_SECS=30 \
  bash benchmarks/harness/run.sh --scenarios=http-server --metric=m3 --rounds=5
BENCH_CAMEL_TOML=<scenario>/Camel.toml.metrics-off M3_DURATION_SECS=30 \
  bash benchmarks/harness/run.sh --scenarios=http-server --metric=m3 --rounds=5
```

then pair the two `m3-summary.json` files with `aggregate-ratios
--independent` as above.

## Published v4 key ratios with bootstrap confidence intervals

Added 2026-08-30 (task 3.3 of `openspec/changes/bench-missing-cells`). This
section is separate from the lever study above; the result banner at
the top covers
only the lever-study arms. The ratios below restate the v4 report
§"Key ratios" with confidence intervals. Both cells of every pair come from
the published v4 run `20260723T161422Z`, so the paired (same-run) mode of
`aggregate-ratios` applies. The point estimates match the published v4
values to two decimals. No v4 number changes.

Command form, one invocation per pair:

```
$ cargo run -p bench-loadgen --bin bench-loadgen -- aggregate-ratios \
    benchmarks/results-published/20260723T161422Z/<cellA>/m3-summary.json \
    benchmarks/results-published/20260723T161422Z/<cellB>/m3-summary.json
```

| Comparison | point | 95% CI (lo–hi) |
|---|---|---|
| rust-camel-lib / camel-standalone-dsl (JVM) | 1.1769 | 1.1665–1.1932 |
| rust-camel-lib / camel-quarkus-dsl-native | 2.0849 | 2.0382–2.1186 |
| camel-standalone-dsl / camel-quarkus-dsl-native (Pair A JVM→native) | 1.7715 | 1.7452–1.7756 |
| camel-standalone-yaml / camel-quarkus-yaml-native (Pair B JVM→native) | 1.7820 | 1.7499–1.7877 |
| rust-camel-lib / rust-camel-cli (YAML parse cost) | 1.0355 | 1.0178–1.0406 |

Method. `aggregate-ratios` runs a paired round-block bootstrap. Each
resample draws the five round indices with replacement, applies the same
indices to both cells, and records the ratio of the medians of the round
means. The interval is the percentile interval over 2000 resamples with
seed 0 (SplitMix64), so identical inputs give identical output. The joint
resampling is what pairs the interval: cells measured in one run share the
randomized-block measurement order, so round conditions (thermal drift,
host state) move both cells together.

Resolution note. Each cell contributes five round means, so the bootstrap
resamples five paired blocks. The CI width reflects that 5-round
resolution. It does not capture run-to-run variance across independent
runs; cross-run claims keep the limitations section of the v4 report.
