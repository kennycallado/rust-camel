# Adoption decision — cargo-fuzz and cargo-mutants

**Status:** Decided (architect final word). **Date:** 2026-08-31.
**Companion to:** `docs/audits/2026-08-31-security-audit.md` (R1).
**Scope:** how rust-camel adopts coverage-guided fuzzing and mutation testing so both add
maximal assurance and neither obstructs the developer loop or worsens the disk/CI budget.

---

## 0. Verdict (one line each)

- **cargo-fuzz — ADOPT NOW.** It closes R1, the single biggest security gap, on parsers of
  genuinely untrusted input; the audit already proves those parsers are the attack surface.
- **cargo-mutants — ADOPT SCOPED, INFORMATIONAL, SECOND.** Worthwhile only as a periodic
  *test-quality probe* on ~5 security-critical modules; never a gate, never whole-workspace.
  It verifies the audit's dense regression tests actually kill mutants, then it goes dormant.

**Sequence:** fuzz first (weeks 1–2), mutants second (week 3+), because fuzzing produces new
regression corpora that *are themselves* the tests mutation testing later grades. Doing
mutants first grades a test suite that fuzzing is about to improve — wasted signal.

---

## 1. Why this order, and is mutation testing worth it here?

The codebase already has **dense adversarial tests** (563/699/832/634… per crate, one per
fix). That fact cuts both ways:

- It **lowers** cargo-fuzz's redundancy risk — fuzzers find *unknown* inputs, which no amount
  of hand-written adversarial tests covers. High ROI, uncorrelated with existing tests.
- It **raises** the bar for cargo-mutants ROI — a suite this dense will already kill most
  mutants, so a whole-crate run (30–60 h) mostly re-confirms what we know while burning the
  disk/CI budget. Low marginal ROI at crate scope.

Therefore mutation testing earns its keep **only** as a narrow, occasional audit that answers
one question: *"Do the security regression tests actually pin the security invariant, or do
they pass vacuously?"* That is answerable in minutes on 5 modules, not hours on a workspace.
Run scoped, it is high-value; run broad, it rots. We run it scoped.

---

## 2. The disk trap and how we neutralize it (read first)

Two hard facts from the repo make the naive setup dangerous:

1. `workspace.members` uses **globs** (`crates/camel-*`, `crates/components/*`, …). A
   `fuzz/` crate dropped inside a member crate gets **swept into the workspace**, pollutes
   the shared `Cargo.lock`, and compiles under the shared `./target`.
2. `.cargo/config.toml` sets `incremental = false` and **no `CARGO_TARGET_DIR`** — everything
   lands in the default `./target` (the 75 GB one that must stay cold in main).

Both tools compile the world **again** under their own instrumentation (fuzz: ASan +
libFuzzer; mutants: N rebuilds). Left unconfined they would create second and third 20–40 GB
target trees. **Non-negotiable mitigations:**

- **Every fuzz/mutants build uses a dedicated, git-ignored, worktree-local target dir** via
  `CARGO_TARGET_DIR`, never the default `./target`. The xtask wrappers set it. See §5.
- The single top-level **`fuzz/` crate is `workspace.exclude`d** (not a member) so it never
  touches the production lockfile or the default target. cargo-fuzz's own nested workspace is
  kept, but we belt-and-suspenders it with an explicit exclude entry because of the globs.
- Corpora and crash artifacts live **outside the repo tree** by default (scratch under
  `/tmp` / `$XDG_CACHE_HOME`); only **minimized** crash reproducers get promoted into the
  tree as regression fixtures. The bulk corpus is never committed.
- Both wrappers **refuse to run in the main checkout** (path guard) — same rule as cargo
  build/test. All fuzz/mutants work happens in a worktree.

Disk budget with mitigations: one extra instrumented target tree, **~8–15 GB, git-ignored,
purgeable, worktree-local**. The cold main `./target` is never touched.

---

## 3. Fuzzing strategy

### 3.1 Targets, ranked by exposure to *untrusted* input

Ranking principle: untrusted = attacker can supply the bytes at runtime (headers, request
bodies, remote route pushes), not merely operator config. Operator-config parsers rank lower
but still fuzz — defense in depth against a compromised control plane.

