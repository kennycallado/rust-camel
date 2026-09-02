# RUNBOOK — canonical v1 baseline run (era 2)

Human procedure for the first canonical era-2 record (run sequence
v5). The agent's deliverable ends at preparation; the run itself is
human-invoked — the hours-long quiet-host predicate cannot be
guaranteed by an agent (spec scenario "Human-invoked execution").

Everything below runs from the repository root.

## 1. Verify the quiet-host gates

Canonical criteria: `benchmarks/harness/CONTEXT.md`, section
"Quiet-host criteria (canonical run)". A run that violates a gate is
VOID — discard the results, quiet the host, re-run. Do not publish or
cite numbers from a void run.

**Load gate** — 1-minute load average below 3.0 for 10 consecutive
minutes before start:

```console
$ cat /proc/loadavg
# first field is the 1-minute average; re-check over ~10 minutes
```

**Baseline-stability gate** — devnull-baseline warmup within ±5%
across its last 3 probes:

```console
$ env -u CARGO_TARGET_DIR cargo build --release -p bench-loadgen
$ BENCH_DEVNULL_PORT=8080 target/release/bench-devnull &
$ target/release/bench-loadgen measure-throughput \
    --url=http://127.0.0.1:8080/bench --duration-secs=10 \
    --warmup-secs=2 --out=/tmp/probe.json
$ jq .mean_msgs_per_sec /tmp/probe.json
# repeat the probe 3×; the spread must stay within ±5%; kill the
# devnull server afterwards
```

**No-concurrent-builds gate** — no concurrent cargo/gradle/maven
processes:

```console
$ pgrep -c 'cargo|gradle|mvn'
0
```

## 2. Build and pin the runner image

```console
$ bash benchmarks/runner/pin.sh
$ cat benchmarks/runner/DIGEST
sha256:<64hex>
```

`pin.sh` builds `benchmarks/runner/Dockerfile` and records the image
digest in `benchmarks/runner/DIGEST`. Tags are convenience labels;
canonical runs consume the digest.

## 3. Docker socket (native-image builds)

The runner image ships NO in-image native-image (see the
`benchmarks/runner/Dockerfile` header). Native Quarkus cells build
through `BENCH_NATIVE_MODE=docker`, which delegates to
`QUARKUS_NATIVE_BUILDER_IMAGE` (the Mandrel builder). That delegation
requires the HOST docker socket inside the runner container:
`run-all.sh` mounts `/var/run/docker.sock` automatically in this mode
(the default). Do not strip the mount — without it every Quarkus
native cell fails.

## 4. The run — one command

```console
$ bash benchmarks/bench run-all
```

- **Coverage**: EVERY active scenario × every contender
  (auto-discovery of `benchmarks/scenarios/` minus `spike-*` and
  unregistered dirs like `multi-step`). No subsets, no env vars —
  `--scenarios=` stays a harness-level developer knob only.
- **Wall-clock**: ~4-6 h for the full matrix. Keep the host quiet for
  the whole window.
- **Artifacts** under `benchmarks/harness/out/<ts>/`:
  - `meta.json` — launch snapshot written by `run-all.sh`: resolved
    digest, `git_commit`, quiet-host load snapshot (one/five/fifteen),
    protocol (rounds 5, order seed), and `run_id` = the launch
    timestamp (no sequence numbering — one command, one complete run,
    one record).
  - `<inner-timestamp>/` — the raw run dir: per-cell `samples.txt`,
    `m2-summary.json`, `provenance.json`, and
    `measurement_order.json` when M3/M4 arms run.
- **samples.txt rss column**: wrapper-launched cells
  (`rust-camel-cli` in `http-server` and the bridge scenarios) write
  the literal `null` — GNU `time -v` measured the bash
  `*-cli-wrapper.sh`, not the contender, so RSS is invalid by
  construction there. Elapsed ms stays valid.

Tick scenarios (`t2-json`, `split-aggregate`, `t2-realistic-eip`):
their fixtures do not exit after the marker — the route keeps looping
on a 10ms timer (`period=10`, first fire immediate, `delay=0` on JVM
Camel) and appends one `BENCH_LATENCY <id> <ns>` record to
`$BENCH_LATENCY_FILE` per tick. Warm M2 data for these three scenarios
comes from that loop (protocol B: the harness launches the contender
per round for the nominal window `warmup-time + samples-per-round ×
10ms` — assuming 10ms ticks — and parses the records); M1 cold-start
is unchanged — the clock still stops at the marker.

Adaptive window (rc-tpig, fixed): cells whose genuine tick period
exceeds 10ms (split-aggregate rust-camel-cli ticks at ~20-25ms) can
never collect `samples-per-round` records inside the nominal window.
When such a cell is still short of the nominal count at nominal-window
end, the harness extends its collection window (1s poll on the latency
log) until the count reaches `samples-per-round` or the cap (6× the
nominal window, bounded by the 600s runaway guard). Fast cells never
enter the extension. The health check hard-fails only on n=0 (dead
cell, `status=failed reason=insufficient-samples observed=0`); a cell
still short of the nominal count at the cap keeps its real n — the
summary carries the data plus a `note=slow-tick …` line appended to
`m2-summary.txt`.

