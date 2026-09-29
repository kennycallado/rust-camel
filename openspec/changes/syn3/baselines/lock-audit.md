# syn3 — Cargo.lock audit (task 3.1)

Baseline: `openspec/changes/syn3/baselines/Cargo.lock.pre` (captured task 1.3,
syn 2 pin). Audited lock: `Cargo.lock` at HEAD (syn 3, post-migration).

Method: `git diff --no-index baselines/Cargo.lock.pre Cargo.lock` — every hunk
walked, counted, justified.

## Result: 3 hunks, 3/3 justified as the syn edge move

| # | Hunk | Justification |
| --- | --- | --- |
| 1 | camel-bean-macros deps: `"syn 2.0.119"` -> `"syn 3.0.4"` | workspace pin flip (task 2.1); direct-dep edge rewiring |
| 2 | camel-endpoint-macros deps: `"syn 2.0.119"` -> `"syn 3.0.4"` | same |
| 3 | xtask deps: `"syn 2.0.119"` -> `"syn 3.0.4"` | same |

Zero unrelated changes. syn 3.0.4 node already existed in the baseline lock
(pulled by async-trait 0.1.92); syn 2.0.119 node retained — required by
unmigrated third-party proc-macros (56 reverse edges). No version bumps, no
additions, no removals. Matches the task-2.1 inventory header exactly.

## cargo audit

Tool present, advisory DB resolved offline. Findings identical against BOTH
locks (pre and post): RUSTSEC-2026-0316/0315 (wasmtime 48.0.1),
RUSTSEC-2026-0314 (wasmtime-wasi 48.0.1), plus allowed warnings incl.
RUSTSEC-2023-0089 (atomic-polyfill 1.0.3). Migration introduced ZERO new
advisories. Pre-existing wasmtime advisories are out of mission scope (wasmtime
is byte-pinned at 48.0.1 per workspace Cargo.toml; tracked elsewhere).

## Battery results (recorded here as task evidence)

- `cargo test -p camel-dsl --lib`: 851 passed / 0 failed (task text said 854 —
  static estimate; actual compiled count is 851, which meets the mission's
  "851+" baseline). goldens corpus: zero modifications (`git status` clean).
- `cargo test --workspace --lib`: 10,225 passed / 0 failed across 71 binaries.
  First run had 26 camel-cli failures — all "camel binary not built" harness
  panics (tests probe target/debug/camel; `--lib` does not build the bin
  target). After `cargo build -p camel-cli`: camel-cli --lib 561/0, full
  battery 10,225/0. Not syn-related.
- Clippy legs (AGENTS.md verbatim): LEG1 workspace-all-features (excl.
  camel-cli/kafka/keycloak/wasm-policy) EXIT 0; LEG2 kafka all-targets EXIT 0;
  LEG3 cli EXIT 0; LEG4 cli-nodflt flavor-regular,exec all-targets EXIT 0.
- `cargo fmt --all -- --check`: EXIT 0.

Resource caps honored (CARGO_BUILD_JOBS=6, test -j 4, sequential cargo, solo
mission).