| Rank | Target | Entry point | Trust level | Why |
|---|---|---|---|---|
| 1 | DSL YAML route parse | `camel-dsl::yaml::parse_yaml_with_threshold_and_security` | control-plane, remote-pushable | Largest grammar; noyalib budgets are the last line — fuzz them |
| 2 | DSL JSON route parse | `camel-dsl::json::parse_json_with_threshold_and_security` | control-plane | serde_json recursion caps + canonical lowering |
| 3 | `${env:}` interpolation | `camel-dsl::env_interpolation::interpolate_env` | operator + templated | Audit-verified single-pass; fuzz confirms no residual `${`/expansion bomb |
| 4 | Simple expression | `camel-language-simple::parser` (+ `evaluator`) | **runtime, header/body-derived** | Evaluated against message data — genuinely untrusted; "zero regex" claim must hold under fuzzing |
| 5 | SSRF IP classification | `camel-api::ssrf` (classifier over parsed IPs) | runtime (redirect Location, header URI override) | NAT64 recursion + literal normalization are subtle; property-style fuzz |
| 6 | Per-component URI parse | `parse_uri` per component (http/mqtt/jms first) | operator, some header-override | Redaction + validation live here (F3-1/F5-x) |

**Order of implementation:** 1 → 2 → 3 → 4 first (the pilot is #1 alone; #2–#4 follow once
the harness pattern is proven). #5–#6 are phase 3.

### 3.2 Harness design

- One top-level **`fuzz/` crate** (`fuzz/Cargo.toml`, `workspace.exclude`d), `cargo-fuzz`
  layout, one `fuzz_targets/*.rs` per target above.
- Each target is a thin `fuzz_target!(|data: &[u8]| { ... })` that feeds bytes to the entry
  point and asserts **no panic / no unbounded resource use** (the parsers already return
  `Result`, so the invariant is "never panic, always terminate under the documented budget").
