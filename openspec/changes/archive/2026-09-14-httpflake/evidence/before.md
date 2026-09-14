# httpflake — Task 1.1: Loaded-soak repro BEFORE the fix

**Date:** 2026-09-14 (soak window 17:35:02–17:35:24 CEST)
**Worktree:** `/home/shared/rust-camel-worktrees/httpflake` (branch `feature/httpflake`)
**Verdict:** **REPRO OBSERVED** — acceptance (a) satisfied.
**Code edits:** none. Tree pristine.

## Environment

| Item | Value |
|---|---|
| `nproc` | 12 |
| CPU burners | 12 (one busy loop per logical CPU) |
| Load average (12 burners active, 17:34:31) | 6.75 |
| Load average (immediately after soak, 17:35:26) | 8.13 |
| `/home/shared` free before soak | 60 G (71% used, 201 G total) |
| `/home/shared` free after soak | 60 G (71% used) |
| Crate | `camel-component-http` (`crates/components/camel-http`) |
| Test count (full suite) | 367 |

## Pristine-tree verification (before soak)

- `git status --short` → clean (no output).
- `git rev-parse HEAD` → `15734a720ca4e694430c52dc663dbc2c58e8d659` (plan-bless commit).
- `git diff 2606c1dc..HEAD -- crates/` → **empty (0 bytes)**. `crates/` is byte-identical to spec-bless commit `2606c1dc`.
- No source file was edited in this task.

## Exact commands

Build (once, before soak):

```
cargo test -p camel-component-http --lib --no-run
```

Full suite, under 12 burners, up to 8 runs:

```
cargo test -p camel-component-http --lib -- --test-threads=12
```

Targeted high-collision variant, up to 5 runs (only when full runs stay green):

```
cargo test -p camel-component-http --lib -- content_type_inferred registry --test-threads=12
```

> Command-form note: the task text wrote the targeted filter without a `--`
> separator (`... --lib content_type_inferred registry -- --test-threads=12`).
> Cargo accepts only one `TESTNAME` positional and rejects the second filter
> (`error: unexpected argument 'registry' found`). The filters were therefore
> passed to libtest after `--`, preserving the intended semantics. All runs
> used `--test-threads=12`.

Burners (detached, output to `/dev/null`, PIDs in `/tmp/httpflake-burners-before.pid`):

```
for i in $(seq 12); do ( while :; do :; done ) > /dev/null 2>&1 & echo $! >> /tmp/httpflake-burners-before.pid; done
```

A wrapper (`logs-before/soak-before.sh`) starts the burners, runs the loop,
and kills every tracked PID on `EXIT`/`INT`/`TERM`.

## Per-run results

Source: `logs-before/results.tsv`.

| Run | Kind | Exit | `did not become ready` matches | Wall (s) | Outcome |
|---|---|---|---|---|---|
| 1 | full | 0 | 0 | 2 | 367 passed |
| 2 | full | 0 | 0 | 2 | 367 passed |
| 3 | full | 0 | 0 | 2 | 367 passed |
| 4 | full | 0 | 0 | 1 | 367 passed |
| 5 | full | 0 | 0 | 2 | 367 passed |
| 6 | full | 0 | 0 | 2 | 367 passed |
| 7 | full | 0 | 0 | 1 | 367 passed |
| 8 | full | 0 | 0 | 2 | 367 passed |
| 1 | targeted | 101 | **1** | 6 | **FAILED — panic captured; loop stopped** |

Loaded green runs: **8** (full) + 0 (targeted, stopped on first). The full-suite
phase stayed green 8/8, so the targeted phase ran and reproduced on its first
iteration.

## Panic excerpt (targeted run 1, `logs-before/run-1-targeted.log`)

```
test tests::test_content_type_inferred_for_xml_body ... FAILED
thread 'tests::test_content_type_inferred_for_xml_body' (1958474) panicked at crates/components/camel-http/src/lib.rs:9249:13:
consumer server did not become ready on port 34613
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
test result: FAILED. 8 passed; 1 failed; 0 ignored; 0 measured; 358 filtered out; finished in 5.01s
```

- Failing test: `tests::test_content_type_inferred_for_xml_body`
- Run number: targeted run 1
- Panic site: `crates/components/camel-http/src/lib.rs:9249` (the `assert!` in `setup_consumer_on_free_port`'s readiness poll)
- Message: `consumer server did not become ready on port 34613`

### Supplementary calibration run

Before the numbered soak, one targeted run was used to calibrate timing
(`logs-before/supplementary-calibration-targeted.log`). Load conditions:
it ran under a manually started 12-burner batch, killed and verified dead
before the numbered soak script started (PID forensics in the task-1.1
review confirm the calibration test threads predate the pidfile burner
PIDs). It also failed, with **three** concurrent readiness panics.
The excerpt below is CONDENSED (three panic lines joined; fields elided)
— see the raw log for verbatim lines:

```
test tests::test_content_type_inferred_for_xml_body ... FAILED
test tests::test_content_type_inferred_for_text_body ... FAILED
test tests::test_content_type_inferred_for_json_body ... FAILED
consumer server did not become ready on port 40223 / 40647 / 41483
test result: FAILED. 6 passed; 3 failed; ... finished in 5.01s
```

## Burner hygiene (post-task)

- `while read p; do kill -0 $p; done < /tmp/httpflake-burners-before.pid` → 0 alive.
- `pgrep -af 'while :; do'` → none.
- No burner started by this task survives.

## Disk

- `/home/shared` free before: 60 G. After: 60 G. Never approached the 5 G stop threshold.

## Artifacts

- `logs-before/soak-before.sh` — soak wrapper (burner trap + run loop).
- `logs-before/results.tsv` — machine-readable per-run table.
- `logs-before/run-{1..8}-full.log` — full-suite logs.
- `logs-before/run-1-targeted.log` — repro log (panic).
- `logs-before/supplementary-calibration-targeted.log` — 3-panic calibration log.
