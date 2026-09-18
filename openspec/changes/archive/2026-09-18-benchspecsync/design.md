# Design: benchspecsync

## Approach

Spec-zone delta correction. Every clause of
`openspec/specs/benchmark-suite/spec.md` was verified against the
current suite implementation (`benchmarks/bench`, `harness/run.sh`,
`harness/run-all.sh`, `harness/summarize.py`, `harness/checks/`,
`harness/loadgen/`, `scenarios/`, `contenders/`, `runner/`,
`records/`, `.github/workflows/ci.yml`) and committed records — by
static reading only; no bench member was executed (owner-exclusive).
drifted clauses become MODIFIED deltas (complete requirement blocks
below in `specs/`); verified clauses are carried verbatim. One docs
fix lands in `scenarios/COVERAGE.md` (tag-retrieval note for untracked
era-1 reports).

## Affected crates

- None (docs/spec zone). Files touched:
  `openspec/changes/benchspecsync/**` (this change),
  `openspec/specs/benchmark-suite/spec.md` (at archive),
  `benchmarks/scenarios/COVERAGE.md` (link hygiene only).

## Architecture boundaries

No runtime surface. The spec canon is normative documentation; this
change tightens the spec↔implementation contract without code motion.
Data/control plane, crate graph, and measurement surfaces untouched.

## Provenance anchors

- rc-h42s6 phase-2 alignment, commit 372c7018 (2026-09-16): minimal-bare
  fixture shape (e_opus rulings D1/D2), node m3/m4 exclusion (D5).
- Owner untracking ef337c0f, bd rc-mq1sh (2026-09-17): `docs/benchmarks/`
  period memos untracked; "Canonical bench records stay tracked under
  `benchmarks/` at the repo root."
- Owner ruling 2026-09-17 (bd rc-h42s6 comment): bench run naming,
  scheduling, and the decision to run at all are owner-exclusive; no
  agent-invented era labels in spec text.

## Inline-edit adjudication (the two known edits)

1. **axum-bare T3 "stdout carries NO per-request trace lines"
   clause — KEEP.** Verified: grep for `BENCH_HTTP_REQUEST` across
   `contenders/axum-bare` (src + tests) = zero matches; absence is
   source-pinned by `harness/test_minimal_bare_shape.py:71-80`;
   drain is real (`main.rs:82-85` `axum::body::to_bytes` before
   `200`/`pong`); keep-alive reuse proven by
   `tests/integration.rs:133-187` (two sequential POSTs, one
   `TcpStream`, 3 writes each, framed reads). Provenance citation
   style (ruling + bd) matches the file's existing convention.
2. **Smoke WARN-only `id=1` clause — KEEP.** Verified:
   `scenarios/http-server/smoke/run.sh:195-197` and `:395-400` print
   WARN lines citing e_opus D2 and `return 0` without touching
   `FAIL`/`FAILED_ARTIFACTS`; HARD assertions are marker-within-30 s
   (`:378-384`) and 200/pong (`:171-177`); transcript drops headers so
   no timing-like numbers (`:410-416`). Only the scenario TITLE
   ("committed evidence") drifted — transcripts were deleted at
   585858c6; retitle + owner-run regeneration clause added.

## Sync evidence (per-clause adjudication)

Legend: MATCH = spec matches code (carried verbatim); MODIFIED = spec
text corrected by this change; HISTORICAL = one-time phase-exit
scenario from its originating change (retained in canon).

