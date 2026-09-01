# Verification: fuzzing-mutation-tooling

Task 3.1 record. Every command below ran in the
`fuzzing-mutation-tooling` worktree on 2026-09-01. Results are the real
outputs; nothing is fabricated.

## Guard-path check

| Check | Command | Result |
|-------|---------|--------|
| Toolchain guard fires before any build | `cargo run --package xtask -- fuzz dsl_yaml; echo "exit=$?"` | exit `1`; stderr: `error: cargo-fuzz or nightly toolchain missing — install with: rustup toolchain install nightly && cargo install cargo-fuzz` |

The worktree guard passed (this is a linked worktree), then the toolchain
guard fired. No fuzz build started. This verifies the toolchain-guard path
of the proposal's locally-verified acceptance criteria.

## Isolation static checks

| Check | Command | Result |
|-------|---------|--------|
| Root lockfile untouched | `git diff --exit-code Cargo.lock` | exit `0` (byte-identical) |
| No `camel-fuzz` workspace member | `cargo metadata --format-version 1 \| grep -c camel-fuzz` | stdout `0` (grep exits `1` on zero matches; the count is the criterion) |
| No `target-fuzz/` build output | `test ! -d target-fuzz \|\| ls target-fuzz` | exit `0`; directory absent (guard fired before build) |
| No fuzz entry in QUALITY GATES | `grep -A40 'QUALITY GATES' AGENTS.md \| grep -c 'fuzz'` | stdout `0` (grep exits `1` on zero matches) |
| `fuzz/target/` git-ignored | `git check-ignore -q fuzz/target && echo fuzz-target-ignored` | exit `0`; printed `fuzz-target-ignored` |

`fuzz/target/` holds build output from the manifest-path cargo commands of
Tasks 1.1-1.3 (`CACHEDIR.TAG`, `debug/`, `tmp/`). It is the excluded
crate's own workspace target, covered by `fuzz/.gitignore`. It is distinct
from both `target-fuzz/` (the wrapper's forced target dir, absent here)
and the shared main `./target`.

## Test commands

| Command | Result |
|---------|--------|
| `cargo test --manifest-path fuzz/Cargo.toml` | exit `0`; `harness_semantics` 5 passed, `seeds` 3 passed |
| `cargo test -p xtask fuzz` | exit `0`; `fuzz::tests` 7 passed |

## Deferred to CI

The following runtime drills are NOT executed in this environment. This
environment has no nightly toolchain and no cargo-fuzz, and AGENTS.md
forbids building xtask in the main checkout. The drills require nightly +
cargo-fuzz or main-checkout execution, which is unavailable here.

Status: `integration-verification-deferred-to-CI`.

- Full run: `cargo xtask fuzz dsl_yaml --time 300` in a worktree, writing
  only under `target-fuzz/`.
- `tmin` drill: an intentional injected panic in the harness is caught,
  minimized, and promoted to a committed regression test.
- Main-`./target` mtime check: the shared main `./target` stays cold
  during a worktree fuzz run.
- End-to-end main-checkout refusal drill: `cargo xtask fuzz` in the main
  checkout refuses to run. Only the pure `is_main_checkout` predicate is
  unit-tested locally (`fuzz::tests::main_checkout_detected` and
  `linked_worktree_detected`); the end-to-end drill needs a main-checkout
  xtask build, which AGENTS.md forbids.

These drills execute in the Phase 2 CI smoke (bd rc-7rw2). The deferral
RECORDS that the runtime drills are unexecuted — it does not claim the
proposal's runtime acceptance criteria passed. Those criteria close when
the Phase 2 CI smoke runs them.