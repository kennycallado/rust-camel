# Design: ci-job-timeouts

## Approach

Pure YAML edit, `.github/workflows/**` only. For each of the 47 jobs in 23 files: audit current state, gather duration evidence, apply the value rule below, add a single `timeout-minutes:` line at job level (where the platform allows the keyword). One value per job covers all matrix legs. No other key moves.

## Value rule

- **R1 — existing value: keep.** Never lower. Existing values that look tight against evidence are reported as findings, not changed.
- **R2 — evidence:** direct gh job timing (last ~15 completed runs) OR a declared same-shape sibling (identical job structure, same runner matrix, shared build scripts — e.g. the three `*-bridge-release.yml` workflows are same-shape siblings of `jms-bridge-release.yml`). Value: nearest 5 ≥ 2× max observed, minimum 10.
- **R3 — documented duration** (in-file comment): 2× the documented value.
- **R4 — sibling-class pin:** match a sibling workflow's pinned value for the same workload.
- **R5 — no evidence:** class default with a note: light/utility 15, integration/test 30, publish fan-out 30.
- **Floors (declared):** cold-cache-sensitive classes carry minimums: release/cross-compile build 90, docker build 60. Where evidence (R2/R4) yields less, value = max(evidence-derived, floor); the row states both numbers.
- **Reusable-workflow caller jobs (exception):** `timeout-minutes` is NOT a supported keyword for jobs that call a reusable workflow (`uses:`) per GitHub Actions docs — adding it invalidates the workflow. Such jobs get disposition `N/A (caller)` and are bounded transitively by callee job timeouts. This is the only sanctioned exception to "every job"; residual risk below.

## Audit table — all 47 jobs

Evidence key: gh n=X med/max from `gh run list`+`gh api` job timings, warm cache.

### Additions (22 jobs get `timeout-minutes`)

| # | Workflow | Job | Current | Evidence | New | Rule |
|---|---|---|---|---|---|---|
| 1 | cxf-bridge-release.yml | guard | none | jms guard gh n=1 max 5m (same-shape sibling) | 10 | R2: 2×5, min 10 |
| 2 | cxf-bridge-release.yml | build | none | jms builds gh 14/20/27m warm (same-shape sibling) | 90 | max(R2 2×27=55, floor 90) |
| 3 | cxf-bridge-release.yml | release | none | jms release gh n=1 1m (same-shape sibling) | 10 | R2: min 10 |
| 4 | docs.yml | validate | none | gh n=3 med 9 max 11 | 25 | R2: 2×11 → 25 |
| 5 | docs.yml | deploy | none | gh n=3 max 2 | 10 | R2: min 10 |
| 6 | integration-http.yml | integration-http | none | mail/redis/rhai pinned 30 | 30 | R4 sibling |
| 7 | integration-sql.yml | integration-sql | none | redis/rhai pinned 30 | 30 | R4 sibling |
| 8 | integration-surreal.yml | integration-surreal | none | redis/rhai pinned 30 | 30 | R4 sibling |
| 9 | integration-wasm.yml | integration-wasm | none | ci.yml wasm-integration pinned 40 | 40 | R4 same workload |
| 10 | jms-bridge-release.yml | guard | none | gh n=1 max 5m | 10 | R2: 2×5, min 10 |
| 11 | jms-bridge-release.yml | build | none | gh linux 14 / macos 27 / windows 20 | 90 | max(R2 55, floor 90) |
| 12 | jms-bridge-release.yml | release | none | gh n=1 1m | 10 | R2: min 10 |
| 13 | musl-jemalloc-verify.yml | verify | none | in-file comment ~20 min | 40 | R3: 2×20 |
| 14 | release-matrix.yml | build | none | gh per-target med 7–13 max 50 (x86_64-apple-darwin) | 100 | R2: 2×50 |
| 15 | release-matrix.yml | release | none | none | 15 | R5 light + note |
| 16 | release-matrix.yml | docker | none | docker legs gh ≤1m warm (supplementary: docker-smoke analog 23–28m incl. Rust compile, not same-shape) | 60 | max(R2 min 10, floor 60) |
| 17 | release-matrix.yml | closure-check | none | gh med 34 max 43 (dev releases run it) | 90 | R2: 2×43 → 90 |
| 18 | release.yml | ci-preflight | none | none; API queries only, fails closed, no polling | 15 | R5 light + note |
| 19 | release.yml | publish | none | none; multicrate crates.io publish | 30 | R5 publish + note |
| 20 | xml-bridge-release.yml | guard | none | jms guard gh 5m (same-shape sibling) | 10 | R2: min 10 |
| 21 | xml-bridge-release.yml | build | none | jms builds gh 14–27m (same-shape sibling) | 90 | max(R2 55, floor 90) |
| 22 | xml-bridge-release.yml | release | none | jms release gh 1m (same-shape sibling) | 10 | R2: min 10 |