| Requirement / Scenario | Verdict | Evidence (current suite) |
|---|---|---|
| Payload-size axis / all 4 scenarios | MATCH | `loadgen/src/payload.rs:21-40,199-267`; `cli.rs:115-143`; fixtures honor `BENCH_PAYLOAD_BYTES` (t2-json only — see MODIFIED req text) |
| Payload-size axis / requirement prose | MODIFIED-5 | BENCH_PAYLOAD_BYTES honored by t2-json only; split-aggregate fixed 100-item array (`run.sh:213-215`), t2-realistic-eip no canonical body |
| t2-json / both scenarios | MATCH | `contenders/rust-camel-lib/src/scenarios/t2-json.rs:88,110-153,264-266,346-371`; 8 smoke logs identical digests; marker map `run.sh:227` |
| split-aggregate / all 3 scenarios | MATCH | `split-aggregate.rs:149-156,215-220,247-255,456-532`; yamls + Java DSL completion_size=100; `run.sh:179,2061` |
| Ratio confidence intervals / all 3 scenarios | MATCH | `loadgen/src/ratios.rs:14-25,209-212,235-358,379-390,525-547`; `cli.rs:448-462` |
| Ratio confidence intervals / requirement prose | MODIFIED-8 | PRNG reused (`bca::SplitMix64`, `ratios.rs:55`); CI method is percentile bootstrap — BCa deliberately not applied to a ratio of medians (`ratios.rs:37-42`) |
| Metric-family overhead / A/B bounded ratio | MATCH | arm Camel.tomls `.metrics-on:21-30` / `.metrics-off:21-28` (port 18191); result labeled lever study in `runner/RUNBOOK.md:171` + `scenarios/COVERAGE.md:143-147` (0.9890, CI [0.9785,1.0126], UNPAIRED) |
| CI bench subset / bench-smoke job | MATCH | `.github/workflows/ci.yml:331-365`; timeout 15, ubuntu, no services; `cargo bench -p camel-bench --bench pipeline --bench body_coercion -- --quick`; both bench targets exist |
| Zone contract / Level-1 audit | MODIFIED-1 | `git ls-files`: tracked `audits/` (4 files, rc-h42s6) + `docs-investigation-strategy.md` (56284c40, pinned by `test_warmup_policy.py:42`, linked `harness/CONTEXT.md:111`); no spike-*/results at level 1 |
| Zone contract / Harness moves without modification | HISTORICAL | one-time move verification of the consolidation change; single squash commit in main history |
| Zone contract / Contenders zone holds builds, not data | MATCH | `contenders/` = rust-camel-lib, node, axum-bare; no payload/golden/parity assets |
| Single facade / both scenarios | MATCH | `bench:47-62` exec passthrough; `run.sh:89-98,946-958,1632` dry-run no-JDK |
| Era-1 freeze / Reports reachable after freeze | MODIFIED-2 | `docs/benchmarks/**` untracked by owner commit ef337c0f (bd rc-mq1sh, 2026-09-17); tag `bench/era-1-final` → 6ec6d25b carries the reports; COVERAGE links currently dangle → task 2 |
| Era-1 freeze / Gauge premise preserved | MODIFIED-2 (GIVEN path only) | ADR-0066:238 quotes 0.9890 CI [0.9785, 1.0126] ✓; addendum now tag-reachable; RUNBOOK:171 mirrors |
| Public terminology confinement / README diet | MATCH | grep README for scan terms (M1-M4, T2j, T-family, paired, bootstrap): zero hits; lowercase `m1`–`m4` (README:31-33) are record-schema metric identifiers (benchmark-records uses lowercase metric ids), not confined vocabulary |
| Contender completeness / both scenarios | MATCH | `run.sh:1435-1481` selection-scoped guard; `:873-875` inactive-scenario notice; `:192-195` SCENARIO_ARTIFACT_SET bridge reduction; node family all-7 |
| Node contender family / cell registration | MATCH | `run.sh:1541-1562`; node-native stdlib-only except XML; fastify binds only http-server (`node-fastify/http-server.mjs:48` sole `listen`) |
| Node contender family / digest parity | MATCH | shared assets only (`xslt-bridge.mjs:73-76`, `xsd-validation-bridge.mjs:53-56`); smoke golden cross-check `scenarios/t2-json/smoke/run.sh:40-44,135` |
| Node contender family / dry-run without Node | MODIFIED-6 | ONE family `package.json` at `contenders/node/`; single `<would-build:npm-ci>` marker (`run.sh:1176-1180`); per-fixture split no longer exists |
| Pinned Node runtime / pin verification | MATCH | `runner/Dockerfile:79-89` (NODE_VERSION/NODE_SHA256, sha256sum -c before install, && fail-closed); `pin.sh:36-38,67-77,104-108` |
| Pinned Node runtime / XML engine auditability | MATCH | `contenders/node/README.md:386-388,605-609` (xmllint-wasm↔Xerces-J 2.12.2; saxon-JS 2.70↔Saxon-HE 12.5) |
| Canonical full-matrix / one command full coverage | MODIFIED-3 | 53-cell arithmetic `run.sh:3283-3285` ✓; node m3/m4 exclusion `run.sh:2766-2768` (D5) changes the measured-set claim; meta `scenarios` 7-active-only verified MATCH (see dedicated row below) |
| Canonical full-matrix / no subset escape hatch | MATCH | `BENCH_SUBSET` zero hits; `run-all.sh:13-17` |
| Canonical full-matrix / gauges and order survive | MODIFIED-3 | order_seed ✓ (`run-all.sh:98-99,171`; record `protocol.order_seed`); gauges: m3/m4 measured set excludes node family (D5) — m1/m2 keep node cells |
| Canonical full-matrix / human-invoked execution | MATCH | `bench:50-53` → `run-all.sh`; RUNBOOK exists |
| Canonical full-matrix / meta.json scenarios list | MATCH | `run-all.sh:129-152` writes a preliminary discovery list at launch (find minus `spike-*`, so it names `multi-step`); registration then resolves the active set (`run.sh:865-876` skips inactive dirs with a notice) and the meta-hygiene step rewrites `meta.json.scenarios` to the active roster via jq before measurement (`run.sh:3337-3351`, bench-consol-tick task 1.5) — final value is the 7 active names, matching this spec and benchmark-records L19-21 |
| Consolidated contender builds / single build, all scenarios | MATCH | argv table `main.rs:28-56`; root-workspace member, non-default; no per-scenario crates under `scenarios/*/` |
| Consolidated contender builds / reference contender builds standalone | MATCH | `builder/build-all.sh:33-37`; `.cargo/config.toml` target pin; `run.sh:2034` binary path |
| Consolidated contender builds / smoke parity after the move | HISTORICAL | one-time post-consolidation verification; static support: committed smoke logs + shape tests |
| Consolidated contender builds / dispatch does not perturb M1 | MATCH | `main.rs:39-41` dispatch-then-build guard |
| Consolidated contender builds / shared node runtime | MATCH | one `node_modules` (`contenders/node/package.json`, fastify 5.12.1); `build-all.sh` npm ci once |
| Consolidated contender builds / completeness guard survives | MATCH | roster-keyed guard `run.sh:1435-1481`; evidence = registered cells; inactive → warning |
| Warm tick mode / requirement prose | MODIFIED-4 | marker latches inside FIRST completed tick (AtomicBool latch `t2-json.rs:264-266`; delay=0 first-fire); rust-camel-cli = direct argv env (`run.sh:1716-1722`), no wrapper for tick scenarios |
| Warm tick mode / protocol B records exist | MATCH | all 24 cells tick (3 scn × 8 runtimes verified fixture-by-fixture); `checks/warm-24.py:41-51`; observed=0 hard-fails `run.sh:2597-2607`; t2-realistic-eip not in skip list (`run.sh:2341-2346`) |
| Warm tick mode / marker timing gate | MATCH | codified: `checks/m1-tolerance.py:12` max(±15%, ±3 ms), n≥30, median; `baseline-medians.py` |
| Warm tick mode / tick parity across runtimes | MODIFIED-4 (scenario wording) | family-completeness guard makes asymmetry a hard error; cli route-mode plumbing verified; delta rewords "wrapper latency-file plumbing" to "argv latency-file plumbing" (run.sh:1716-1722 direct env, no wrapper for tick scenarios) |
| axum-bare / marker after bind, flushed | MATCH | `main.rs:30,38-63` (8080/BENCH_AXUM_BARE_PORT, bind → single flushed marker → serve) |
| axum-bare / T3 route contract (inline edit #1) | KEEP | adjudicated above — clause verified accurate |
| axum-bare / roster registration http-server-only | MATCH | `run.sh:202-204,3283-3319`; Pair A/B `run.sh:3530-3531` exclude axum-bare; `test_summarize.py:1480-1528` |
| axum-bare / roster drift guard | MATCH | `summarize.py:212`; `test_summarize.py::test_roster_mirror_no_drift:1308-1478` |
| axum-bare / no-camel isolation with lock parity | MATCH | `Cargo.toml:7-14` (axum+tokio only); non-default member; lock diff = own package entry (commit 20a7263a) |
| axum-bare / smoke case (inline edit #2) | MODIFIED-7 (title) + KEEP (clause) | WARN-only id verified (adjudicated above); title "committed evidence" false since 585858c6 — transcripts deleted, regeneration owner-run-only |
| axum-bare / published records stay byte-identical | MATCH | `summarize.py:1422` persisted `expected_cells`; records: 20260903=52, 20260915=53 (with axum-bare) |

## Decisions

1. **audits/ codified, docs-investigation-strategy.md kept at level 1** —
   both tracked deliberately (rc-h42s6; test-pinned live-defect link);
   the owner's ef337c0f message explicitly affirms bench artifacts stay
   tracked under `benchmarks/`.
2. **meta.json active-only contract matches code** — the
   launch-time snapshot names the discovery set (including
   `multi-step`), but registration excludes inactive scenarios and
   the meta-hygiene step rewrites `meta.json.scenarios` to the active
   roster before measurement.
3. **No era labels in new spec text** — per owner ruling 2026-09-17,
   drift is cited as "rc-h42s6 phase-2 alignment (2026-09-16)" /
   ruling D5, never as an era name.
4. **COVERAGE.md links** — keep relative links (they resolve in the
   owner checkout where untracked files remain on disk) but add the
   tag-retrieval note; fix the stale "Reports live at
   docs/benchmarks/YYYY-MM-DD..." sentence.

## Open questions

None blocking. The node m3/m4 exclusion lift (rc-wkqt0) will require a
future spec delta when it lands — noted, not actioned.