Cli tick bodies are frozen (rc-sgmk, fixed): the rust-camel-cli
routes build their canonical bodies once per process (a yaml literal
constant, or the cache-EIP first-tick latch where the body is
size-parameterized) and log `BENCH_INPUT_SHA256` once, so the cli
tick window contains only exchange processing — same as the other
contenders.

## 5. Post-run: summarize, publish, check

```console
$ OUT=benchmarks/harness/out/<ts>
$ RUN_DIR=$(find "$OUT" -mindepth 1 -maxdepth 1 -type d | head -1)

$ bash benchmarks/bench summarize \
    --run-dir "$RUN_DIR" --meta "$OUT/meta.json" \
    --out-dir "$OUT-record"
$ bash benchmarks/bench publish \
    --run-dir "$OUT-record" --records-dir benchmarks/records
$ python3 benchmarks/harness/summarize.py --check benchmarks/records
```

- Before `summarize`, replace `meta.json`'s
  `protocol.duration_secs` (the launch estimate 10800) with the
  actual wall-clock seconds of the run.
- `summarize` builds `run.json` + `summary.md` into `--out-dir`;
  `run_id` comes straight from meta (the launch timestamp).
- `publish` copies the record into `records/` and rebuilds
  `records/index.json`.
- `--check` is the mechanical post-run guard (index/dir identity,
  digest pinning, summary regeneration).
- If you overrode harness flags (for example `--rounds`), edit
  `meta.json`'s `protocol` before `summarize`.

## 6. Gauges stay ON

Memory gauges are enabled in every measured cell of the canonical
run — per ADR-0066
(`docs/adr/0066-metrics-collector-binding-and-lifetime.md`): the A/B
lever study measured ratio 0.9890, 95% CI [0.9785, 1.0126], UNPAIRED
— cost unresolved from zero, ≤~1% at this resolution (full protocol
and interpretation:
`docs/benchmarks/history/2026-08-29-benchmark-v4-addendum.md`). Do not
disable gauges for the v1 record.

## 7. COMPANION: payload axis (optional, not a gate)

Optional second invocation. Its absence does NOT void the v1 record:

The payload axis (two reference contenders: rust-camel-lib and
camel-quarkus native, × 4 payload classes: 1 / 32 / 256 / 1024 KiB)
is a COMPANION measurement, not a gate of the v1 record — and its
one-command sweep is NOT YET WIRED into `run-all.sh` (a `BENCH_PAYLOAD_AXIS`
sweep + contender pinning is follow-up work tracked with bd rc-f4po).
Until wired, a per-class run can be done manually by setting
`BENCH_PAYLOAD_BYTES` per invocation with `--scenarios=t2-json`.
Absence of the axis does NOT void the v1 record.

After this change merges to main, create the tag on the pre-merge
main tip — the merge commit's first parent, the last commit where
era-1 reports lived at docs/benchmarks/ unmodified:
`git tag bench/era-1-final <merge-sha>^1`. Keep the tag local;
pushing it is the human's action with the branch push.

## 8. Post-run validation (agent-side, human-gated)

This checklist discharges the spec scenario "v1 record lands". It runs
AFTER the human executes the run and publishes the record (sections
4-5). It is meaningful only against a REAL published v1 record — it is
not a pre-run gate. The task-3.4 checkbox covers the checklist
deliverable only; the record gate itself is human-gated and tracked
as bd rc-f4po. No earlier task discharges the postcondition.

- [ ] (a) Guard green:
      `python3 benchmarks/harness/summarize.py --check benchmarks/records`
      exits 0 (index/dir identity, digest pinning, summary
      regeneration).
- [ ] (b) `records/index.json` gained its first era-2 entry: `era`
      `"2"`, `run_id` in `<YYYYMMDD>-v5` form (date-first, sequence 5).
- [ ] (c) The published `run.json` conforms to `records/SCHEMA.md`:
      `schema_version` 1; every top-level key present;
      `container_digest` is the `sha256:<64hex>` form recorded in
      `runner/DIGEST`; `protocol.order_seed` present; every cell
      carries `input_sha256` (a digest, or a documented `null` for
      scenarios without a canonical payload contract).
- [ ] (d) Gauges ON evidence: metric fields in the cells include the
      gauge readings (memory gauges enabled per ADR-0066 — section 6).
- [ ] (e) Summary tables contain no number absent from `run.json` —
      the `--check` guard proves this mechanically (it regenerates
      every `summary.md` from its sibling `run.json` and byte-diffs).

The task checkbox is ticked only when (a)-(e) all pass against the
real published v1 record.
