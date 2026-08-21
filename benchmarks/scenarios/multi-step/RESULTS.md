# RESULTS — multi-step convoy-signature run (c16 / c300 / c1000)

Task 3.1 of OpenSpec change `loadgen-connections-multistep-bench`.
Workload: route `bench-multi` (stream-cache, two rhai scripts, choice with
rhai predicate and header step). Fixture: `rust-camel-cli/multi-step-cli-wrapper.sh`
with the release `camel` binary. Load generator: `bench-loadgen` from this
worktree, subcommand `measure-throughput`.

## Run profiles

All runs: `--url=http://127.0.0.1:8081/bench-multi --duration-secs=30
--warmup-secs=5 --workers=4`. Only `--connections` differs. Date: 2026-08-21.
Effective `ulimit -n` for the c1000 run: **524288** (host default; the check
demanded more than 1100, so no raise was needed).

| run  | connections | mean msgs/s | p50 msgs/s | min msgs/s | cv    | total 2xx | error_rate_pct |
|------|-------------|-------------|------------|------------|-------|-----------|----------------|
| c16  | 16          | 6425.1      | 6453       | 4311       | 0.096 | 192754    | 0.0            |
| c300 | 300         | 6229.5      | 6253       | 5877       | 0.036 | 186886    | 0.0            |
| c1000| 1000        | 6291.5      | 6323       | 5838       | 0.029 | 188744    | 0.0            |

`total_errors` and `total_non_2xx` are 0 in all three runs.

## Throughput ratios

- c300 / c16 = 6229.5 / 6425.1 = **0.970**
- c1000 / c16 = 6291.5 / 6425.1 = **0.979**

## Convoy gate

- `error_rate_pct` < 1.0 in all three artifacts. Observed: 0.0 everywhere.
- c1000 per-second buckets hold no zero-value seconds. Observed: 30 buckets,
  min 5838, max 6543.
- `"workers": 4` present in all three, with `"connections"` 16, 300, 1000.

## Interpretation

Post-rc-vdy2 expectation: flat or sub-linear degradation. The data meets it.
Throughput stays within 3 percent of the c16 baseline at 300 and at 1000
connections. The c1000 run is slightly faster than c300, so the difference
between those two runs is noise, not a trend. No convoy signature appears:
throughput does not collapse as connection count grows from 16 to 1000.
The coefficient of variation drops as connections rise (0.096 to 0.029),
which means per-second throughput gets steadier, not spikier. This is the
opposite of a convoy pattern (bursty seconds followed by stalls).

## Recompute from the committed artifacts

Run these commands from the worktree root. They re-derive every number above.

```sh
# Gate: error_rate_pct < 1.0, workers/connections fields match.
python3 - <<'EOF'
import json
for name, conns in [("c16",16),("c300",300),("c1000",1000)]:
    d = json.load(open(f"benchmarks/scenarios/multi-step/artifacts/{name}.json"))
    assert d["workers"] == 4 and d["connections"] == conns, (name, d["workers"], d["connections"])
    assert d["error_rate_pct"] < 1.0, (name, d["error_rate_pct"])
    print(f"{name}: workers={d['workers']} connections={d['connections']} error_rate_pct={d['error_rate_pct']} OK")
EOF

# Gate: c1000 buckets contain no zero-value seconds.
python3 -c 'import json; b=json.load(open("benchmarks/scenarios/multi-step/artifacts/c1000.json"))["per_second_buckets"]; assert len(b)>0 and min(b)>0, b; print("c1000 buckets:", len(b), "min:", min(b), "max:", max(b), "- no zero-value seconds OK")'

# Profiles: mean, p50, min, cv per run.
python3 -c 'import json
for n in ("c16","c300","c1000"):
    d=json.load(open(f"benchmarks/scenarios/multi-step/artifacts/{n}.json"))
    print(n, "mean=%.1f p50=%d min=%d cv=%.3f errors=%d non2xx=%d" % (d["mean_msgs_per_sec"],d["p50_msgs_per_sec"],d["min_msgs_per_sec"],d["cv"],d["total_errors"],d["total_non_2xx"]))'

# Ratios: c300/c16 and c1000/c16.
python3 -c 'import json
m=lambda n: json.load(open(f"benchmarks/scenarios/multi-step/artifacts/{n}.json"))["mean_msgs_per_sec"]
print("c300/c16  = %.3f" % (m("c300")/m("c16")))
print("c1000/c16 = %.3f" % (m("c1000")/m("c16")))'
```

## Operator note: flag syntax

`bench-loadgen` accepts only the `--key=value` form. `parse_flags`
(`benchmarks/harness/loadgen/src/cli.rs`) reads the text between `--` and
`=`. A space-separated `--workers 4` parses to the empty string, the parse
of the empty string fails, and the code falls back to defaults: `url=""`,
`workers` = CPU count, duration 60 s, warmup 10 s. The run then drives
error requests in a tight loop and never dials the target. Use the
`--key=value` form for every flag, as the commands in this file do.
