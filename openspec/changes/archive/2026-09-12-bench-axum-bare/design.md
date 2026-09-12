# Design: bench-axum-bare

## Approach

One new reference contender, wired through the existing contender pattern so
the generic per-cell machinery (m1 marker clock, m2 protocol A, m3, m4, GNU
time RSS) measures it at the next canonical run without harness surgery.

**Fixture** (`benchmarks/contenders/axum-bare/`, package `axum-bare-fixture`,
bin `axum-bare-fixture`): axum 0.8 (root workspace dep, lock 0.8.9 — lock delta limited to the
fixture's own package entry) + tokio (rt-multi-thread, macros, net). `#[tokio::main]`
multi-thread runtime (same shape as the rust-lib fixture). Route: any-method
`/bench` handler that (1) prints `BENCH_HTTP_REQUEST received`, (2) drains the
request body via `axum::body::to_bytes(body, 1 MiB)` (T3 posts ~32 KiB
bodies; keep-alive requires the drain — same work devnull and camel-http
do), (3) increments an
`AtomicU64` counter and prints `BENCH_HTTP_REQUEST id=<n>` (starting at 1 —
smoke contract, rc-am22), (4) responds `200 text/plain; charset=utf-8` body
`pong` (mirrors camel-http's reply finaliser). Marker: after
`TcpListener::bind` succeeds, spawn `axum::serve`, then `println!`
`BENCH_ROUTE_READY` (bare T3 shape per `SCENARIO_MARKER`) **with explicit
stdout flush** — std stdout is a LineWriter (line-buffered even when
piped), so the newline already emits the line; the explicit flush follows
the devnull family convention (cli_runtime.rs run_devnull) and makes the
marker-then-serve ordering explicit. Port: `8080`
default (T3 family binds 0.0.0.0:8080, cells are mutually exclusive),
`BENCH_AXUM_BARE_PORT` env override for the integration test and smoke
(mirrors `BENCH_DEVNULL_PORT` at run.sh:755).

**Roster mechanics** (the rc-2k33 three-file contract, extended by one
projection):

- run.sh: a `declare -A REFERENCE_CONTENDERS=(["http-server"]="axum-bare")`
  block (greppable, same shape as `SCENARIO_ARTIFACT_SET`) + a conditional
  registration in the non-bridge resolver that `add_cell`s
  `http-server/axum-bare` with the bare marker, launching the fixture binary
  resolved from the fixture-local target path (same resolution + `env -u
  CARGO_TARGET_DIR` semantics as rust-camel-lib). NOT added to
  PAIR_A/PAIR_B_CONTENDERS (pairs stay 4+4; the reference contender is not a
  pair member). The m1 console summary prints one extra "Reference" line per
  scenario from the same map. Because the family is not declared in
  FAMILY_COMPLETENESS, per-scenario opt-in is the documented mechanism for
  http-server-only participation.
- summarize.py: `HTTP_REFERENCE_CONTENDERS = {"http-server": ("axum-bare",)}`;
  `expected_roster()` extends the per-scenario contender tuple accordingly →
  53 identities (5×8 + 2×6 + 1). `WARM_APPLICABLE` and `BRIDGE_*` untouched
  → http-server/axum-bare automatically joins the warm/publish completeness
  gate (it is a 4-metric cell by design).
- checks/warm-24.py: untouched (axum-bare is not a tick contender; gate stays
  24/24).
- test_summarize.py: `test_roster_mirror_no_drift` extended to grep
  `REFERENCE_CONTENDERS` from run.sh and assert equality with
  `summarize.HTTP_REFERENCE_CONTENDERS` **in both directions**; 52-pins at
  :1220, :1346, :1349, :1353, :1411 become 53 with the reference-cell
  arithmetic named.

**Backward compatibility** (mission gate): published records persist
`expected_cells` in run.json; `--check` guard 5 regenerates only summary.md
from the stored run.json (summarize.py:1505-1506) — no roster re-derivation.
A pre-edit hash of `--check` output (and of every summary.md) is captured
before the first roster edit and re-verified after — byte-identical proof.

**Smoke**: `scenarios/http-server/smoke/run.sh` gains an axum-bare case
(build if needed, launch on a free port via the env override, POST /bench,
assert marker + `200`/`pong` + `id=1`) plus an optional artifact filter so
the case can run standalone without the JVM toolchain. Committed
`axum-bare.log` from a real run of this change; log carries no timing
numbers (liveness evidence, not a measurement — sealed-policy compatible per
rc-am22 precedent).

**Docs**: harness/CONTEXT.md roster-contract row updated (three-file contract
+ reference projection, 53 arithmetic); records/SCHEMA.md expected_cells
prose era-qualified ("52-cell rosters persist in records published before the
axum-bare reference cell joined; post-change runs expect 53"); strategy doc
§4 rc-audm.6 gets an additive supersession pointer to the §8 refutation.

**bd landings**: rc-u034 closed (code landed; measurement belongs to the next
canonical run); rc-audm.6 closed after filing a linked P3 residual
("compare xslt cli vs lib at next canonical run", discovered-from rc-audm.6);
rc-audm.8 stays open (live warmup-design defect) with an addendum citing the
empty-diff identity proof — `git diff --stat 53730f19 HEAD -- warmup.rs
run.sh` is empty — plus the current-code grep cites.

## Affected crates

- NEW `benchmarks/contenders/axum-bare` (`axum-bare-fixture`): the fixture.
- Root `Cargo.toml`: members list + default-members exclusion — **named
  out-of-zone exception** (pre-flight ruling); shared Cargo.lock; the
  only allowed lock change is the `axum-bare-fixture` package entry —
  third-party versions stay pinned (axum/tokio already in the lock).
- No camel crate changes. benchmarks/harness (run.sh, summarize.py,
  test_summarize.py, builder/build-all.sh), benchmarks/scenarios/http-server
  (smoke), benchmarks docs, openspec specs — all in-zone.

## Architecture boundaries

Benchmarks-zone only; no product-crate (Runtime/DSL/Components) surface is
touched. The fixture deliberately sits OUTSIDE the camel stack — its purpose
is to isolate hyper/axum/tower/tokio stack-tax from camel-tax, so it must not
depend on any `camel-*` crate. Data/control-plane boundary: not applicable
(no runtime product code).

## Alternatives considered

- **Fixture as a bench-loadgen bin (devnull placement)** — rejected: devnull
  is measurement apparatus (calibration baseline); a roster contender is a
  system under test and belongs in `contenders/` with its own crate, per the
  consolidated-builds precedent.
- **Appending axum-bare to FULL_CONTENDERS** — rejected: 4 of 5 added cells
  would be meaningless (no tick/cold semantics for a bare HTTP handler),
  would drag warm-24 from 24 to 27 cells, and would break Pair A/B arithmetic
  (4+4). The scenario-conditional reference tuple keeps every existing
  invariant intact.
- **Era bump to 3** — rejected: additive cell, no schema/protocol change;
  `expected_cells` is record data, not schema.
