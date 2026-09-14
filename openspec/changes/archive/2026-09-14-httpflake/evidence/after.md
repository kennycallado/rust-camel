# httpflake — Task 1.4: Loaded-soak AFTER the fix (formal evidence, RE-RUN)

**Date:** 2026-09-14 (soak window 19:12:14–19:15:18 CEST)
**Worktree:** `/home/shared/rust-camel-worktrees/httpflake` (branch `feature/httpflake`)
**Verdict:** **CLEAN — 28/28 loaded runs green, 0 readiness panics.**
**Code edits:** none by this task. The completed Tasks 1.2/1.3 fix is present in the
working tree (uncommitted): the rogue unguarded reset caller now holds
`REGISTRY_TEST_MUTEX`, and the helper + hammer recover from mutex poison.

## Environment

| Item | Value |
|---|---|
| `nproc` | 12 |
| CPU burners | 12 (one busy loop per logical CPU) |
| Load average (soak start, 19:12) | 3.82 |
| Load average (after soak, 19:15) | 13.34 |
| `/home/shared` free before soak | 51 G (150 G used, 75% of 201 G) |
| `/home/shared` free after soak | 51 G (150 G used) |
| Crate | `camel-component-http` (`crates/components/camel-http`) |
| Test count (full suite) | 369 (367 + 2 new regression tests) |
| HEAD | `15734a720ca4e694430c52dc663dbc2c58e8d659` (plan-bless commit) |

## Fix under test (uncommitted diff, `crates/components/camel-http/src/lib.rs`)

- `registry_rejects_tls_on_plain_port` (lib.rs ~6195) now acquires
  `REGISTRY_TEST_MUTEX` before its `ServerRegistry::reset()` — the last of the
  23 reset call sites to be guarded (with `#[allow(clippy::await_holding_lock)]`,
  matching the 29 existing guarded sites).
- Poison-recovering acquire in the helper (`wait_for_registry_ready` caller path,
  lib.rs ~9233) and the hammer thread (lib.rs ~9322):
  `unwrap_or_else(|poisoned| poisoned.into_inner())` — the mutex guards test
  serialization only, so a failed sibling must not cascade.
- Earlier fix (Tasks 1.2/1.3, already in tree): `REGISTRY_TEST_MUTEX` held
  stage→ready in `setup_consumer_on_free_port` + `wait_for_registry_ready`
  (1 ms→64 ms backoff, 10 s deadline).

## Exact commands

Build (once, before soak):

```
RUSTC_WRAPPER= cargo test -p camel-component-http --lib --no-run
```

Full suite, under 12 burners (exactly 15 iterations):

```
RUSTC_WRAPPER= cargo test -p camel-component-http --lib -- --test-threads=12
```

Targeted high-collision + regression filter (multi-filter goes after `--`;
cargo accepts one positional TESTNAME only):

```
RUSTC_WRAPPER= cargo test -p camel-component-http --lib -- content_type_inferred registry readiness_survives --test-threads=12
```

Supplementary targeted batch (10 iterations, same command as above).

Burners (detached, PIDs in `/tmp/httpflake-burners-after2.pid`), started/killed by
`logs-after2/soak-after2.sh` (same trap pattern as Task 1.1):

```
for i in $(seq 12); do ( while :; do :; done ) > /dev/null 2>&1 & echo $! >> /tmp/httpflake-burners-after2.pid; done
```

## Per-run results (this run)

Source: `logs-after2/results.tsv`.

