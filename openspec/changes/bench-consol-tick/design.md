# Design: bench-consol-tick

## Context

Owner-ruled product: `bash benchmarks/bench run-all` measures EVERY
active scenario × every contender, one run = one complete record. Phase
0 (main commit `7db78627`) retired the subset machinery; this change
removes the last two blockers to the first complete record: disk waste
(fixture-per-scenario builds) and the warm gap (single-shot fixtures).

Authority citations (e_opus verification 2026-08-31; oracle e_gpt
authoritative fix set): the pairing model and build-topology contracts
live in `benchmarks/harness/CONTEXT.md` — §3:129-183 (pairing fairness;
esp. 146-149 classpath isolation), §2:106 / §4:196 (fixture-local
`target-dir` pin + `env -u CARGO_TARGET_DIR` NixOS gotcha, workspace
member-non-default pattern), §1:37-40 (contender/artifact/pairing
glossary). The root `CONTEXT.md` is not where these live; cite the
harness file.

## The expected matrix (52 cells, asymmetric)

Five full scenarios × 8 contenders = 40 cells; the two bridge scenarios
(xsd-validation-bridge, xslt-bridge) carry the documented 4-artifact
core (camel-standalone-dsl, camel-quarkus-dsl-native, rust-camel-lib,
rust-camel-cli) + the 2 node contenders = 6 cells each = 12. Total
**52**, matching the harness expected-cell computation (`benchmarks/harness/run.sh`
scenario wiring). Every completeness claim below uses this roster, not
a 7×8 product.

## Per-family decisions (papal verdicts, oracle-confirmed)

| Family | Decision | Why |
|---|---|---|
| rust-camel-lib | Consolidate: one crate at `benchmarks/contenders/rust-camel-lib/`, `<scenario>` argv dispatch, per-scenario route-builder modules; replaces the 7 scenario-crate workspace `members` entries | 5.2 GB → ~1 GB; deps compile once; same artifact across scenarios makes cross-scenario comparison clean; era-2 has zero published records so re-baselining is free |
| rust-camel-cli | No change (build) — Phase B adds tick emission via its existing wrapper latency-file plumbing | Already one build + per-scenario YAML — the pattern lib copies |
| camel-standalone-dsl/-yaml | No change (per-scenario jars) | Classpath-isolation fairness is load-bearing (§3:146-149); ~90 MB win does not justify re-opening it; stale `target/` trees purged post-run instead |
| camel-quarkus-*-native | Exempt by construction | AOT bakes the route into the binary; per-scenario artifacts are the measurement, not waste (821 MB inherent) |
| node-native / node-fastify | Consolidate runtime dir `benchmarks/contenders/node/` | fastify node_modules 15 MB × 7 → 1; node-native has no deps (except XML: saxon-js/xmllint-wasm) |

Consolidation changes **build topology only** — the contender×pairing
matrix and report rows are unchanged (§1 glossary: same contenders,
same pairs). Per-scenario DATA (shared payloads, goldens, parity
hashes) stays under `scenarios/<scn>/`.

## Guard re-keying (load-bearing)

`assert_family_completeness` and the expected-cell count today key on
`scenarios/<scn>/<member>` directory presence (`benchmarks/harness/run.sh:1499-1520`,
`3094-3106`). The node move deletes that evidence — enforcement would
go silently dark. Both checks MUST re-key on registered-cell evidence
(the `add_cell` roster built during wiring) or an explicit family
registration map. Behavior preserved: a completeness-declaring family
registering a subset of selected active scenarios is a hard error
listing the missing scenarios; a fixture present for an inactive
scenario warns, not errors. RED-proof: temporarily unregister one node
fixture → the run aborts naming the family and the missing scenario.

## Measurement integrity (M1)

- The consolidated lib binary grows (~4.8 MB → ~8-12 MB union of
  scenario deps). Extra `ld.so`/page-table work at exec is
  sub-millisecond against 24-46 ms cold starts — below host noise
  (§6:272). Non-executed route code does not page in (lazy paging keeps
  RSS honest).
- **Guard**: no scenario-dispatch work before the marker. Fixture
  shape: `parse argv → build ONLY the selected route → emit
  SCENARIO_MARKER → (Phase B) tick loop`. The marker fires on the same
  code-path position as today.