- Use `arbitrary` (already in `Cargo.lock`, zero new top-level dep) for the structured
  targets (#5 SSRF: derive `Arbitrary` for an IP-ish input); raw `&[u8]` → `str::from_utf8`
  for the text parsers (#1–#4, #6).
- **Seed corpora** from existing test fixtures: every adversarial test input from the audit
  (absolute-path headers, `../` payloads, decimal/hex IP literals, alias-bomb YAML) is
  copied in as a seed. This front-loads coverage massively.

### 3.3 Corpus + artifact management

- **Corpus lives outside the tree.** Default location `$CARGO_TARGET_DIR/../fuzz-corpus/<target>/`
  (worktree-local, git-ignored). Growth is a developer/CI concern, not a repo concern.
- **Crash artifacts:** on a find, `cargo fuzz tmin` (minimize) → the minimized reproducer is
  committed as a **normal regression test** in the owning crate (`tests/regressions/`), *not*
  as a raw fuzz artifact. The crash file itself never enters the repo. This converts every
  finding into a permanent unit test and keeps the repo binary-clean.
- **No corpus is committed** in phase 1–2. If corpus sharing becomes valuable later, store a
  *minimized seed corpus* (small, text) per target — decided at that point, not now.

### 3.4 CI integration model — both, but bounded

- **Per-PR (short, non-blocking budget):** a `fuzz-smoke` job runs each target for a **fixed
  wall-clock 60 s** (`-max_total_time=60`) seeded from the committed regression fixtures.
  Purpose: catch a *newly introduced* trivially-crashing input on the changed parser. It is
  **`continue-on-error: true`** — a find annotates the PR, it does not red-X the merge (§6).
- **Nightly (long, non-blocking):** a scheduled workflow runs each target **10–15 min**,
  restoring the prior nightly corpus from a CI cache key (not the repo). A find opens a bd
  issue automatically (§6), never blocks anything (nothing is gated on nightly).
- **Not on every push locally.** Developers run `cargo xtask fuzz <target>` on demand.

CI cost control: the smoke job is one extra ~5-min job (build dominates, 60 s×N run is
cheap) and reuses `Swatinem/rust-cache`. The nightly job is off the PR critical path
entirely. Given the "avoid heavy CI" constraint, **smoke is opt-in per-PR via a path filter**
— it only runs when files under the fuzzed crates change.

---

## 4. Mutation testing strategy

### 4.1 Scope: module-scoped, never crate-scoped

Target set (exactly the audit's security-critical surface):

- `camel-api/src/ssrf.rs`
- `camel-api/src/endpoint_uri.rs::redact_value` and the `redact_*` family across
  `camel-config`, `camel-http`, `camel-jms`, `camel-mqtt`, `camel-component-surrealdb`
- `camel-file` `validate_relative_filename` + `validate_path_is_within_base`
- `camel-processor` aggregator/resequencer **limit enforcement** (`max_bucket_size`,
  `max_buckets`, `DEFAULT_MAX_*` checks)
- `camel-core/src/claim_check/memory_repository.rs` depth cap

`cargo-mutants` supports file/regex scoping (`--file`, `--in-diff`). A `.cargo/mutants.toml`
(or per-run flags via the xtask) pins `examine_globs` to exactly these files. **Whole-crate
runs are forbidden** by policy — they are the 30–60 h trap.

### 4.2 Local-first, CI-optional, never gated

- **Primary mode: on-demand local** via `cargo xtask mutants [--diff]`. A developer touching
  `ssrf.rs` runs `cargo xtask mutants --file crates/camel-api/src/ssrf.rs` and reads the
  surviving-mutant list as a **test-gap report**. Minutes, not hours.
- **`--in-diff` mode for PRs (informational):** an optional nightly/weekly job runs
  `cargo mutants --in-diff <pr-range>` over *only the changed security files*. Output is a
  **report artifact + bd issue on new survivors**, never a gate.
- **No mutation-score threshold gate. Not hard, not ratchet.** Rationale: a numeric gate on a
  glob'd file set invites gaming (delete the file from the glob) and produces flaky failures
  from equivalent mutants. The value is the **surviving-mutant list a human reads**, not a
  number a bot enforces. If a survivor reveals a real gap → write the test (that is the whole
  point). This keeps mutants from ever blocking a merge.

### 4.3 Cadence

Mutation testing is **event-driven, not continuous**: run it (a) once now as the pilot
baseline, (b) whenever a security-critical module changes, (c) after any future security
audit. Between those, it stays dormant. This is why it "won't rot" — it is a tool you *pull*,
not a gate that *pushes*, so there is no red-X to disable-and-forget.

---

## 5. Infrastructure wiring (concrete)

All paths relative to a **worktree root**, never main.

### 5.1 New xtask subcommands (extend the `main.rs` string-match dispatch)

- `cargo xtask fuzz <target> [--time N] [--nightly]`
  - Guards: refuse if `$PWD` resolves to the main checkout (reuse the worktree-detection the
    project already relies on); refuse if `cargo-fuzz` absent (print install hint).
  - Sets `CARGO_TARGET_DIR=<worktree>/target-fuzz` (git-ignored), corpus dir under
    `<worktree>/target-fuzz/corpus/<target>`.
  - Wraps `cargo +nightly fuzz run <target> -- -max_total_time=${N:-60}`.
  - On crash: runs `cargo fuzz tmin`, prints the minimized input path and a copy-paste
    snippet for a regression test.
- `cargo xtask mutants [--file P | --diff] [--json]`
  - Guards: main-checkout refusal; `cargo-mutants` presence check.
  - Sets `CARGO_TARGET_DIR=<worktree>/target-mutants` (git-ignored).
  - Defaults `examine_globs` to the §4.1 security set; `--diff` switches to `--in-diff`.
  - Emits the surviving-mutant list; `--json` for bd-issue automation.

Both subcommands are **not** added to the `QUALITY GATES` block. They are developer/CI tools,
not gates.

### 5.2 Files added

- `fuzz/Cargo.toml` — `cargo-fuzz` crate, **listed in `workspace.exclude`** in root
  `Cargo.toml` (defends against the member globs).
- `fuzz/fuzz_targets/{dsl_yaml,dsl_json,env_interp,simple_expr}.rs` (pilot: `dsl_yaml` only).
- `.cargo/mutants.toml` — pins `examine_globs` to the security set; `timeout_multiplier`
  tuned so slow security checks don't false-timeout.
- `.gitignore` additions: `/target-fuzz`, `/target-mutants`, `**/*.fuzz_crash` (crashes are
  promoted as tests, never committed raw).
- `.github/workflows/fuzz-smoke.yml` — path-filtered, 60 s/target, `continue-on-error`.
- `.github/workflows/nightly-assurance.yml` — schedule; long fuzz + scoped `mutants --in-diff`
  on `main`; both open bd issues on findings, neither gates.

### 5.3 Worktree / cold-main respect

- xtask wrappers hard-refuse in main (path guard). All instrumented builds are worktree-local
  under `target-fuzz/` / `target-mutants/`, both git-ignored and purgeable. The shared 75 GB
  `./target` in main is never read or written by either tool.

---

## 6. Anti-obstruction guarantees (explicit, binding)

1. **No fuzz or mutants result is a merge gate.** They are absent from the `QUALITY GATES`
   block. PR-time fuzz smoke is `continue-on-error: true`; mutation testing never runs on the
   blocking PR path.
2. **Findings become bd issues, not red X's.** Nightly fuzz crash → `bd create -t bug -p 1
   --deps discovered-from:<audit>`; new mutation survivor → `bd create -t task -p 2`. The
   pipeline files the issue and stays green.
3. **Crash artifacts are minimized and committed as regression tests**, in the owning crate's
   test tree, in normal Rust — never as raw binary corpus blobs. This is the *only* thing
   fuzzing adds to the repo.
4. **No corpus in the repo** (phase 1–2). Corpora are worktree-local/CI-cached scratch.
5. **No mutation-score number is enforced anywhere.** The output is a human-read gap list.
6. **Developer loop untouched:** `cargo build`/`test`/`clippy` behavior, timings, and the
   default `./target` are unchanged. Both tools are opt-in commands.

---

## 7. Phased rollout

### Phase 1 — Pilot (1 week, fuzz only, `dsl_yaml`)

- Add `fuzz/` crate (excluded), `xtask fuzz`, `.gitignore` entries, seed corpus from existing
  DSL adversarial fixtures, the `fuzz-smoke.yml` path-filtered job (smoke on `dsl_yaml` only).
- **Success criteria:** (a) `cargo xtask fuzz dsl_yaml --time 300` runs clean in a worktree,
  writes only to `target-fuzz/`, main `./target` untouched (verify mtime); (b) an
  *intentional* injected panic is caught and `tmin`-minimized into a committed regression
  test in <10 min end-to-end; (c) CI smoke job adds <6 min and is non-blocking; (d) zero new
  entries in the production `Cargo.lock`.
- **Fail/stop criteria:** any second full target tree appears under the default `./target`;
  smoke job blocks a merge; production lockfile churns.

### Phase 2 — Fuzz expansion (week 2)

- Add targets `dsl_json`, `env_interp`, `simple_expr` (ranks 2–4). Wire `nightly-assurance.yml`
  (10–15 min/target, corpus from CI cache, bd-issue on find).
- **Expansion criterion:** phase 1 met all success criteria and produced ≥1 real or seeded
  regression test. Any real crash found here is triaged as a security bd issue immediately.

### Phase 3 — Mutation pilot + remaining fuzz targets (week 3)

- Add `xtask mutants`, `.cargo/mutants.toml` scoped to the §4.1 set. Run the baseline once;
  file the surviving-mutant list as bd tasks. Add fuzz ranks 5–6 (`ssrf`, per-component URI).
- **Success criteria:** scoped mutants run over the 5 modules completes in **<15 min total**;
  produces a concrete, actionable survivor list; adds nothing to gates; no target-tree bloat
  outside `target-mutants/`.
- **Expansion criterion (to keep mutants in rotation):** the baseline finds ≥1 *actionable*
  survivor (a real test gap). If it finds none, mutants is confirmed low-ROI here → keep the
  xtask for on-demand use after future audits, drop the nightly `--in-diff` job.

---

## 8. What NOT to do (traps)

- **Do NOT make either tool a CI merge gate.** Non-negotiable; it violates the anti-obstruction
  charter and will be disabled-and-forgotten.
- **Do NOT run `cargo mutants` at crate or workspace scope.** That is the 30–60 h trap the team
  already identified. Module globs only.
- **Do NOT let the `fuzz/` crate be a workspace member.** The `crates/*` globs *will* try to
  sweep a nested `fuzz/`; the explicit `workspace.exclude` is mandatory.
- **Do NOT write instrumented builds into the default `./target`.** Always `CARGO_TARGET_DIR`
  to a worktree-local, git-ignored dir. This is the single most important disk rule.
- **Do NOT commit raw corpora or crash blobs.** Commit *minimized reproducers as Rust tests*.
- **Do NOT set a mutation-score threshold.** It gets gamed via the glob and produces flaky
  equivalent-mutant failures.
- **Do NOT fuzz the JS/WASM/exec sandboxes for OOM** — the audit already documents Boa heap
  amplification (F3-2) as an accepted residual with no API to bound it; a fuzzer will just
  rediscover a known, unfixable limitation and waste budget.
- **Do NOT add `proptest`** as a parallel track now — it is not in the lockfile, overlaps the
  fuzz targets, and adds a dep. Revisit only if a *stateful* invariant (not a parser) needs it.

## 9. Budgets (estimated)

- **Disk:** +8–15 GB, worktree-local, git-ignored, purgeable (`target-fuzz` + `target-mutants`
  combined). Main `./target`: **0 change**.
- **Developer time:** `xtask fuzz <t> --time 60` ≈ 1 build + 60 s. Scoped `xtask mutants` over
  5 modules ≈ **<15 min** (a few hundred mutants × fast module tests).
- **CI time:** smoke job +≈5–6 min, path-filtered, non-blocking. Nightly ≈ 60–90 min total,
  off the critical path.
- **Whole-crate mutants (rejected):** 30–60 h — the number we are explicitly refusing.

## 10. The three highest-value first actions

1. **Wire `cargo xtask fuzz` + the excluded `fuzz/` crate + `dsl_yaml` target seeded from the
   existing DSL adversarial fixtures**, with `CARGO_TARGET_DIR=target-fuzz` and a
   main-checkout refusal guard. (Delivers R1's #1 target; proves the disk-safe pattern.)
2. **Add the path-filtered, `continue-on-error` `fuzz-smoke.yml`** (60 s/target) so fuzzing is
   visible on PRs without ever blocking a merge. (Locks in the anti-obstruction contract.)
3. **File the bd epic + phase tasks** (`bd create -t epic "Adopt cargo-fuzz + scoped
   cargo-mutants" --deps discovered-from:<security-audit>`), with the phase-3 mutants baseline
   as a task so it is scheduled, not gated. (Ensures mutants is *pulled*, not left to rot.)

---

## 11. Answers to the six questions (summary)

1. **Both, sequenced.** Fuzz first (high uncorrelated ROI, closes R1). Mutants second, scoped
   and informational — real ROI *only* as a periodic probe that the audit's dense tests kill
   mutants; worthless and rot-prone at crate scope, so we never run it that way.
2. **Fuzz:** targets ranked #1 DSL-YAML → #6 URI parsers by untrusted-input exposure; corpus
   worktree-local/CI-cached (never repo); CI = short non-blocking PR smoke **and** long
   nightly; crash → minimized regression test; disk contained via mandatory `CARGO_TARGET_DIR`
   + excluded `fuzz/` crate.
3. **Mutants:** module-scoped only; local-first + optional nightly `--in-diff`; **no score
   gate at all** (informational survivor list); event-driven cadence so it never blocks.
4. **Infra:** two xtask subcommands with main-checkout guards + worktree-local target dirs;
   `fuzz/` excluded from the member globs; `.cargo/mutants.toml`; two non-gating workflows.
5. **Anti-obstruction:** neither in `QUALITY GATES`; PR smoke `continue-on-error`; findings →
   bd issues; crashes → committed tests; no repo corpora; no enforced score.
6. **Rollout:** 1-week `dsl_yaml` fuzz pilot with disk/lockfile/non-blocking success criteria,
   then fuzz expansion, then a scoped mutants baseline that must find ≥1 actionable survivor
   to stay in nightly rotation.
