# Design: fuzz-smoke-ci

## Context

Phase 1 delivered the `fuzz/` crate, `dsl_yaml` target, seeds, and the
`cargo xtask fuzz` wrapper, verified locally up to the boundary of what
this machine can prove (no nightly, no cargo-fuzz). Four runtime drills
are recorded as `integration-verification-deferred-to-CI` in
`openspec/changes/archive/2026-09-01-fuzzing-mutation-tooling/verification.md`.
This change is the executor that closes them: a single new workflow
plus one wrapper fix in `scripts/xtask` (tmin artifact-prefix
forwarding, surfaced by this change's spec blessing round).

Binding sources: adoption decision 2026-08-31 §3.4 (smoke model), §5.2
(cost: job adds < 6 min), §6 (anti-obstruction); AGENTS.md QUALITY GATES
(fuzzing never enters the gate list); bd `rc-7rw2` + handoff notes
(audit `fuzz/Cargo.lock`; verify tmin output detection; close the four
deferrals).

## Goals / Non-Goals

**Goals**
- `fuzz-smoke.yml`: path-filtered, 60 s/target, `continue-on-error`, self-verifying.
- Execute all four deferred drills in CI, with closure evidence split
  honestly (see D1): three drills close on the introducing PR; the
  300 s full run closes on a post-merge dispatch; promotion closes on
  the first real crash.
- `cargo audit --file fuzz/Cargo.lock` coverage (handoff note).
- Job wall-clock < 6 min on the happy path (decision success criterion,
  measured on the introducing PR — not assumed).

**Non-Goals**
- Nightly long-run workflow + automated bd filing (decision assigns that
  to the nightly workflow — separate change).
- New fuzz targets (`dsl_json`, `env_interp`, `simple_expr` — later phases).
- Promotion automation: crash → committed regression test stays a human
  PR (decision §6.3); the drill verifies detection + minimization +
  instruction only.
- Mutation testing (rc-eba8).

## Decisions

### D1 — The workflow is self-verifying on its own PR

The path filter includes `.github/workflows/fuzz-smoke.yml` itself, so
the PR that introduces this change runs the workflow — three drills
(tmin, cold-main, refusal) execute before merge, on GitHub-hosted
nightly + cargo-fuzz. Closure evidence splits three ways:

- **PR evidence**: tmin drill, cold-main assertion, refusal drill —
  observed on the introducing PR's green run.
- **Post-merge evidence**: the deferred 300 s full run — a one-time
  `workflow_dispatch` with `time=300` after merge (the recurring PR
  smoke is 60 s by decision §3.4).
- **First-real-crash evidence**: promotion — the deferred wording's
  synthetic-panic promotion is superseded (the drill panic is the bug,
  not a promotable input); promotion closes when a real crash is
  triaged into a committed regression test per §6.3.

This change's verification.md records all three evidence slots.
`rc-7rw2` stays open until the human observes the PR run and the 300 s
dispatch (the human pushes; the conductor never does).

### D2 — CI runs the wrapper, never raw cargo-fuzz

CI invokes `cargo xtask fuzz dsl_yaml` exactly like a developer. Raw
`cargo fuzz run` would bypass the guards, seed staging, artifact
snapshot, and tmin — the entire Phase 1 surface under test. The GitHub
checkout is a main checkout (git-dir == common-dir), which the wrapper
refuses by design. Consequences, in order:

1. **Refusal drill is free**: run the wrapper in the checkout, assert
   exit 1 and the message `refusing: cargo xtask fuzz must run in a
   linked worktree` (closes deferred drill 4, the e2e refusal).
2. **Real runs execute from a linked worktree**: `git worktree add
   ../fuzz-wt HEAD` then run the wrapper with `--manifest-path`-free
   `current_dir = worktree`. Worktree-local `target-fuzz/` holds corpus,
   artifacts, and instrumented builds — nothing in the checkout.

### D3 — Drill mechanics (the four deferred criteria)

| # | Deferred criterion | CI step | Assertion |
|---|---|---|---|
| 4 | e2e main-checkout refusal | run wrapper in checkout with `CARGO_TARGET_DIR=$RUNNER_TEMP/xtask-refusal-target` (the xtask build itself must not create `./target`) | exit 1, guard message present |
| 1 | full run, only under `target-fuzz/` | `cargo xtask fuzz dsl_yaml --time ${{ inputs.time || 60 }}` in worktree (`inputs.time` is empty on `pull_request`, so the fallback yields the 60 s PR budget; the closure run is a `workflow_dispatch` with `time=300`) | exit 0; `target-fuzz/corpus/dsl_yaml/` populated; no crash artifact |
| 3 | main `./target` stays cold | checkpoint asserts | `test ! -d ./target` runs at six pinned checkpoints — baseline (b), after refusal (g), after smoke (i), after tmin (j), after audit (k2), and in the `if: always()` summary (l). Setup steps (c–f: toolchain/cache/install) run no cargo build in the checkout and restore no checkout target (`cache-targets: false`), so they cannot create `./target`; CI starts cold — absence is stronger than the local mtime probe |
| 2 | tmin: injected panic caught, minimized | temporary patch in the WORKTREE harness only (`panic!("fuzz-smoke drill")` appended to the target body), `--time 20` | exit ≠ 0; `new artifact(s):` and `minimized artifact:` in output; minimized file exists under `target-fuzz/artifacts/dsl_yaml/`; `fuzz/artifacts/` never created; promotion line printed |

The tmin drill is the first execution of `entries_created_after` /
`newest_file` against real cargo-fuzz output — exactly the naming risk
the Phase 1 holistic review flagged for Phase 2 verification. It depends
on the wrapper fix this change ships (see proposal): `minimize()` must
forward `-artifact_prefix` to `cargo fuzz tmin`, otherwise tmin writes
to cargo-fuzz's default `fuzz/artifacts/` and the wrapper's
`target-fuzz/artifacts/` scan finds nothing. The same fix caps each tmin
round at `-max_total_time=120` (total may span rounds; the job timeout
is the hard ceiling). tmin's exit status is treated as advisory:
cargo-fuzz 0.13.2's own post-minimization artifact scan reads
`fuzz/artifacts/<target>/`, which the wrapper's redirected layout never
creates, so tmin can exit non-zero after a successful minimization —
the wrapper decides success by the presence of a fresh
`minimized-from-*` artifact, not the exit code.

Criterion honesty (deferred wording vs CI execution):
- **300 s full run**: the recurring PR smoke is 60 s (decision §3.4
  binding). The deferred criterion's `--time 300` closes with a one-time
  post-merge `workflow_dispatch` run at `time=300`; this change's
  verification.md records that as the closure evidence step.
- **Promotion**: the deferred wording says the drill panic is
  "promoted to a committed regression test". A synthetic drill panic is
  the bug, not the input — promoting it is meaningless. The CI drill
  closes detection + minimization + instruction; the promotion criterion
  closes on the first REAL crash triage. Recorded explicitly, not
  silently weakened.

The worktree is discarded after the job; the panic injection can never
reach any ref.

Budget: 60 s smoke + 20 s drill + refusal (instant) + builds.

### D4 — Non-blocking contract covers the whole job

Job-level `continue-on-error: true`, including the fuzz-lock audit step.
Rationale: the epic binds "never blocks PR merge" for this workflow, and
the fuzz lockfile is a fresh 346-pkg dependency surface (libfuzzer-sys
toolchain) where advisory noise is expected until triaged. A real
advisory or crash annotates the PR and is triaged per §6.2 (`bd create
-t bug -p 1`); nothing auto-blocks. Root-lock `cargo audit` remains the
blocking gate in `ci.yml`, untouched.

### D5 — Toolchain, install, cache (honest cost model)

- `dtolnay/rust-toolchain` pinned SHA with `toolchain: nightly` (repo
  convention: pinned SHAs, no floating tags). Nightly installs each run
  (~30–60 s); toolchains are NOT cached (rust-cache does not cover
  `~/.rustup`).
- cargo-fuzz: `taiki-e/install-action` (the action family ci.yml already
  uses for cargo-audit) if its catalog carries cargo-fuzz; otherwise
  `cargo install cargo-fuzz --locked --version $CARGO_FUZZ_VERSION`.
  Either way the installed binary is restored via explicit
  `actions/cache` on `~/.cargo/bin`, keyed on the pinned
  `CARGO_FUZZ_VERSION` env var, so steady-state install cost is near
  zero. Worker verifies the catalog at implementation time.
- `Swatinem/rust-cache` with `cache-targets: false`: caches only the
  `~/.cargo` registry/git — nothing builds in the checkout (xtask builds
  run inside the worktree; the refusal drill redirects
  `CARGO_TARGET_DIR`), so a restored checkout `./target` would be both
  useless and a cold-main-assertion violation. The worktree
  `target-fuzz/` (instrumented build) is deliberately NOT cached —
  corpora and instrumented builds stay scratch (decision §6.4), and
  PR-to-PR hit rates would be low anyway.
- `ubuntu-latest` only, no OS matrix (cost control, §3.4); permissions
  `contents: read`; `timeout-minutes: 20` as the hard ceiling (raised
  from 10 after r_glm cold-path budget analysis: the introducing PR is
  a zero-cache run — nightly install + source-built cargo-fuzz + two
  cold xtask builds + cold instrumented build ≈ 650–850 s, and the
  300 s dispatch evidence run adds 240 s of fuzzing; 10 min would
  cancel both closure-evidence runs. The < 6 min criterion is a
  steady-state measurement, unaffected by the ceiling).

Cost expectation (to be MEASURED on the introducing PR, recorded in
verification.md): nightly install + cargo-fuzz restore + xtask build +
instrumented build (warm registry) + 80 s of runs ≈ 5–6 min steady
state. The decision's "< 6 min" criterion is evaluated on that
measurement, not assumed.

### D6 — Findings surface: annotation + summary

Job-level `continue-on-error` does NOT run later steps after a failed
step — so drill and audit steps NEVER fail themselves: each ends with
`exit 0` and appends `PASS/FAIL <name>` to `$RUNNER_TEMP/findings.md`
(all drills always run, evidence is complete). Infra steps
(checkout/toolchain/install) keep fail-fast semantics — their failure
is a broken workflow, not a finding. Expected non-zero exits (refusal,
panic drill) are NOT findings: a **finding** is exactly one of (a) a
drill assertion mismatch (guard message missing, minimized artifact
absent, `fuzz/artifacts/` created, checkout `./target` present,
`git status --porcelain` dirty), (b) the clean smoke run exiting
non-zero or producing a crash artifact, (c) a fuzz-lock audit advisory.
The final `summary` step (`if: always()`) re-asserts cold-main, then
integrity-checks findings.md against the expected outcome set
(`cold-baseline`, `refusal-drill`, `smoke`, `tmin-drill`,
`fuzz-lock-audit`, `cold-main-final`): a MISSING entry means an infra
step failed and downstream drills were skipped — the summary reports
"infrastructure failure" with the skipped list and exits 1 (an
infra-failed job must never render all-clear). On any `FAIL` entry it
writes to `$GITHUB_STEP_SUMMARY` — failing checks, artifact paths, the
§6.3 promotion instruction, the §6.2 triage command (`bd create -t bug
-p 1`) — then exits 1, which job-level `continue-on-error` renders as
an annotation, not a red X. All-clear requires all six entries present
and passing. No `gh`/bd
automation in this workflow — the decision assigns automated filing to
the nightly job.

## Risks / Trade-offs

- **Instrumented build cost per PR** (~2–4 min) on the filtered paths
  only; accepted by the decision's cost model (§5.2).
- **`created()` on the runner FS** (tmpfs/ext4): GitHub runners support
  btime; the `Ok(None)` diagnostic path covers degenerate ties (Phase 1
  holistic note N2). Drill asserts on the printed path.
- **audit noise** on the young fuzz lockfile → non-blocking by D4 until
  triaged; escalation to a gate is a one-line follow-up if the human
  wants it.

## Alternatives Considered

- **Raw `cargo fuzz` steps, no wrapper** — rejected (D2): bypasses every
  Phase 1 guard; drills 3/4 become untestable.
- **Blocking audit step** — rejected (D4): violates the epic contract
  before triage.
- **Matrix over targets** — YAGNI: one target exists (`KNOWN_TARGETS =
    ["dsl_yaml"]`); the loop shape arrives with target expansion.
- **Local-only verification (actionlint + shellcheck, no CI drills)** —
  insufficient: the four deferrals demand real nightly + cargo-fuzz
  execution; only CI provides it.

## Open Questions

None — the adoption decision pre-answers budget, blocking, and filing
policy. The only implementation-time check is D5's install-action
catalog detail.

## References

- docs/audits/2026-08-31-fuzzing-mutation-adoption-decision.md §3.4,
  §5.2, §6 — adoption decision (local disk; `docs/**` is gitignored in
  this repo — the bd epic `rc-4g9j` carries the same reference)
- openspec/changes/archive/2026-09-01-fuzzing-mutation-tooling/verification.md (deferred drills)
- openspec/specs/fuzz-tooling/spec.md — canonical requirement this
  change MODIFIES (Crash minimization and promotion)
- AGENTS.md QUALITY GATES (fuzzing absent — deliberately)
- ADR-0033/ADR-0038 — the DoS-cap defaults the dsl_yaml target defends
