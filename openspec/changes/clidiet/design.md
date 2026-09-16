# Design: clidiet

## Approach

Two independent concerns, one change, two spec deltas.

**Allocator dedup.** The Phase A audit (evidence/audit-phase-a.md) shows
zero allocator crates under default features; under `--all-features` each
is pulled solely by camel-cli's own optional deps — there is no
transitive or feature-unification culprit. jemalloc wins the `cfg` race
in `main.rs`, powers `allocator_metrics.rs` (ADR-0066) and heap profiling
(bd rc-i9f9); mimalloc powers one unreachable `#[global_allocator]` line.
Fix: delete the `mimalloc` feature, its optional dep, and the
`main.rs` cfg block; prune `Cargo.lock`. Lockfile evidence note: the
workspace member `benchmarks/contenders/rust-camel-lib` keeps its OWN
optional mimalloc dep, so the `[[package]] mimalloc`/`libmimalloc-sys`
entries REMAIN in the shared lockfile — the expected lock diff is the
removal of camel-cli's `"mimalloc",` dependency edge only. The
"dropped from the graph" evidence is the cargo-tree closure test
(mimalloc stack absent from camel-cli's `--all-features` closure) plus
`cargo tree -p camel-cli --all-features -i mimalloc` erroring with no
matching package for camel-cli's graph. Canonical policy, documented in the spec: default builds use
the system allocator; release/musl builds opt into jemalloc
(`--features jemalloc`, Dockerfile note, soak plan bd rc-vnm8); mimalloc
is removed as an unused alternative. The benchmarks fixture keeps its own
independent `alloc-mimalloc` (untouchable zone).

**Slim feature profile.** The workspace table already consumes
camel-bundles with `default-features = false` (root Cargo.toml), and
camel-cli already forwards the seven feature-gated bundles; that part of
the surface needs no change. The camel-cli-controllable diet surface is:
the LSP server stack (camel-lsp, tower-lsp — non-optional today;
ariadne stays non-optional because `camel lint` also renders with it,
commands/lint.rs) and the five `camel-core` language features (lang-js,
lang-rhai, lang-jsonpath, lang-xpath, lang-minijinja — hardcoded on the
camel-core dep today). These become camel-cli features: `lsp` gating the
two LSP-only deps and the `camel lsp` subcommand, `lang-*` forwarding to camel-core, with
`camel-core = { workspace = true }` carrying no language features by
default. A `full` meta-feature re-aggregates today's exact default set;
`default = ["full"]`. `slim-http` is a named empty feature: the
`--no-default-features` baseline (http, direct, seda, log, mock, timer,
file, controlbus, container, validator, cron, master, template stay
non-optional). Source-level references (the LSP subcommand) get
`#[cfg(feature = "lsp")]` gates.

**Scope boundary (deferral).** camel-bundles depends unconditionally on
jms, sql, redis, opensearch, ws, cxf, xslt, xj; camel-cli cannot exclude
those crates from any profile without editing camel-bundles (out of
lease). They stay linked in slim builds; the exclusion is deferred to a
camel-bundles-side follow-up (deferral ledger + bd by the master).
`camel-language-minijinja` joins the deferred family (execution
finding, task 2.2): `camel-template` — non-optional through camel-cli's
own dependency and camel-bundles — hard-depends on it, so it cannot
leave the slim closure from camel-cli's side. The `lang-minijinja`
feature still exists and re-enables the camel-core forward for full
parity, but the crate itself is present in every profile via
camel-template. Correspondingly, the golden comparison filters
`camel-core feature "lang-*"`/`"camel-language-*"` lines on both sides:
cargo tree renders feature nodes for dep-declaration activation, never
for feature-forwarding, so the lang move from declaration to forwarding
erases 10 golden lines by mechanism, not by closure change.
Already-optional bridges (kafka, grpc, wasm, llm, mcp, mqtt, surrealdb,
exec) are excluded by `--no-default-features` once the `full`
re-aggregation is in place.

**Guards.** `crates/camel-cli/tests/feature_profiles.rs` shells
`cargo tree` (metadata-only, no build-lock conflict). Golden command,
pinned and normalized for path-independence:

    cargo tree -p camel-cli -e features,no-dev --prefix none --locked \
      | sed -E 's| \(/[^)]*\)||g; s| \(\*\)||g; s| \[\*\]||g' | sort -u

The `features` edge kind makes the snapshot prove feature-for-feature
equality (package lines plus `pkg feature "name"` lines), not just the
package closure.

(1) default closure == committed golden
`tests/fixtures/default-deptree.txt` (generated with the same pipeline at
the change base); (2) slim closure (`--no-default-features`) excludes
the controllable set: camel-component-{kafka,grpc,wasm,llm,mcp,mqtt,
surrealdb,exec}, camel-lsp, tower-lsp, and the language-runtime crates
for js, rhai, jsonpath, xpath (NOT minijinja — camel-template
hard-depends on it; ariadne also stays: it serves `camel lint`); (3) `--all-features` closure
contains neither `mimalloc` nor `libmimalloc-sys` (jemalloc IS expected
there — it is the surviving override). The compile matrix (every profile
compiles) runs as explicit build steps in Phase C — the slim-http release
build doubles as its compile proof — rather than as a nested cargo test,
which would deadlock on the parent's target lock.

**Measurements.** `evidence/bench-cold-local.sh` (mission 99 replica:
marker mode spawn→`BENCH_ROUTE_READY`, help mode, `/usr/bin/time -v` max
RSS, 3 warmups discarded) on the full-default and slim-http release
binaries, same toolchain. The fixture boots TWO routes: the one-shot
timer→log marker route and an http consumer (`from:
http://127.0.0.1:0/bench-health` — ephemeral port, no collision) that must bind for boot to
complete — exercising the http capability the profile is named for.
Protocol: n=30 per mode per binary; report median and p90 for
milliseconds, median for max-RSS, exact bytes for size. Acceptance
bands: full-build marker median within ±5% of the Phase A baseline
median; RSS median within ±2 MB; binary size delta zero or explained
(dedup must not change default codegen — zero delta expected and
reported as the no-behavior-change proof).

## Affected crates

- `crates/camel-cli`: feature table, main.rs cfg block, camel-core
  consumption, lsp cfg gates, new profile test + golden fixture,
  CONTEXT.md allocator/profile notes.
- Root `Cargo.toml`: none expected; only if a removal note is needed.
- `Cargo.lock`: pruned (camel-cli's `"mimalloc",` dependency edge removed only; `[[package]]` entries remain — the benchmarks fixture keeps its own).

## Architecture boundaries

Runtime/DSL/Components/Services/Languages are untouched semantically.
The change lives on the binary composition surface: which optional
component crates the camel binary links and how camel-cli expresses its
camel-core and camel-bundles feature forwards from the consumer side.
The `camel-bundles` cascade itself, camel-bundles' Cargo.toml,
camel-core internals, and every component crate are not edited.
Language runtimes are only gated as camel-cli → camel-core feature
forwards. Metrics (ADR-0066) and integration-tier demand-gating
(ADR-0069 §8) semantics are preserved exactly in the default closure.

## Phases

### Phase 1: Allocator dedup
- **Goal:** mimalloc gone from camel-cli under every feature combination.
- **Dependencies:** none.
- **Externally-visible types/interfaces:** feature `mimalloc` removed
  (build-surface change only; nothing in-tree used it).
- **Deliverable:** Cargo.toml/main.rs edits, pruned Cargo.lock (edge
  removal), allocator-graph test.
- **Exit-criteria:** profile test asserts the mimalloc stack absent
  under `--all-features`; default golden deptree unchanged; lock diff
  verified as camel-cli's `"mimalloc",` edge only.

### Phase 2: Slim feature profile
- **Goal:** optional lsp/lang surface + `full`/`slim-http` meta-features
  with identical default closure.
- **Dependencies:** Phase 1 (same feature table).
- **Externally-visible types/interfaces:** new features `full`,
  `slim-http`, `lsp`, `lang-js`, `lang-rhai`, `lang-jsonpath`,
  `lang-xpath`, `lang-minijinja` (ariadne remains non-optional: shared
  with `camel lint`).
- **Deliverable:** feature table rework, lsp cfg gates, camel-core
  consumption change, golden snapshot test, slim exclusion test.
- **Exit-criteria:** golden deptree identical to base (under the
  bilateral lang-feature-edge filter — see Approach); slim closure
  excludes the controllable set; every matrix profile compiles.

### Phase 3: Measurement + sweep
- **Goal:** recorded deltas and a clean sweep.
- **Dependencies:** Phases 1–2.
- **Externally-visible types/interfaces:** none.
- **Deliverable:** bench evidence files (including the dedup
  before/after rows inside results-full.txt and the audit-table baseline
  section), rc-kyq15 verification note.
- **Exit-criteria:** full-vs-slim RSS/startup/size table in evidence;
  full build within the stated bands vs Phase A baseline — this is also
  the dedup no-regression proof (expected zero delta); final verdicts per
  the conductor's paired/tagged protocol (single-session rows retained;
  marker-RSS INCONCLUSIVE with regime-noise attribution accepted); sweep
  disposition recorded.

## Alternatives considered

- **Keep both allocator features (no dedup):** rejected — leaves the
  dead-graph artifact the bd targets.
- **Make mimalloc exclusive instead of removing:** rejected — mimalloc
  has no consumer, no build path, and no production story.
- **Exclude the bundles-unconditional bridges from camel-cli:** rejected
  — impossible without editing camel-bundles (lease); deferred instead.
- **Edit camel-bundles defaults to slim them:** rejected — crosses the
  zone lease; separate mission owns that crate's manifest.
- **xtask profile-matrix subcommand:** rejected — scripts/xtask is
  outside the lease; cargo tree tests inside camel-cli are lock-free and
  sufficient. Compile matrix runs as explicit build steps.
- **trybuild compile_fail tests:** rejected — wrong axis (cfg errors in
  code, not dependency-graph exclusion) and brittle.