- Workspace parity: the consolidated crate MUST remain a root-workspace
  member (non-default; `members` list gains the one new path, loses the
  seven scenario paths) so lib and cli resolve the SAME root
  `Cargo.lock` — dep-version parity between Pair A and Pair B is a
  §2/§4 invariant. `cargo metadata` verification (build once, resolve
  binary path with `CARGO_TARGET_DIR` unset) lands in the wiring.

## Phases

Two delivery phases, one plan-bless (full multi-phase `tasks.md`
authored before the single plan-blessing):

- **Phase A — Consolidation**: move lib + node fixtures (workspace
  member list edit included), re-key the completeness guard + expected
  cells on registration, wire harness resolution to the consolidated
  artifacts, COVERAGE exemption/requirement notes, meta `scenarios`
  active-only, smoke-parity of all 52 cells. Phase-exit criteria:
  every expected cell builds, every marker fires, `BENCH_INPUT_SHA256`
  digests byte-identical to pre-move values, guard RED on partial
  family, dry-run green.
- **Phase B — Tick mode**: add tick loops (marker-then-loop) to the 3
  scenarios × ALL EIGHT runtimes — including `rust-camel-cli` (wrapper
  latency-file plumbing exists from the bridge precedent) — and REMOVE
  `t2-realistic-eip` from the Protocol B m2 skip list (`benchmarks/harness/run.sh:2323-2330`) when emission lands. Phase-exit criteria: `parse-protocol-b`
  n>0 for all 24 warm cells; M1 unperturbed — quantitative gate: per
  cell, median time-to-marker within **max(±15%, ±3 ms)** of the
  pre-change median, **n≥30** samples per cell, statistic = median,
  ANY cell outside tolerance blocks the phase exit.
- Inter-phase review gate after Phase A (≥2 tasks → mandatory
  inter-phase review per conductor flow).

## Completeness definition + fail-closed publish

A record is COMPLETE iff every cell of the EXPECTED registered roster
for `meta.json.scenarios` is measured: m1 data for all; m2 data for
every cell whose scenario's warm concept applies. Warm applicability is
declared scenario vocabulary: `startup-minimal` = n/a (cold-only by
design); `http-server` = applicable (Protocol A loadgen); t2-json,
split-aggregate, t2-realistic-eip, xsd-validation-bridge, xslt-bridge =
applicable (Protocol B ticks). The expected roster is derived with the
same asymmetry rules as the harness expected-cell computation
(5×8 + 2×6 = 52) — persisted in the record (`expected_cells` count or
equivalent) or recomputed deterministically by the publisher; a wholly
absent cell (no directory, no JSON) counts as missing, not as
"vacuously complete".

`bench publish` (the owner-facing facade; delegates to
`summarize.py --publish`) SHALL fail closed on incomplete records:
nonzero exit, listing every missing cell (scenario/contender/metric).
The no-gap rule is enforced in code, not advisory.

## Risks

- **Cargo target-dir pin migration**: the fixture-local pin transfers
  verbatim to `contenders/rust-camel-lib/.cargo/config.toml`; the
  harness binary resolution already goes through `cargo metadata` with
  `CARGO_TARGET_DIR` unset. Drift here breaks every lib cell at once —
  the smoke-parity gate catches it.
- **Guard re-key regression**: moving from directory evidence to
  registration evidence changes what RED looks like; the Phase-A
  RED-proof (unregister one fixture → abort naming family+scenario) is
  the guard's own acceptance test.
- **Node script moves vs harness resolution**: harness resolves entry
  scripts per scenario; moves must update the wiring map. Markers fire
  from the new locations; the re-keyed guard no longer depends on paths.
- **Tick loops change fixture CPU behavior post-marker**: Protocol B
  parsing and the 30 s marker deadline are unchanged; a runaway tick
  loop dies by the existing tree-kill between rounds.
- **Canon drift from Phase 0**: subset/run_id deltas must match shipped
  behavior exactly (timestamp run_id, `scenarios` meta key, legacy
  `run_seq`/`subset` read-paths kept for old run dirs).
