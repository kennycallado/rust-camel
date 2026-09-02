# Proposal: bench-consol-tick

## Why

Two converging needs, one change (e_opus ruling 2026-08-31, task
`ses_fa7757520ffeR2xPDaxtVawipt` — follow-up to the one-command ruling;
spec blessed BLESS-WITH-FIXES by the oracle e_gpt, authoritative fix
set applied):

1. **Disk**: the rust-camel-lib fixture compiles its own dep tree PER
   SCENARIO (fixture-local `target/`, 7×) — **5.2 GB measured** on this
   host. The owner asked for "one build per contender that can run all
   scenarios"; papal verification confirmed the lib family as the thief
   and blessed consolidation.
2. **Warm gap**: t2-json, split-aggregate and t2-realistic-eip fixtures
   are single-shot in EVERY runtime (marker, then idle) — warm cells
   with no data, blocking the first complete era-2 record under the
   owner's rule "no publicamos si nos faltan datos".

Fusion is the point: Phase B (tick loops) rewrites the same fixtures
Phase A (consolidation) restructures. One rewrite, not two.

## What Changes

- **Phase A — Consolidation** (structural):
  - `rust-camel-lib`: 7 per-scenario fixture crates → ONE parametrized
    crate at `benchmarks/contenders/rust-camel-lib/` (`src/main.rs`
    dispatch + `src/scenarios/<scn>.rs` route builders), replacing the
    seven scenario-crate entries in the root workspace `members` list
    (shared root `Cargo.lock` dependency parity preserved — the
    `benchmarks/harness/CONTEXT.md` §2/§4 member-non-default pattern moves with
    it). ~5.2 GB → ~1 GB.
  - `node-fastify`: per-scenario `node_modules` → ONE shared runtime dir
    (`benchmarks/contenders/node/`); per-scenario entry scripts move
    there; `node-native` scripts move alongside (dependency-free except
    XML scenarios).
  - Per-scenario DATA stays per-scenario (payloads, goldens,
    `BENCH_INPUT_SHA256` parity hashes) — only builds consolidate.
  - **Completeness guard re-keyed**: `assert_family_completeness` and
    the expected-cell count currently key on
    `scenarios/<scn>/<member>` fixture directories — the node move
    deletes that evidence. Both checks MUST switch to registered-cell
    evidence (the `add_cell` roster) or an explicit family registration
    map, independent of fixture directory layout. Partial-family
    failure and inactive-scenario warning behavior are unchanged (RED
    on a family registering a subset).
  - **Untouched by design**: `rust-camel-cli` (already one build + YAML
    routes — the pattern Phase A copies), `camel-standalone-*` JVM jars
    (per-scenario; consolidating would re-open the classpath-isolation
    fairness contract `benchmarks/harness/CONTEXT.md` §3:146-149 for a
    ~90 MB win — refused), `camel-quarkus-*-native` (AOT bakes the
    route; per-scenario artifacts ARE the measurement — exempt forever;
    `benchmarks/scenarios/COVERAGE.md` MUST state this exemption).
  - `benchmarks/scenarios/COVERAGE.md` gains the consolidation
    exemptions as requirements (not aspirations): cli/JVM/native exempt
    with reasons; node/lib consolidated with the new locations.
  - meta.json `scenarios` MUST list ACTIVE scenarios only
    (`multi-step` stays out; the launch-time discovery excludes
    non-registered scenario dirs, not just `spike-*`).
- **Phase B — Tick mode** (behavioral): `BENCH_LATENCY` tick loops
  added to t2-json, split-aggregate, t2-realistic-eip in **ALL EIGHT
  runtimes per scenario** (standalone-dsl, standalone-yaml,
  quarkus-dsl-native, quarkus-yaml-native, rust-camel-lib,
  rust-camel-cli — including its wrapper latency-file plumbing —,
  node-native, node-fastify). With one consolidated fixture per family,
  Phase B is 3 loops × the runtime set. Fixture shape (blessed):
  `parse scenario arg → build selected route → emit SCENARIO_MARKER →
  tick loop`. No dispatch work before the marker; non-selected route
  builders never execute. `t2-realistic-eip` is REMOVED from the
  Protocol B m2 skip list when tick emission lands (today
  `benchmarks/harness/run.sh` skips it — leaving it would silently void 8 Phase-B cells).
- **Completeness definition + fail-closed publish** (rides along, per
  papal Phase 2, amended by the oracle): a record is complete when the
  EXPECTED registered roster for `meta.json.scenarios` is fully
  measured — every expected cell has cold (m1) data AND warm (m2) data
  where the scenario's warm concept applies. The expected roster is the
  asymmetric **52-cell matrix** (5 full scenarios × 8 contenders + 2
  bridge scenarios × 6: 4 core + 2 node). `startup-minimal` warm is
  `n/a` by design, not a gap. `bench publish` FAILS CLOSED on
  incomplete records (nonzero exit + full missing-cell list including
  wholly absent cells) — the owner's no-gap rule is enforced, not
  advisory.
- **Canon sync** for the retired-on-main Phase 0 (commit `7db78627`):
  `benchmark-suite`/`benchmark-records` deltas bring the spec in line
  with shipped behavior (full-matrix default, timestamp `run_id`,
  `scenarios` meta key; legacy `run_seq`/`subset` read-paths kept for
  old run dirs).

## Acceptance criteria

- Phase A exit gate: all **52** expected cells build and emit their
  `SCENARIO_MARKER` in smoke; per-scenario `BENCH_INPUT_SHA256` digests
  are byte-identical to pre-move values; `cargo metadata` verifies the
  consolidated crate is a root-workspace member replacing the seven
  scenario-crate entries (one shared root `Cargo.lock`), the crate
  builds once, and `env -u CARGO_TARGET_DIR` semantics are preserved; the completeness
  guard still fails RED on a family registering a subset (verified by
  temporarily unregistering one node fixture).
- Phase B exit gate: the 3 tick scenarios emit `BENCH_LATENCY` records
  after the marker under the existing Protocol B contract in all 8
  runtimes (`parse-protocol-b` sees n>0 per cell; no m2 skip for
  `t2-realistic-eip`); M1 marker timing unperturbed — quantitative
  gate: per-cell median time-to-marker within max(±15%, ±3 ms) of the
  pre-change median, n≥30 samples per cell, any cell outside tolerance
  blocks the phase exit.
- Dry-run green; meta.json `scenarios` lists exactly the 7 active
  scenarios.
- Disk steady state post-run ≈ 9.2 GB (lib target ~1 GB, root target
  4.2 GB unchanged, natives 821 MB unchanged).

## Excluded

- `multi-step` (inactive, owner-ruled out of the race).
- JVM/Quarkus-native consolidation (per-family verdicts above).
- Cache relocation (`benchmarks/.cache` off `/home`) — separate bd
  follow-up, low priority (post-consolidation headroom ≈ 11.3 GB free).
- Any measurement run / record publication (owner runs
  `bash benchmarks/bench run-all` after merge; human-invoked).