| Run | Kind | Exit | `did not become ready` matches | Wall (s) | Outcome |
|---|---|---|---|---|---|
| 1 | full | 0 | 0 | 12 | 369 passed |
| 2 | full | 0 | 0 | 11 | 369 passed |
| 3 | full | 0 | 0 | 11 | 369 passed |
| 4 | full | 0 | 0 | 12 | 369 passed |
| 5 | full | 0 | 0 | 11 | 369 passed |
| 6 | full | 0 | 0 | 11 | 369 passed |
| 7 | full | 0 | 0 | 11 | 369 passed |
| 8 | full | 0 | 0 | 11 | 369 passed |
| 9 | full | 0 | 0 | 11 | 369 passed |
| 10 | full | 0 | 0 | 12 | 369 passed |
| 11 | full | 0 | 0 | 11 | 369 passed |
| 12 | full | 0 | 0 | 11 | 369 passed |
| 13 | full | 0 | 0 | 11 | 369 passed |
| 14 | full | 0 | 0 | 11 | 369 passed |
| 15 | full | 0 | 0 | 12 | 369 passed |
| 1 | targeted | 0 | 0 | 1 | 10 passed |
| 2 | targeted | 0 | 0 | 1 | 10 passed |
| 3 | targeted | 0 | 0 | 1 | 10 passed |
| 1 | supplementary | 0 | 0 | 1 | 10 passed |
| 2 | supplementary | 0 | 0 | 1 | 10 passed |
| 3 | supplementary | 0 | 0 | 1 | 10 passed |
| 4 | supplementary | 0 | 0 | 1 | 10 passed |
| 5 | supplementary | 0 | 0 | 1 | 10 passed |
| 6 | supplementary | 0 | 0 | 1 | 10 passed |
| 7 | supplementary | 0 | 0 | 1 | 10 passed |
| 8 | supplementary | 0 | 0 | 1 | 10 passed |
| 9 | supplementary | 0 | 0 | 1 | 10 passed |
| 10 | supplementary | 0 | 0 | 1 | 10 passed |

**28/28 green. 0 `did not become ready` matches across all 28 logs. 0 panics**
(the only `panicked` string in any log is the passing test name
`monitor_task_handles_panicked_task`). Acceptance met: 15/15 full green,
3/3 targeted green, 10/10 supplementary green, 0 readiness panics total.

## The full before/after story

### Before the fix (Task 1.1, `before.md`, pristine tree, same 12-burner load)

- Full suite: green **8/8** (367 passed each) — **non-discriminative**.
- Targeted variant: **FAILED on run 1** — `consumer server did not become ready
  on port 34613` (lib.rs:9249) inside `test_content_type_inferred_for_xml_body`.
- Supplementary calibration: **3 simultaneous readiness panics** (xml/text/json
  bodies) on ports 40223/40647/41483.

The discriminative before-signal lived in the **targeted variant**, not the full
suite.

### First AFTER attempt (failed, `logs-after/` + old `after.md`)

The first Task 1.4 soak ran with the Tasks 1.2/1.3 fix (mutex held stage→ready,
10 s deadline) but **before** the rogue-caller guard. It still failed under load:

- Full suite: runs 1–3 green (369 passed), **run 4 FAILED** — the new regression
  test `readiness_survives_concurrent_registry_reset` panicked at its own 10 s
  deadline (`consumer server did not become ready on port 41575`, lib.rs:9269).
- The panic unwound while holding `REGISTRY_TEST_MUTEX`, **poisoning** it and
  cascading 35 sibling failures (`PoisonError` at every `.lock().unwrap()` site).
- Targeted variant: green 3/3. Supplementary: **run 1 FAILED** with the same
  readiness panic (port 39467) + 8-test poison cascade.

Full details of that attempt (per-run table, panic excerpts, failure-chain
analysis) are preserved in the old `after.md` content summarized above and in
`logs-after/` (`run-{1..4}-full.log`, `run-{1..3}-targeted.log`,
`run-1-supplementary.log`, `results.tsv`, `soak-after.sh`).

### Root cause (`diagnosis.md`)

Six instrumented repro rounds under the same load (`logs-diag/`) narrowed it to a
capture-immune file-append trace:

```
t7: start-ENTER port=34197
t7: cell-SET key=(127.0.0.1:34197)      ← entry inserted
t7: start-GOT-REGISTRY port=34197
t4: reset-CALL                          ← 275µs later, MID-WINDOW
t4: cell-SET key=(127.0.0.1:0)          ← t4 re-inserts its own entry
t7: poll#32..#160 keys=["127.0.0.1:0"]  ← our entry wiped, 10 s → panic
```

