# First Container Run — Postmortem and Closure Doctrine (2026-08-31)

**Status:** authoritative postmortem for bd `rc-f4po` (the v1 container record).
**Authority:** subordinate to the harness code (`benchmarks/harness/run.sh`,
`run-all.sh`, `builder/build-all.sh`). Where prose and code disagree, code wins.

The first real container benchmark run died eight times in sequence, each a
different latent layer. This document records the failure class, the ninth
latent layer found by systematic audit (fixed here), the audit surface swept,
and the structural closure that ends the class permanently.

---

## 1. The failure class

Every one of the eight deaths shares one shape:

> **A host capability, control-flow assumption, or piece of container-local
> state that the era-1 bare-metal host silently satisfied, but the container
> does not — and the failure is silent (no marker, no error, or an inherited
> non-zero return code swallowed by `set -e`).**

The deaths were not random. They are the layers of an onion: each fix exposed
the next latent assumption, because the run had never executed end-to-end in a
container before. This is expected for a first containerization and is not a
sign of a bad harness — it is a sign that the harness was written against an
implicit host contract that containerization makes explicit.

### Incident ledger (all fixed + verified before this audit)

| # | Layer | Fix commit |
|---|---|---|
| 1 | Image had no C toolchain (`linker cc not found`) | `e7525c19` |
| 2 | No `libc6-dev` (crt objects missing) | `faf00389` |
| 3 | `build-all.sh` `REPO_ROOT` one level shallow + `find\|while` silent death under `set -euo pipefail` | `d1a5a112` |
| 4 | No docker CLI in image (pre-flight abort) | `038b626f` |
| 5 | Socket mount without docker gid (permission denied) | `ef8a3179` |
| 6 | Repo mounted at `/work` only → nested Mandrel builder bind-mounts resolved to nothing on the host | `c9a5bbee` |
| 7 | Quarkus native stripped `ListAppendStrategy` (YAML `#class:` reflection, never registered) | `139cb70e` |
| 8 | `_kill_process_tree_recursive` bare `return` inherited rc=1 under `set -e` — silently aborted the harness | `59829ece` |
| + | Observability: container-local scratch lost on `--rm` → `BENCH_SCRATCH_DIR` host-visible; M1 per-cell echo | `64cd6d7e` |
| + | `mawk` lacks `asort` → `gawk` added to image; image re-pinned | `d8027e55` |

Two sub-classes recur:

- **Host-tool / host-state assumptions** (1, 2, 4, 6, +scratch, +gawk): the
  container lacks a binary, a mount path, or a persistent directory the bare
  host provided.
- **Unguarded non-zero under `set -e`** (3, 8): a command or function returns a
  non-zero code that is *semantically fine* (a fast process already reaped, a
  partial `find`) but, called bare under `set -euo pipefail`, aborts the whole
  run with no message.

---

## 2. The ninth latent layer (found by this audit, fixed here)

**Same class as incident #8, one layer deeper.**

The M1 *measured-runs* loop called `measure_once` **bare** under
`set -euo pipefail`:

```sh
# run.sh, M1 measured loop (before fix)
measure_once "$cell" "$SCRATCH_DIR/${cell//\//_}.txt"
```

`measure_once` legitimately returns `1` on four transient conditions:

1. process exited before the marker was observed,
2. 30 s marker deadline exceeded,
3. `marker_count != 1`,
4. invalid / zero RSS read from `/usr/bin/time`.

Under `set -e`, a **bare** function call that returns non-zero aborts the
script. Verified:

```
set -euo pipefail; f(){ return 1; }
for r in 1 2 3; do echo "round $r"; f; echo "end $r"; done
# → prints "round 1", then EXITS. rounds 2,3 never run.
```

