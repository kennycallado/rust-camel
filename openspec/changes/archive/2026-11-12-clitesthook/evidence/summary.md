# Stability evidence — `job_signal_second_signal_forces_exit`

Change: `clitesthook` (task 1.3). Worktree
`/home/shared/rust-camel-worktrees/clitesthook`, branch `feature/clitesthook`.
Raw data: `evidence/stability-runs.jsonl` (one JSON object per invocation).

## Setup

```
cargo test -p camel-cli --test job_signal_test --no-run
  -> Executable tests/job_signal_test.rs (target/debug/deps/job_signal_test-71d8caa686d1268c)
```

Every run invokes that binary directly:

```
target/debug/deps/job_signal_test-71d8caa686d1268c \
  job_signal_second_signal_forces_exit --exact --test-threads=1
```

`elapsed_s` is the wall time of the whole binary invocation (spawn to exit),
measured with `EPOCHREALTIME` around each run. The test's internal INT→exit
bound stays enforced by `FORCE_EXIT_BOUND` inside the test; the 10 s outlier
threshold here applies to the outer wall time only.

The test is a real subprocess test, not a stub: `strace -f -e trace=execve`
on one invocation shows
`execve("/home/shared/rust-camel-worktrees/clitesthook/target/debug/camel", ...)`
plus the two `kill` invocations (INT, TERM).

## Unloaded sweep — 50 runs

| runs | pass (exit 0) | fail | min | median | max | mean | outliers |
| ---- | ------------- | ---- | --- | ------ | --- | ---- | -------- |
| 50   | 50            | 0    | 0.030 s | 0.055 s | 0.059 s | 0.052 s | none |

- Pass/fail tally: **50/50 exit 0**. No non-zero exit occurred, so no
  `timeout`-style 124 could be folded into anything.
- Outliers (wall elapsed ≥ 10 s): **none**. The spread is 29 ms top-to-bottom;
  the slowest run is 0.059 s against a 10 s threshold.
- No wall-time trend over the sweep (runs 1, 25 and 50 are 0.030 s / 0.055 s /
  0.055 s).
- Caveat: these wall times are WARM page-cache runs (the ~466 MB debug binary
  was exec'd repeatedly). Cold first-exec boot measured 8-16 s by the mission
  conductor's independent probes (well inside the 30 s WAIT ceiling, but above
  the 10 s warm outlier threshold) — the sweep measures warm stability and
  says nothing about cold-start margin; CI runs cold and stays bounded by the
  harness ceilings, not by the outlier threshold.

## Load sweep — 10 runs, one batch

Exactly one loaded batch, as specified:

- 12 CPU spinners (one per `nproc`), `sh -c 'while :; do :; done' &`, PIDs
  tracked, killed from a `trap ... EXIT` handler in the driving script.