`registry_rejects_tls_on_plain_port` (lib.rs ~6194) called `ServerRegistry::reset()`
**without** holding `REGISTRY_TEST_MUTEX` — the only one of 23 reset call sites
unguarded, violating the blessed spec's R2 ("test code that mutates the global
registry SHALL hold REGISTRY_TEST_MUTEX"). Its reset fired mid-window in another
test, wiping the freshly inserted entry; `bound_addr` then polled None forever.

### Completed fix (closing Task 1.3)

1. The rogue caller now takes `let _guard = REGISTRY_TEST_MUTEX.lock().unwrap();`
   before its reset (lib.rs ~6200).
2. Poison-recovering acquire in the helper and the hammer
   (`unwrap_or_else(|p| p.into_inner())`) — kills the 35-test blast radius.

### This run (formal AFTER evidence, `logs-after2/`)

With the completed fix, the previously-failing loaded filter is green
**28/28** (15 full + 3 targeted + 10 supplementary), 0 readiness panics. The
acid test of the previously-failing targeted variant — the discriminative
signal — is green 13/13 under the same 12-burner load (3 numbered + 10
supplementary), where before the fix it failed 1-in-1..4.

> Narrative weighting (per task): the before signal lived in the targeted
> variant (full-suite-before was green 8/8 — non-discriminative). The first
> after attempt moved the failure into the fix's own regression test under
> full-suite load; the completed fix (rogue-caller guard + poison recovery)
> eliminates it: full suite green 15/15, targeted variant green 13/13.

## Burner hygiene (post-task)

- `pgrep -af 'while :' | grep -v pgrep` → none.
- `while read p; do kill -0 $p; done < /tmp/httpflake-burners-after2.pid` →
  0 alive.
- The soak script's `EXIT`/`INT`/`TERM` trap killed and hard-killed every
  tracked PID. No burner survives.

## Disk

- `/home/shared` free before: 51 G. After: 51 G (150 G used both times; this
  task wrote only ~130 KB of logs). Never approached the 5 G stop threshold.

## Artifacts

- `logs-after2/soak-after2.sh` — soak wrapper (burner trap + 15 full / 3 targeted /
  10 supplementary loop).
- `logs-after2/results.tsv` — machine-readable per-run table (28 rows).
- `logs-after2/run-{1..15}-full.log` — full-suite logs, all green.
- `logs-after2/run-{1..3}-targeted.log` — targeted green logs.
- `logs-after2/run-{1..10}-supplementary.log` — supplementary green logs.
- `logs-after/` — first (failed) attempt logs; `logs-diag/` — diagnosis logs;
  `diagnosis.md` — root-cause write-up.

## Conclusion

Task 1.4 acceptance is **met**: 15/15 full green, 3/3 targeted green, 10/10
supplementary green, 0 readiness panics total, under the same 12-burner load
that reproduced the flake before the fix and in the first after attempt.

## Post-evidence hardening (r_glm review finding 1)

After the 28-run evidence above, the poison-cascade fix was completed
suite-wide: every REGISTRY_TEST_MUTEX acquisition in the crate (33 sites in
lib.rs + 3 in static_endpoint.rs) now routes through one poison-recovering
helper (lock_registry_test_mutex, into_inner recovery — the mutex guards
test serialization only). One failing test now fails as ONE test instead of
cascading ~30 PoisonError failures. Gates re-verified after the change:
cargo fmt --check --all clean; cargo clippy -p camel-component-http
--all-targets -- -D warnings clean; full suite 369/0; targeted loaded
filter re-confirmed 6/6 green under 12 burners (same command as the
supplementary batch). The 28-run table above was produced with the
semantically-identical per-site recovery (helper + hammer only); the
hardening extends the same recovery mechanically to every site.