Over the full run — 50 rounds × up to 26 cells, including node's ~26 ms timing
edge (the exact condition behind incident #8), JVM cold-start jitter, and
listener-port reuse between cells — the probability of **at least one**
transient `return 1` across a 2–4 h run approaches 1. A single blip in round 1
of 50 would silently discard the entire record.

### Why the fix is warn-and-continue, not FATAL

The M2 protocol-A/B driver already made the correct choice:

```sh
m2_measure_protocol_a "$cell" ... || echo "warn: ... continuing" >&2
```

M1 measured runs must match it. Rationale:

- A single transient failure must never kill a multi-hour unattended record.
- `n=50` with occasional dropped samples still yields a valid nearest-rank p95
  (the metric is an order statistic over whatever clean samples landed).
- A **genuinely** broken cell surfaces as `no data` in the final per-pair
  summary, which the human reviews **before** `bench publish`. No silent bad
  data reaches a published record.
- Warmup keeps its **FATAL** semantics unchanged (a failed warmup means the
  smoke test lied; `run.sh` M1 warmup loop still `|| { … exit 1; }`). A cell
  that cannot produce even one clean sample is caught **before** measured runs
  begin. The fix narrows fatality to the warmup gate, exactly where the
  documented invariant places it (`benchmarks/harness/CONTEXT.md` §2, "warmup
  failure is FATAL").

The fix (this change):

```sh
measure_once "$cell" "$SCRATCH_DIR/${cell//\//_}.txt" \
    || echo "warn: m1 measured run failed for $cell (round $((r+1))/$N); dropping sample, continuing" >&2
```

**Proven live** in the M2 smoke below: three JVM cells failed warmup-stability
(under-provisioned warmup, a test-parameter artifact) and the run continued to
`rc=0`, recording the surviving cells' samples.

---

## 3. Audit surface swept (evidence, not theory)

The audit swept every sub-class of the failure taxonomy against the current
pinned image `sha256:1247326c…`.

### 3.1 Host-tool inventory (verified present in the pinned image)

`bash 5.1.16` (supports `local -A`, `${arr[-1]}`), `gawk` (as `/usr/bin/awk`
via alternatives — `asort` available), `mawk`, `jq`, `sort`, `taskset`,
`timeout`, `setsid`, `flock`, `pgrep`, `pkill`, `stat`, `comm`, `getconf`,
`nproc`, `/usr/bin/time`, `cargo`, `java 21`, `mvn`, `gradle`, `node 22.14.0`,
`docker 28.5.2`. Locale is `en_US.UTF-8`, but the container run is launched with
`-e LC_ALL=C` (`run-all.sh`), which is the harness's documented requirement for
the `/usr/bin/time` RSS-label parse and locale-stable YAML/JSON parsing.

**One tool is MISSING: `python3`.** It is **not** on the m1/m2 critical path.
`summarize()` inside `run.sh` is a bash+awk function; the python
`summarize.py` is invoked **host-side** only, by `bench summarize` / `bench
publish`, **after** the container run writes raw samples + `meta.json`. The
host has `python3`. No action required for the run; noted so nobody adds a
container-side `summarize.py` call without also adding python3 to the image.

### 3.2 Unguarded-non-zero-under-`set -e` sweep

Every `return`, `|| return`, `&& return`, and bare-function-call-in-loop in the
m1/m2 reachable paths was inspected:

- `_kill_process_tree_recursive`: `return 0` (fixed in #8) — safe.
- `resolve_shared_src_main` bare `return` (line ~972): reached only after a
  successful `echo` (rc=0); build-phase only, not m1/m2 — safe.
- `measure_once` internal `return 1`s: all guarded by the caller **after this
  fix**.
- M2 protocol A/B: already guarded with `|| echo warn` — safe.
- `build-all.sh` `find\|while` and `cargo … | tail -3`: guarded post-#3;
  `cargo` failure correctly propagates via pipefail (fail-loud on real build
  break, which cannot fire spuriously because natives are fingerprint-cached).

The M1 measured-loop bare call was the **only** unguarded reachable instance.

### 3.3 Container-local state that dies with `--rm`

- Scratch: fixed (`BENCH_SCRATCH_DIR` host-visible, `64cd6d7e`).
- Results: written to `BENCH_RESULTS_ROOT` (bind-mounted `harness/out/<ts>`).
- Bridge PID/latency files (`/tmp/v3-*`): container-local, but the **v1 subset
  does not include bridge scenarios** (`xslt-bridge`, `xsd-validation-bridge`
  are excluded). No `/tmp/v3-*` lifetime crosses the run boundary in v1. The
  bridge feature flags (`METRIC_BRIDGE_TRACKING`, `METRIC_RSS_SAMPLE`) default
  off and are only switched on for the excluded scenarios.

### 3.4 M2 first-execution risks

The M2 protocol-A loadgen→contender seam had never run in a container. The
smoke (below) exercised it end-to-end on `http-server` (a v1 subset scenario)
including **both node cells** (`node-native`, `node-fastify`): full BCa
bootstrap p50/p95/p99 samples produced, `rc=0`. The xml-bridge seam is **not**
in the v1 subset — confirmed excluded.

### 3.5 Timing-edge classes (incident #8 pattern)

The M1 smoke ran `node-native` at 23 ms — inside the same
observed-in-`/proc`-poll-but-reaped-before-kill window that produced #8. The
kill-tree fix held and the measured-loop fix (this change) guarantees that even
if a future edge produces a `return 1`, the run survives.

---

## 4. Verification performed against the current pinned image

All on `sha256:1247326c2f505…` with the M1 measured-loop fix applied.

1. **`bash -n`** on `run.sh`, `run-all.sh`, `build-all.sh` — clean.
2. **`pin.sh --report`** — node `22.14.0`, docker CLI `28.5.2` (drift guard
   intact).
3. **1-cell M1 smoke** (`--scenarios=startup-minimal --metric=m1 --n=1
   --warmup=0`): all 8 cells produced samples, `rc=0`, gawk tables printed,
   native fingerprint-cache skipped rebuilds, samples host-visible. node cells
   at 23 ms (edge condition) — green.
4. **M2 protocol-A smoke** (`--scenarios=http-server --metric=m2 --rounds=1
   --samples-per-round=200 --warmup-time=2 --warmup-msgs=200`): rust-camel +
   node cells produced full BCa samples; 3 JVM cells failed warmup-stability
   (under-provisioned 2 s warmup — a test-parameter artifact, NOT a harness
   bug; the real run uses 30 s) and the run **continued to `rc=0`**, proving
   the warn-and-continue resilience live.

---

## 5. Disk verdict

- `/home` (repo partition) had **3 G free** at audit time. Results/out are
  KB-scale. All four v1-subset `rust-camel-lib` binaries and all natives are
  cached; `build-all.sh` is idempotent.
- **Minimal safe reclaim (done):** remove the three **non-v1** fixture targets
  (`xslt-bridge` 877 M, `xsd-validation-bridge` 889 M, `t2-realistic-eip`
  688 M = **2.45 G**). They are not in the v1 subset; removing them lifts
  headroom to ~5.4 G with zero impact on the run. Reversible (`cargo build`
  rebuilds them).
- **Deferred (do NOT land before the record):** `CARGO_TARGET_DIR` →
  `/home/shared` consolidation (bd `bench-build-consolidation`). It is a
  structural change to build layout; landing it immediately before a record
  invalidates fingerprint caches and risks a fresh multi-hour native rebuild.
  Land it **after** the v1 record.

---

## 6. Structural closure — the in-container integration smoke gate

The eight deaths + the ninth found here all share one property: **they are only
observable when the harness runs inside the container against the real image.**
Unit tests, `bash -n`, and host-side dry-runs cannot see them. The permanent
closure is therefore a **thin in-container smoke gate** that runs the shortest
possible real matrix cell inside the pinned image and asserts `rc=0` + samples
produced.

### 6.1 What it runs (the exact contract)

A single command, ~1 min, the same one used in this audit's step 3:

```
BENCH_SUBSET=v1 out-of-tree smoke:
  docker run … <pinned image> …
    run.sh --scenarios=startup-minimal --metric=m1 --n=1 --warmup=0
  assert: rc=0 AND >=1 samples.txt with a "<ms> <rss_kb>" line for
          rust-camel-lib AND both node cells.
```

Optionally extend with one protocol-A cell
(`--scenarios=http-server --metric=m2 --rounds=1 --samples-per-round=50
--warmup-time=30 --warmup-msgs=1000`) for ~3 min total, which additionally
exercises the loadgen seam and JVM warmup convergence.

### 6.2 Where it lives (budget-scaled placement)

- **RUNBOOK step 0 (mandatory, now):** the operator runs the 1-cell M1 smoke
  against the pinned digest **before** every canonical `run-all` launch. Budget:
  ~1 min. This is the cheapest, highest-value placement and requires no CI
  spend. Add it to `benchmarks/runner/RUNBOOK.md` as "Step 0: smoke the pinned
  image".
- **pre-merge (recommended) for changes touching `benchmarks/harness/**`,
  `benchmarks/runner/**`, or `benchmarks/scenarios/**`:** the same 1-cell smoke,
  gated behind a path filter so it only runs when the harness or image inputs
  change. Budget: ~1 min of runner time per such PR. This catches a new latent
  layer at the commit that introduces it, not 2 h into a record.
- **CI (optional, deferred):** a nightly full-image smoke is over-budget for the
  value; the pre-merge path filter already covers the surface. Do not add a
  per-PR unconditional container smoke — most PRs do not touch the harness.

### 6.3 Why this ends the class

The gate closes the loop that produced eight sequential deaths: it makes the
container the *first* place a harness change is exercised, not the last. Any
future host-assumption or unguarded-`set -e` regression fails the ~1-min gate at
authoring time instead of surfacing hours into an unattended record.

---

## 7. Follow-ups for the human to bless (not done here)

- `bench-build-consolidation` (`CARGO_TARGET_DIR` → `/home/shared`): land
  **after** the v1 record.
- Wire the §6.2 pre-merge path-filtered smoke into CI (a `.github/workflows`
  job); RUNBOOK step 0 is the immediate, zero-CI-cost closure.
- `rc-am22` (rust-lib http-server per-request lines regression) and `rc-dh7t`
  (bare auto-discovery aborts on inactive multi-step) remain open, unrelated to
  the v1 record path.
