# Tasks: loadgen-connections-multistep-bench

## loadgen

### Task 1.1: --connections knob decoupled from workers

**Files:**
- `benchmarks/harness/loadgen/src/cli.rs` (modified)
- `benchmarks/harness/loadgen/src/cli_runtime.rs` (modified)
- `benchmarks/harness/loadgen/src/main.rs` (modified — usage/help text only)
- `benchmarks/harness/loadgen/tests/cli_integration.rs` (modified)

**Steps:**
1. In `cli.rs::measure_throughput_main`: parse `--connections N` (`usize`) after the
   existing `workers` parse; default = the resolved `workers` value (flag absent ⇒
   bit-for-bit current behavior). Pass to `run_measure_throughput` as a new parameter
   inserted before `output_path`.
2. In `cli_runtime.rs::run_measure_throughput` (line ~455): change signature to
   `(url, duration_secs, warmup_secs, workers, connections, output_path)`; keep
   `worker_threads(workers.max(2))` unchanged.
3. In `run_throughput_async`: set `pool_max_idle_per_host(connections)` on the shared
   client; change the loop-spawn count from `workers` to `connections`
   (`Vec::with_capacity(connections)`); worker task bodies unchanged (sequential
   POST + drain-before-count stays).
4. Add `"workers": workers` and `"connections": connections` as top-level fields in
   the result JSON artifact written at `cli_runtime.rs:~580` (`serde_json::json!`).
   Flat fields, greppable. (Note: `throughput.rs:189` is a #[test]-only JSON literal
   mirroring cli_runtime's shape — it is NOT a published artifact and gains no fields.)
5. Update the `measure-throughput` help text in `main.rs::usage()` to document
   `--connections`.

**Tests:** (executable spec — name, setup, action, assert, command, expected)
- `connections_flag_recorded_in_output`: setup — the test binds its own TCP listener on
  `127.0.0.1:0` serving one canned HTTP response per connection (thread-per-connection,
  no sleep); action — `run_loadgen(["measure-throughput", "--url", listener_url (the
  test-bound 127.0.0.1:0 address as a String), "--duration-secs", "1", "--warmup-secs", "0", "--workers", "3",
  "--connections", "7", "--out", tmp])`; assert — output JSON contains `"workers": 3`
  and `"connections": 7` as top-level fields; command —
  `cargo test -p bench-loadgen --test cli_integration connections_flag_recorded_in_output`;
  expected — fails before Task 1.1 (unknown flag / missing field), passes after.
- `connections_default_matches_workers`: action — same invocation WITHOUT
  `--connections`, `--workers 3`; assert — JSON has `"connections": 3` equal to
  workers; command —
  `cargo test -p bench-loadgen --test cli_integration connections_default_matches_workers`;
  expected — fails before (field absent), passes after.
- `connections_drive_inflight_concurrency`: setup — test spawns a std `TcpListener`
  HTTP server with ONE THREAD PER ACCEPTED CONNECTION; each handler reads the request,
  increments an `AtomicUsize` of currently-open requests, updates a max watermark,
  sleeps 50 ms, writes the canned response, decrements; action — run measure-throughput
  with `--workers 2 --connections 8 --duration-secs 2` against it; assert — observed
  max concurrency is at least 7 (8 requested minus 1 slack for connection setup/
  teardown timing — proves the knob drives ~N tasks, not merely more than workers);
  command —
  `cargo test -p bench-loadgen --test cli_integration connections_drive_inflight_concurrency`;
  expected — fails before Task 1.1 (max ≈ 3: task-count and pool capped at workers),
  passes after.

**Acceptance:**
- All three new tests pass; full `cargo test -p bench-loadgen` green.
- `cargo fmt --check` and `cargo clippy -p bench-loadgen -- -D warnings` exit 0.
- `rg -n '"workers"' benchmarks/harness/loadgen/src/cli_runtime.rs` shows the result-JSON site carries the new fields.

- [x] 1.1

## scenario

### Task 2.1: multi-step benchmark fixture

**Files:**
- `benchmarks/scenarios/multi-step/rust-camel-cli/routes/multi-step.yaml` (new)
- `benchmarks/scenarios/multi-step/rust-camel-cli/Camel.toml` (new)
- `benchmarks/scenarios/multi-step/rust-camel-cli/multi-step-cli-wrapper.sh` (new)

**Steps:**
1. Write `routes/multi-step.yaml`: route id `bench-multi`,
   `from: "http://0.0.0.0:8081/bench-multi"`, steps in this order:
   - AMENDMENT (implementation-discovered, task 2.1): `stream_cache: true` then
     `convert_body_to: text` — the http consumer hands the pipeline a
     `Body::Stream`; rhai's `body` reads `as_text()` = `""` without them, so the
     preflight contract is unachievable (first run produced `-M1-M2`).
   - `script` (rhai): `body = body.to_upper() + "-M1"; properties["stage"] = "one";`
     (string-map CPU work + property seed)
   - second `script` (rhai): branch-seeded mutation —
     `if (properties["stage"] == "one") { properties["stage"] = "two"; body = body + "-M2"; } else { properties["stage"] = "branch-fail"; }`
   - `choice`: when-predicate (rhai) `property("stage") == "two"` → branch steps:
       `set_header` key `X-Bench-Stage`, `language: "rhai"`, value expression
       `property("stage")` (⇒ `"two"`). Header INSIDE the happy branch by design: a
       skipped/short-circuited choice leaves the header absent, so preflight proves
       branch execution, not just body shape.
     otherwise → `set_body` literal `BRANCH-FAIL` (observably wrong; nothing masks a
     skipped step)
   No terminal `set_body` on the happy path — the response body IS the accumulated
   script output.
   Deterministic preflight contract: request body `ping` ⇒ response body exactly
   `PING-M1-M2` AND response header `X-Bench-Stage: two`; any skipped/mis-ordered step
   yields a different body, `BRANCH-FAIL`, or header `branch-fail`.
2. Header comment block in the YAML documents: purpose (convoy-detection workload),
   step list with the derivation-order rationale, preflight contract, port 8081
   rationale (side-by-side with http-server), and the no-per-exchange-logging rule
   for the load phase.
3. Write `Camel.toml` mirroring `benchmarks/scenarios/http-server/rust-camel-cli/Camel.toml`
   (same shape, route file reference updated).
4. Write `multi-step-cli-wrapper.sh` mirroring `http-server-cli-wrapper.sh`'s child
   detection (wait for `CamelContext started` on the child's stdout — this is the
   CHILD-ready signal, not the public marker). Then, before emitting anything public,
   run the PREFLIGHT verbatim:
   ```sh
   RESP="$(curl -sS -i -X POST --data ping http://127.0.0.1:8081/bench-multi)"
   echo "$RESP" | grep -q '^HTTP/.* 200' || fail
   echo "$RESP" | grep -q 'X-Bench-Stage: two' || fail
   [ "$(echo "$RESP" | tail -1)" = "PING-M1-M2" ] || fail
   ```
   where `fail()` kills the child process and exits nonzero. ONLY after preflight
   passes, print `BENCH_ROUTE_READY $(date +%s%3N)` (unix epoch milliseconds).
5. `chmod +x` the wrapper script.

**Tests:** (executable spec)
- `multi_step_preflight_contract_holds`: setup — locally built camel-cli (release),
  wrapper script; action — run wrapper, block until BENCH_ROUTE_READY appears on
  stdout, issue the same POST as the preflight, read body+header, kill child; assert —
  body `PING-M1-M2`, header `X-Bench-Stage: two`; command — scripted shell sequence
  documented verbatim in the YAML header comment (no cargo test harness exists for
  scenario fixtures — same convention as http-server); expected — passes only when all
  steps execute in order (a no-op script, dropped branch, or wrong derivation order
   breaks the exact string or header).
- Verification executed (task 2.1): wrapper reached BENCH_ROUTE_READY; preflight POST returned body PING-M1-M2 + header x-bench-stage: two; 5 repeat requests identical; fail-path (aborted child) produced no marker.

**Acceptance:**
- Wrapper reaches BENCH_ROUTE_READY and the preflight assertions pass against a
  locally built CLI.
- `bash -n benchmarks/scenarios/multi-step/rust-camel-cli/multi-step-cli-wrapper.sh` exits 0.
- Route loads without YAML/schema errors (visible in CLI startup logs during the
  preflight run).

- [x] 2.1

## verification

### Task 3.1: c16/c300/c1000 convoy-signature run

**Files:**
- `benchmarks/scenarios/multi-step/artifacts/c16.json` (new)
- `benchmarks/scenarios/multi-step/artifacts/c300.json` (new)
- `benchmarks/scenarios/multi-step/artifacts/c1000.json` (new)
- `benchmarks/scenarios/multi-step/RESULTS.md` (new)

**Steps:**
1. Build camel-cli release and bench-loadgen in the worktree.
2. Start the multi-step fixture via its wrapper; block until BENCH_ROUTE_READY.
3. Run `loadgen measure-throughput --url=http://127.0.0.1:8081/bench-multi
   --duration-secs=30 --warmup-secs=5 --workers=4 --connections=16
   --out=benchmarks/scenarios/multi-step/artifacts/c16.json` (equals-form flags:
   the CLI only parses `--key=value`; space form silently defaults — see the
   operator note in RESULTS.md and bd rc-bujs).
4. Before the c1000 run, check file-descriptor capacity: `ulimit -n` must exceed
   1100 (1000 sockets + loadgen/camel-cli process overhead); if it does not, raise it
   for the shell session (`ulimit -n 4096`) and record the effective limit in
   RESULTS.md — an fd-exhaustion failure must not masquerade as a benchmark result.
   Then run with `--connections=1000
   --out=benchmarks/scenarios/multi-step/artifacts/c1000.json` (repeat the c300 run
   with `--connections=300
   --out=benchmarks/scenarios/multi-step/artifacts/c300.json` before it).
5. Verify all three artifacts against the convoy gate: error_rate_pct < 1.0,
   non-degenerate per-second buckets at c1000 (no zero-value seconds), `"workers": 4`
   present with `"connections": 16` / `300` / `1000` respectively.
6. COMMIT all three JSON artifacts (they are small — one bucket object per second — and
   are the machine-checkable evidence; environment-specific numbers are acceptable in
   a benchmark-results artifact by convention, cf. docs/benchmarks/*).
7. Write `RESULTS.md`: all runs' profiles (workers, connections, mean per-second,
   p50, error rate), the computed c300/c16 and c1000/c16 throughput ratios, the interpretation
   (post-rc-vdy2 expectation: flat or sub-linear degradation), and the verbatim
   inspection commands (jq/python one-liners) so any reader can recompute the ratios
   from the committed artifacts.
8. Stop the fixture.

**Tests:** (executable spec)
- `artifacts_satisfy_convoy_gate`: action — inspect the three committed artifacts with
  the commands recorded in RESULTS.md; assert — `error_rate_pct` < 1.0 in all three,
  c1000 buckets contain no zero-value seconds, matching `"workers": 4`, `"connections"`
  values 16, 300, and 1000; command — the jq/python one-liners recorded in RESULTS.md;
  expected — pass required for task completion.

**Acceptance:**
- All three artifact JSONs are committed and satisfy the convoy gate.
- `RESULTS.md` exists with profiles, both ratios, and verbatim recomputation commands.

- [x] 3.1
