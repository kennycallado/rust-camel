# Design: clippy-all-targets

## Measurement (deliverable 1)

Base: local `main` = `70a696ae2d01e35b53fc7a47f6b4351534cd80a8`. 183 workspace members.
Environment: rustc/cargo 1.99.1, 12 cores, sccache 0.15 (`/home/shared/sccache`), worktree
`target/` on ext4, runs contained in `systemd-run --user --scope` with `MemoryMax=12G`,
`TMPDIR=/tmp`, foreground.

| Run | Command (relative flags) | Result | Wall clock |
|---|---|---|---|
| A | `--workspace --all-features --all-targets -- -D warnings` (literal, no excludes) | exit 101 — incomplete survey: `sasl2-sys` build script panics: "Unable to find libsasl2" (chain: `sasl2-sys` ← `rdkafka-sys` ← `rdkafka` ← `camel-component-kafka`, activated by `--all-features`). 41 of 183 workspace members linted before the abort; those 41 were clean. | 212.5 s to abort |
| D | `--workspace --all-features` + 4 excludes + `-D warnings` (current gate line) | exit 0 | 0.670 s steady-state (warm, no sccache); 7.3 s first warm run with sccache |
| E | D + `--all-targets`, pre-fix | exit 101 — exactly 1 failing crate (see table below) | 47.8 s first full run (compiles all test targets) |
| E2 | E + the one-line rename fix | exit 0 | 15.5 s (warm local target, no sccache) |
| E3 | E2 re-run (steady-state) | exit 0 | 1.166 s |
| D3 | D re-run (steady-state) | exit 0 | 0.670 s |

Per-crate table (crate → failing targets → lint names), pre-fix, for the four-exclusion
selection (`--workspace --all-features --exclude camel-cli --exclude camel-component-kafka
--exclude security-keycloak --exclude security-wasm-policy --all-targets`), which is the
selection option (a) widens:

| Crate | Failing target | Lint | Count |
|---|---|---|---|
| `camel-master` | test `master_component` | `deprecated` (rustc lint, not clippy): `AtomicUsize::fetch_update` renamed to `try_update` | 1 |

All other members' lib/bin/test/bench/example targets: green. `camel-master` has four
workspace dependents whose targets cargo skipped in run E after the failure
(`camel-bundles`, `camel-test`, `example-master-leader`, `example-master-leader-yaml`);
the green post-fix run E2/E3 linted them and they pass.