- 10 additional runs of the same invocation, each wrapped in `timeout 60`.
- Trap fired on script exit; every spinner PID verified gone afterwards
  (`kill -0` per PID: none alive; the only `pgrep -f` match was the checking
  shell's own command line).

| runs | pass (exit 0) | fail | min | median | max | mean | outliers |
| ---- | ------------- | ---- | --- | ------ | --- | ---- | -------- |
| 10   | 10            | 0    | 0.115 s | 0.133 s | 0.147 s | 0.132 s | none |

Under 12-way CPU saturation the wall time lands around 2.4x the unloaded
median (0.055 s → 0.133 s) and every run still exits 0. No run approached
`timeout`'s 60 s cap.

Combined: **60 runs, 60 exit 0, zero non-zero exits, zero ≥ 10 s outliers.**

## Negative control (task 1.2, `evidence/negative-control.md`)

With the `CAMEL_JOB_PIPELINE_MARKER` seam emission temporarily commented out,
the same test **fails** on the marker wait: exit 101, `panicked at
crates/camel-cli/tests/job_signal_test.rs:317: camel job never reached the
pipeline-entry seam marker`, `test result: FAILED. 0 passed; 1 failed;
finished in 30.04s`. After a byte-identical `git checkout --` restore it
passes again (exit 0). So the green runs above depend on the observed
pipeline-entry seam, not on timing luck.

## Gate sweep (worktree `/home/shared/rust-camel-worktrees/clitesthook`)

### Registry gates (`AGENTS.md` § QUALITY GATES, run via `scripts/gates-local.sh`)

`gates-local.sh` exit **0**; summary line: `passed: 29 failed: 0 skipped: 4
ran: 29 of 33`.

| gate | result |
| ---- | ------ |
| fmt check | PASS (exit 0) |
| clippy | PASS (exit 0) |
| lint-unwrap | PASS (exit 0) |
| lint-glob-imports | PASS (exit 0) |
| lint-unsafe-safety | PASS (exit 0) |
| lint-missing-docs-ratchet | PASS (exit 0) |
| lint-secrets | PASS (exit 0) |
| lint-single-source | PASS (exit 0) |
| lint-non-exhaustive | PASS (exit 0) |
| lint-log-levels | PASS (exit 0) |
| lint-log-redaction | PASS (exit 0) |
| lint-cancel-tokens | PASS (exit 0) |
| lint-test-sleep | PASS (exit 0) |
| lint-no-free-port | PASS (exit 0) |
| lint-no-raw-http-test-server | PASS (exit 0) |
| lint-no-env-mutation-unguarded | PASS (exit 0) |
| lint-unbounded-wait | PASS (exit 0) |
| lint-ignore | PASS (exit 0) |
| lint-publish-cycles | PASS (exit 0) |
| lint-publish-registration | PASS (exit 0) |
| lint-component-deps | PASS (exit 0) |
| lint-bridge-versions | PASS (exit 0) |
| lint-gate-forwarding | PASS (exit 0) |
| lint-gate-parity | PASS (exit 0) |
| lint-no-deprecated | PASS (exit 0) |
| lint-context-citations | PASS (exit 0) |
| lint-metric-labels | PASS (exit 0) |
| schema-check | PASS (exit 0) |
| doc-build | PASS (exit 0) |
| lint-commits | skipped (CI) — needs `git fetch origin main`; also skipped by conductor policy (remote fetch op) |
| doc-guide-test | skipped (CI) — mdbook toolchain; `docs/**` unchanged by this change |
| cargo-audit | skipped (CI) — advisory DB download |
| lint-bridge-releases | skipped (CI) — GitHub release API |

All 33 registry gates are listed: 29 executed and passed, 4 skipped by
`gates-local.sh`'s default network/toolchain skip list and left to CI.

### Additional touched-crate commands (task step 5)

| command | exit | note |
| ------- | ---- | ---- |
| `cargo fmt --check --all` | 0 | clean |
| `cargo clippy -p camel-core --all-targets -- -D warnings` | 0 | clean |
| `cargo clippy -p camel-cli --all-targets -- -D warnings` | 0 | clean |
| `RUSTDOCFLAGS="-D warnings" cargo doc -p camel-core -p camel-cli --no-deps` | **101** | fails in `camel-cli` only — see below. PRE-EXISTING, not this change: both failing files (`camel-cli/src/compile/{materialize,policy}.rs`) are byte-identical to `main` (blob-hash identity) and this diff touches only tests, a 7-line camel-core seam, and openspec artifacts. Filed as bd `rc-7ej4d` (fix: backtick the private names instead of linking). The repo's own AGENTS.md `doc-build` gate excludes `camel-cli` and passes. |
| `RUSTDOCFLAGS="-D warnings" cargo doc -p camel-core --no-deps` | 0 | the half this change touches is clean |
| `bash scripts/gates-local.sh` | 0 | 29/33 ran, all PASS |

#### The `-p camel-cli` doc failure is pre-existing and unrelated

`rustdoc` rejects private intra-doc links in `crates/camel-cli/src/compile/`:

```
error: public documentation for `materialize` links to private item `standard_materialization_candidates`
  --> crates/camel-cli/src/compile/materialize.rs:15:22
error: public documentation for `create` links to private item `standard_materialization_candidates`
  --> crates/camel-cli/src/compile/materialize.rs:138:12
error: public documentation for `policy` links to private item `is_in_memory_class`
  --> crates/camel-cli/src/compile/policy.rs:42:7
```

Verification without running cargo on `main` (forbidden): `materialize.rs` and
`policy.rs` are byte-identical between `main` and `HEAD`
(`git rev-parse main:<path>` == `git rev-parse HEAD:<path>` for both), and
`git diff main...HEAD` changes only `crates/camel-cli/tests/job_signal_test.rs`,
7 lines in `crates/camel-core/src/lifecycle/adapters/route_controller_trait.rs`,
and the `openspec/changes/clitesthook/` artifacts. The gate's scope
(`camel-cli` src) does not intersect this change. The repository's own
`doc-build` gate does not include `camel-cli` at all and passed above.

## Conclusion

The marker-synchronized test is stable: 50/50 unloaded and 10/10 loaded runs
exit 0, with tight wall-time distributions and no outliers, and the negative
control shows the run depends on the seam marker. All registry gates that can
run locally pass; the single failing command is a pre-existing `camel-cli`
rustdoc issue on code identical to `main`.