### Caller jobs — platform exception (2 jobs, no edit)

| # | Workflow | Job | Current | Disposition | Rule |
|---|---|---|---|---|---|
| 23 | release.yml | call-release-matrix | none | N/A — `timeout-minutes` unsupported on `uses:` jobs; adding it invalidates the workflow | Transitively bounded: callee release-matrix.yml jobs get 100/15/60/90 |
| 24 | release-dev.yml | call-release-matrix | none | N/A — same keyword restriction | Transitively bounded: same callee |

### Keeps — existing values unchanged (23 jobs)

| # | Workflow | Job | Current | Rule |
|---|---|---|---|---|
| 25 | assurance-monthly.yml | fuzz-deep | 330 | R1 |
| 26 | assurance-monthly.yml | mutants-baseline | 330 | R1 |
| 27 | assurance-monthly.yml | mutants-rotating | 330 | R1 |
| 28 | assurance-monthly.yml | report | 15 | R1 |
| 29 | bridge-jvm-tests.yml | bridge-jvm-tests | 40 | R1 |
| 30 | bridge-jvm-tests.yml | jms-bridge-native-tests | 60 | R1 |
| 31 | ci-weekly.yml | macos-unit-full | 120 | R1 (finding: gh max 102 — 15% headroom, adequate) |
| 32 | ci.yml | unit-tests | 90 | R1 |
| 33 | ci.yml | macos-build-smoke | 90 | R1 |
| 34 | ci.yml | msrv-check | 90 | R1 |
| 35 | ci.yml | full-tests-linux | 150 | R1 (finding: gh max 139 — 8% headroom; report only, separate change if it flakes) |
| 36 | ci.yml | docker-smoke-test | 60 | R1 |
| 37 | ci.yml | coverage | 100 | R1 |
| 38 | ci.yml | quality | 40 | R1 |
| 39 | ci.yml | bench-smoke | 20 | R1 |
| 40 | ci.yml | wasm-integration | 40 | R1 |
| 41 | fuzz-smoke.yml | fuzz-smoke | `${{ github.event_name == 'workflow_dispatch' && 30 \|\| 20 }}` | R1 — expression kept verbatim |
| 42 | integration-mail.yml | mail-docker-tier | 30 | R1 |
| 43 | integration-ollama-smoke.yml | ollama-embed-smoke | 15 | R1 |
| 44 | integration-redis.yml | integration-redis | 30 | R1 |
| 45 | integration-rhai.yml | integration-rhai | 30 | R1 |
| 46 | published-bridge-smoke.yml | smoke | 60 | R1 |
| 47 | published-jms-bridge-smoke.yml | smoke | 90 | R1 |

22 + 2 + 23 = 47 jobs, every job in every file accounted for.

## Residual risk (caller jobs)

A caller job's wall time cannot be capped by the platform. Longest configured dependency-path budget: build 100 + max(docker 60, release 15) = 160 min (docker and release both depend only on build and run in parallel; excludes queueing/platform delays). A hang between callee jobs remains uncapped. Accepted platform limitation; documented here and in the park. Everything else in every workflow is explicitly bounded.

## Deferred half (out of scope)

Orphan cleanup between dirty steps and network-binding tests: filed as bd `rc-2w5sj` (`discovered-from:rc-0nrer`) with the rc-y24l/rc-qdt3 evidence. Not in this diff.

## Affected crates

None — CI infrastructure only. No `crates/**`, no `scripts/**`, no `AGENTS.md`.

## Architecture boundaries

No runtime/DSL/component/service/language surface touched. `openspec/specs/ci-compile-gate/` and `test-result-integrity/` (landed today) were checked: they govern compile gates and test-result handling, not job time budgets — `skip_specs: true` is correct.

## Alternatives considered

- **Repo-wide `defaults.run.timeout-minutes`** — not a thing GitHub Actions supports at workflow level; rejected.
- **Per-step timeouts** — finer than needed, more edit surface, rejected; job-level bounds the whole.
- **Bounding caller jobs via a wrapper job** — requires restructuring `needs:` graphs; forbidden by the order; rejected.
- **Raising `full-tests-linux` 150** — existing value; R1 keeps it; reported as finding instead.

## Honesty note

No CI behaviour claim beyond this diff: timeouts take effect on the next workflow run; the first real evidence is a run on `main`.
