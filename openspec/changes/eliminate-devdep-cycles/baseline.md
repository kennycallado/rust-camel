# Baseline: eliminate-devdep-cycles

**Date:** 2026-08-11
**Source:** `cargo run -p xtask -- publish --show-cycles` (post-Task-0.1 SCC fix)

This is the truthful baseline after the SCC-gated cycle detector landed
(Task 0.1). It replaces the earlier "25 broken edges / 16 crates" reading,
which was an artifact of the over-breaking greedy loop bug. The goal of
Phase 1 is to drive this set to zero.

## `no_verify` set (3 holders)

- `camel-component-http`
- `camel-core`
- `camel-endpoint-macros`

## Broken weak edges (4 real intra-SCC edges)

```
camel-component-http --dev/build-dep--> camel-core
camel-core            --dev/build-dep--> camel-component-http
camel-core            --dev/build-dep--> camel-component-ws
camel-endpoint-macros --dev/build-dep--> camel-endpoint
```

Edge semantics: `{from=target} --dev--> {to=holder}`; `no_verify` = holders
(the crate that DECLARES the dev-dep).

## Why 4 (not the decision doc's 3)

`DECISION-devdep-cycles.md` enumerated 3 cyclic edges. The SCC solver
correctly found a 4th: `camel-component-http --dev--> camel-core` (the
crate dir is `crates/components/camel-http` but the crate name is
`camel-component-http`; its `Cargo.toml` declares a `camel-core`
dev-dependency). This forms a mutual dev/dev cycle with
`camel-core --dev--> camel-component-http`. The solver finding a real edge
the manual analysis missed is a positive correctness signal.

## Phase 1 expectation

After Task 1.1 (camel-core drops its `camel-component-http` +
`camel-component-ws` dev-deps) and Task 1.2 (camel-endpoint-macros derive
tests relocate to camel-endpoint), all four edges dissolve:

- `core→http` and `core→ws`: removed by Task 1.1.
- `http→core`: becomes one-way (no return path) once `core→http` is gone.
- `endpoint-macros→endpoint`: removed by Task 1.2.

Task 1.3 verifies `no_verify set: 0 crate(s)` and zero broken edges.
