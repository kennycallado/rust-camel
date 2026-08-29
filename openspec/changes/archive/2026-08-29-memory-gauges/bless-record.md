# Bless Record — memory-gauges (re-bless)

- Verdict: **BLESS**
- Gate: expert blessing gate, second round (previous verdict BLESS-WITH-FIXES, findings F1-F8)
- Skill: self-grill-proposals (non-interactive grilling; evidence below)
- Artifacts reviewed: proposal.md, design.md, specs/component-metrics-emission/spec.md
  at this change directory (read in full)
- Reproducible artifact hashes (sha256, this gate's own recipe):
  - proposal.md `cd778d26fb3b1e06…` (full: `cd778d26fb3b1e06` prefix of `sha256sum proposal.md`)
  - design.md `2694d9b372888165…`
  - specs/component-metrics-emission/spec.md `93c8f368bd4f5750…`
- Stated NEW HASH `sha256:9e6b2fb0…` could not be reproduced with 8 standard recipes
  (concat in 3 orders, hash-of-hashes with/without newlines, no-separator join,
  all-files find-cat). The recipe is undocumented; verdict binds to the on-disk
  artifact set reviewed above, which matches the promised post-fix content exactly.

## Fix verification (F1-F8)

| Fix | Where | Evidence | Status |
|---|---|---|---|
| F1 per-call init-executed flag, direct +1, 1 miss + N−1 hits | design D1 (L56-75) | local `AtomicBool` set by init future; `built == true` → miss else hit; "Direct +1 increments … no delta bookkeeping"; delta alternative documented rejected; spec requirement mirrors ("exactly one miss and N−1 hits", "Misses MUST count client constructions"). Implementable as written: existing `get_with` init future already borrows `self` non-'static (client_cache.rs:69-82). | LANDED |
| F2 epoch advanced per sample before reads; MIBs once; warn+retry, never abort | design D4 (L115-125) | "each tick advances `epoch` FIRST (jemalloc caches stats between epoch advancements …)" — rationale included; "initialized once before the loop"; "logs `warn` and retries on the next tick; it never aborts `camel run`". Spec: "Each sample MUST advance the jemalloc epoch before reading stats" + scenario "a read failure warns and retries, never aborts". | LANDED |
| F3 sampler in commands/run.rs after ctx.start(), camel run only | design D4 (L107-110) | "starts in `commands/run.rs` after context construction and `ctx.start()`, capturing a clone of the runtime metrics handle, under `#[cfg(feature = \"jemalloc\")]`"; scoped to `camel run`. Cross-ref: commands/run.rs exists, `ctx.start()` at run.rs:600; `#[global_allocator]` only in camel-cli main.rs:4-9. | LANDED |
| F4 seam signature + three tests | design D5 (L133-144) | `fn emit_allocator_snapshot(read: impl Fn() -> Result<AllocatorSnapshot, String>, metrics: &Arc<dyn MetricsCollector>) -> bool`; Ok → four `set_allocator_memory` emissions, exact values, true; Err → none, warn, false; unwired `MetricsHandle` no-op test. Param type matches `ctx.metrics()` return (`Arc<dyn MetricsCollector>`, context.rs:658). | LANDED |
| F5 controlled spec scenario; growth inference in prose | spec L56-63 + L52-54 | Scenario: one read → exactly four emissions equal to read totals + epoch advanced; requirement prose: "Diagnosis supported: growing `allocated` … flat `allocated` with growing `resident` …". | LANDED |
| F6 Phase 1 cache trio only; allocator surface Phase 2 | design Phases (L160-176) | Phase 1: trait methods (cache trio) + forwarding + cache families + kind/handle/choke point + unit tests; Phase 2: `AllocatorStat` + `set_allocator_memory` + allocator family + `tikv-jemalloc-ctl` optional dep + `AllocatorSnapshot` seam + run sampler; exit criteria: default build green, `--features jemalloc` compiles sampler, seam tests prove mapping + failure policy. | LANDED |
| F7 two-variant HttpComponentKind + as_str invariant test + single choke point incl. ssrf | design D2 (L77-86) | `Http`/`Https`, `as_str() -> "camel-http" \| "camel-https"`, invariant test on the as_str image; `new` gains `(kind, metrics)`; all emission inside `get_or_build`. Cross-ref: production `get_or_build` call sites are exactly lib.rs:2252-2256 and ssrf.rs:389. | LANDED |
| F8 OTEL no-op Non-Goal; specs scoped to wired Prometheus | design Non-Goals L49-51, D3 L102-103, Risks L155-156; proposal L41-42, L62-63; spec L5-6, L44-46 | All five surfaces state it; both spec requirements say "through the wired Prometheus collector". | LANDED |

## Self-grill record (new-defect sweep)

**Questions generated:**
1. [glossary] Does "store the late-bound `MetricsHandle`" conflict with `ctx.metrics()` returning `Arc<dyn MetricsCollector>`?
2. [sharpen] D4 says MIBs "initialized once before the loop" yet init failure "retries on the next tick" — contradiction?
3. [scenario] Do the new `set_*`/`increment_*` call sites survive lint-metric-labels given `self.kind.as_str()` is a field-receiver, not an enum-variant path?
4. [cross-ref] Do design citations hold (TTL 60 s / capacity 64, moka `entry_count`, seda sampler shape, families precedent, bd ids)?

**Answers (with citations):**
1. [glossary] No. `CamelContext` stores `metrics: Arc<MetricsHandle>` (context.rs:53) and `metrics()` returns it unsized-coerced with doc "the shared late-bound handle" (context.rs:655-659). The Arc wraps the ArcSwap handle; late binding survives trait dispatch (ADR-0066 Decision 1: "Consumers may hold the handle before any real collector exists"; metrics.rs:95-110). Task-authoring precision item only (see Notes n1). (`context.rs:53,655`; `docs/adr/0066…md` Decision 1)
2. [sharpen] Wording tension only. Semantics unambiguous: never abort, retry per tick; implementer memoizes successful init (lazy `Option<Mibs>`). No contract break. (design.md:118-123)
3. [scenario] Yes. `TARGET_FNS` = `record_counter`, `record_histogram`, `record_component_operation` (lint_metric_labels.rs:32-34); `set_queue_depth` is not scrutinized today (seda passes `format!` labels into it, seda lib.rs:526). Dedicated methods are outside the lint's scope; proposal's "lint-metric-labels-clean by construction" holds. (`scripts/xtask/src/lint_metric_labels.rs:32-34`)
4. [cross-ref] All verified: `PINNED_CLIENT_TTL = from_secs(60)`, `PINNED_CLIENT_MAX_ENTRIES = 64` (client_cache.rs:19,23); moka 0.12.16 `entry_count()` exists (moka sync/cache.rs:671); seda sampler shape `tokio::spawn` + `interval` loop (camel-component-seda lib.rs:509-529, design cites 512-529 — accurate window); queue-depth GaugeVec precedent families.rs:149-156; jemalloc `stats` feature camel-cli Cargo.toml; tikv-jemallocator 0.7 (ctl version-match rule in D4 is satisfiable); bd rc-u4qz/rc-0sxi/rc-nkzb/rc-vnm8 all exist with matching intent. (`client_cache.rs:19,23`; `Cargo.toml:187`)

**Outcome:** confirm (all four outcomes; no refine/merge/split/drop/open-question).
**Self-grill mode:** self-grill-proposals skill

## Non-blocking notes for STAGE 2 (task authoring)

- **n1 (D2 type precision)**: the cache field will concretely be the `Arc<dyn MetricsCollector>`
  returned by `ctx.metrics()` (handle-wrapped) or require a new `Arc<MetricsHandle>`
  accessor in camel-core; either satisfies D2's intent. Task blocks should name the
  concrete choice.
- **n2 (D4 wording)**: "MIBs initialized once before the loop" + init-failure retry →
  implement as memoized lazy init; task block should say so.
- **n3 (bd sync)**: rc-0sxi description lists three stats and a 60 s cycle; the blessed
  design has four stats (adds `mapped`) and a 5 s run-command sampler. Update the bd
  description at apply time to prevent future drift flags.
- **n4 (hash recipe)**: submitter should record the bless-hash recipe (or re-pin) so the
  next re-bless can verify integrity mechanically.

## Verdict

**BLESS.** F1-F8 all landed and are internally consistent across proposal/design/spec;
no new defects introduced; signatures agree across D2/D3/D5; phase exit criteria are
coherent and testable. Notes n1-n4 are task-authoring/process guidance, not artifact
defects.

## Amendment 2026-08-29 (round-2 r_glm)

design.md D2 rewritten post-bless: cache wiring moved from constructor
to create_endpoint-time wire() with OnceLock (constructors are runtime-less).
New design.md sha256 prefix: b0179552801869c9
Superseded by the plan-bless whole-dir hash.

## Round 3 — plan-blessing gate (expert, supersedes spec blessing)

- Verdict: **BLESS-WITH-FIXES → BLESS** (fixes P1-P7 applied in-gate, re-verified)
- Skill: self-grill-proposals; artifacts: proposal.md, design.md,
  specs/component-metrics-emission/spec.md, tasks.md — all read in full
- Post-fix hashes (this gate; recipe below):
  - proposal.md `cd778d26fb3b1e06…` (unchanged since round 1)
  - design.md `9c09c3a0e1abc5ea…`
  - spec.md `93c8f368bd4f5750…` (unchanged since round 1)
  - tasks.md `4da257a49d688e80…`
  - concat4 (proposal‖design‖spec‖tasks, that order, no separators):
    `sha256:938467e640a9fcba6b8d307402b7473b59d2513dd03cf568ddaf5f8e49989c45`
  - Recipe: `python3 -c "import hashlib;from pathlib import Path;ps=['proposal.md','design.md','specs/component-metrics-emission/spec.md','tasks.md'];h=hashlib.sha256();[h.update(Path(p).read_bytes()) for p in ps];print(h.hexdigest())"`
    run in this change directory. Submitter's earlier `sha256:eeb60713…` was
    computed before round-3 fixes and its recipe is undocumented (n4 resolved
    here by pinning this recipe).

### Fixes applied (P1-P7)

| Fix | Artifact | Change | Evidence |
|---|---|---|---|
| P1 | design.md D2 | `lib.rs:1921` → `lib.rs:1922` | round-2 fix landed in tasks.md only; actual `fn create_endpoint` (HttpComponent) at lib.rs:1922, HttpsComponent at 2003 (rg-verified) |
| P2 | design.md Context | `Cargo.toml:94–96` → `Cargo.toml:97` | `tikv-jemallocator = { … features = ["stats"] }` at camel-cli Cargo.toml:97 (comment block 88–96) |
| P3 | tasks 1.3 step 3 | parenthetical: `self.cache.entry_count()` is moka's public method, NOT the `#[cfg(test)]` wrapper | wrapper `entry_count` is cfg(test)-gated (client_cache.rs:98–99); `self.cache` is the moka field (get_or_build uses `self.cache.get_with`); a worker shortcut to `self.entry_count()` compiles under `cargo test` but breaks non-test clippy/build |
| P4 | tasks 1.3 step 5 | ssrf tests 616/661 "construct through component paths" → "construct the cache directly, unwired" | ssrf.rs:616/:661 call `PinnedClientCache::new(...)` directly (verified); conclusion (no change) unchanged |
| P5 | tasks 1.1/1.2/2.1 acceptance | added `cargo fmt --check --all` | keeps every commit fmt-green (was only in 1.3/2.2) |
| P6 | tasks 2.2 step 1 + acceptance | `Cargo.toml:138` → `:140`; added `cargo clippy -p camel-cli --features jemalloc -- -D warnings` | feature at Cargo.toml:140; CI's clippy runs camel-cli with default features only — feature-gated sampler otherwise never clippy-checked |
| P7 | tasks 2.2 real-read test | reworded `expected:` — a dropped epoch advance yields Ok-but-stale, so the test cannot red-flag ordering; epoch-first is enforced by the D4 read shape + review | jemalloc epoch semantics; spec scenario clause remains covered by D4's mandated implementation shape |

### Self-grill record (plan-level)

**Questions generated:**
1. [glossary] Do plan terms ("wired collector", "recording double", "seam") conflict
   with ADR-0066 / CONTEXT-MAP glossary?
2. [sharpen] Which cited anchors are fuzzy or stale ("run.rs:600 area",
   "Cargo.toml:138", "lib.rs:1921", "94–96")?
3. [scenario] Construct worker misreads that become defects: `self.entry_count()`
   vs `self.cache.entry_count()`; dropped epoch advance; `grep -c` exit-code
   inversion; fmt-drifted intermediate commits.
4. [cross-ref] Are all NEW-symbol signatures identical across design/tasks, all
   cited file paths real, and all external crate assumptions true?

**Answers (with citations):**
1. [glossary] Consistent: "wired" tracks ADR-0066 late-bound-handle language and
   the round-1 record Q1; "seam" matches D5. No CONTEXT-MAP conflicts found.
   (`docs/adr/0066…`, bless-record.md:33-39)
2. [sharpen] Four stale/fuzzy refs found → P1, P2, P6. All other anchors
   verified exact: metrics.rs:22/53/67/110/161/247, families.rs:149–156,
   mod.rs:144/167, context.rs:53/:531/:658, client_cache.rs:50/69–82/234,
   lib.rs:2252–2256, ssrf.rs:389, main.rs:4–9, run.rs:600, seda lib.rs:512–529,
   lint_metric_labels.rs:123–133 (rg-verified this gate).
3. [scenario] (a) `self.entry_count()` compiles in test, breaks non-test build —
   closed by P3 parenthetical; (b) dropped epoch advance passes the real-read
   test — closed by P7 rewording; (c) `grep -c` prints 0 and exits 1 on the
   correct path — tasks already assert on stdout ("prints `0`"), left as-is;
   (d) fmt drift at 1.1/1.2/2.1 commits — closed by P5.
4. [cross-ref] Signatures identical across surfaces for all NEW symbols
   (`set_pinned_client_cache_size`/`_hit`/`_miss`, `set_allocator_memory`,
   `AllocatorStat`, `HttpComponentKind`, `wire`, `AllocatorSnapshot`,
   `emit_allocator_snapshot`, four family names). Package name is
   `camel-component-http` (Cargo.toml:2) — `-p` flags correct. `ctx.metrics()`
   returns exactly `Arc<dyn MetricsCollector>` (context.rs:658) — `wire(kind,
   ctx.metrics())` type-checks verbatim. External: tikv-jemalloc-ctl 0.7.0
   `stats` module compiles in docs.rs default-feature build (module ungated;
   the crate's `stats` feature only toggles sys/stats, already enabled via
   jemallocator's `features=["stats"]`), so `features = ["use_std"]` is
   sufficient. lint-metric-labels TARGET_FNS is a static list not including the
   new methods; lint-log-levels regex targets only `error!(` — the planned
   `warn!` needs no `log-policy` annotation. TDD: every task test precedes impl
   with an honest red condition or an explicit guard rationale; traceability:
   all six spec scenarios map to owning tests (steady→warm_key; one-miss→
   single_flight; label-distinction→as_str-image + 1.2 export; four-gauges→
   ok_snapshot + real_read; warn-retry→err_read + ignore-bool loop; no-feature→
   cargo tree + cfg gates). Phase boundary matches design ## Phases exactly.

**Outcome:** refine (P1-P7); all applied and re-verified in-gate.
**Self-grill mode:** self-grill-proposals skill

**Final verdict: BLESS.** The plan is executable as written: every command runs
in the worktree, every anchor is now exact, both prior rounds' fixes landed
coherently, and the phase exit criteria are testable.