Context: the two defects cited in bd `rc-e6kz2` are already retired on main —
`camel-test` `len_zero` fixed by `6da6a1439` ("use is_empty in the batch-bind
assertions"), and the StreamList unbounded wait bounded earlier. Note the StreamList
defect was found by the `cargo xtask lint-unbounded-wait` gate, a separate mechanism;
widening clippy would not have caught it. The residual clippy-visible test-target debt
is the single `deprecated` lint above.

Measurement incidents worth recording: two runs were lost to the documented sccache
"Failed to create temp dir" failure mode (server started inside the systemd scope
inherits a per-call tmpdir that the scope teardown/deletion invalidates). The ordered
remedy (`sccache --stop-server`, retry) was applied; final survey runs unset
`RUSTC_WRAPPER` for determinism and rely on the local `target/` fingerprints, which
reproduced green at steady state twice.

## Decision (deliverable 2): option (a) — add `--all-targets` to the workspace line

The exact new invocation:

```
cargo clippy --workspace --all-features \
  --exclude camel-cli \
  --exclude camel-component-kafka \
  --exclude security-keycloak \
  --exclude security-wasm-policy \
  --all-targets \
  -- -D warnings
```

(the four excludes are carried over unchanged from the current registry line)

**Cost.** Two distinct numbers, measured locally (12 cores, logs under
`target/survey/`: `runA-full.log`, `runD-baseline.log`, `runE-alltargets.log`,
`runE2-postfix.log`, `runE3-steady.log`, `runD3-steady.log`; timing via bash
`TIMEFORMAT` around each invocation inside the systemd scope):

- Steady-state, local, unchanged tree, warm local `target/` fingerprints, sccache
  disabled for determinism: 0.670 s (current gate line) → 1.166 s (with
  `--all-targets`). Delta ≈ 0.5 s per run.
- One-time cost to lint all test targets for the first time: 47.8 s wall (dependency
  caches warm via sccache). CI's clippy job starts from a cold-ish cache: expect the
  first CI run after adoption to pay minutes for test-target compilation.

**Unmeasured**: per-push CI overhead when code changed — CI re-lints affected crates
and their dependents, and with `--all-targets` their test targets too, which today's
gate does not compile. That number is not measured here and is not claimed; the only
measured figures are the two local ones above. Option (a) as literally worded in the
order — the no-excludes full flag — is infeasible on this machine (libsasl2) and would
tie gate success to a native dev package no other gate needs; keeping the existing four
excludes preserves today's feasibility exactly while widening coverage everywhere else.

**Coverage.** 179 of 183 members have all targets (unit tests via `#[cfg(test)]`,
`tests/` targets) linted, up from lib/bin only. The four excluded crates stay excluded
from the all-features line for pre-existing structural reasons (kafka's all-features
build needs native libsasl2; security-keycloak and security-wasm-policy — and any
examples depending on them — remain outside this line as they are today).
`camel-component-kafka` already has its own `--all-targets -- -D warnings` line
(default features) covering its test targets. Correcting a draft claim: only the
`flavor-regular,exec` camel-cli variant carries `--all-targets` today; the default
`cargo clippy -p camel-cli -- -D warnings` line does not, and this change does not add
it — camel-cli's test targets remain uncovered, same as today.

**Enforcement boundary.** CI hard-codes its own copy of the clippy gate
(`.github/workflows/ci.yml:394-405`); this mission order prohibits touching
`.github/workflows/**`. The AGENTS.md edit therefore takes effect immediately for local
sweeps (`scripts/gates-local.sh` parses the registry) and for the conductor's merge
gate, but CI keeps the old line until a workflow edit lands. bd follow-up filed for the
CI sync so the change is not silently half-enforced.

**Regression behaviour.** With option (a) there is no mutable state: any crate in the
widened selection that regresses turns the gate red on the next local sweep and at the
conductor's merge gate, blocking the merge until fixed. CI blocking awaits the workflow
sync (rc-2s1vm); the four excluded crates are outside this line, as today. With
option (b) — a per-crate opt-in list — a fixed crate stays on the exclusion list until
someone remembers to remove it; the list silently stops covering what it once covered,
and each excluded crate needs a bd issue with exact commands and lint lists. The owner
values gates that cannot drift; a hard-coded list is justified only when the fixes it
defers are large. They are not: the survey found one mechanical rename. Option (b)
buys deferral of a one-line fix at the price of permanent bookkeeping — rejected.
Option (c) keeps the hole that let `clippy::len_zero` reach main as test-only code
open and relied on "cover it another way in CI" with no concrete mechanism — rejected.

**Fix bound.** Option (a) requires one code fix; the order caps this at "~15 small
mechanical fixes" before falling back to (b). We are at 1. The fix is a pure rename
(`fetch_update` → `try_update`, identical signature, rustc's own deprecation note
prescribes it), test-only, no `#[allow]`, no behaviour change — within bounds.

## skip_specs

No spec canon owns the clippy gate registry: `openspec/specs/gate-forwarding-lint/`
specifies the `lint-gate-forwarding` xtask (bundle gate forwarding), and
`ci-compile-gate` references the `--all-features` clippy gate only in a comment about a
CI compile step. `AGENTS.md` is the registry's home. Proposed: `skip_specs: true`.
e_gpt rules at the spec bless.

## gates-local.sh

No change needed: the script parses the QUALITY GATES registry from `AGENTS.md`, so the
edited line is picked up automatically. Verification includes running the sweep and the
exact new invocation.

## Affected files

- `AGENTS.md` — one line in the clippy gate entry (`--all-targets` added).
- `crates/components/camel-master/tests/master_component.rs` — the one-line rename.
- `openspec/changes/clippy-all-targets/**` — artifacts, `.review.json`.

bd: `rc-e6kz2`.
