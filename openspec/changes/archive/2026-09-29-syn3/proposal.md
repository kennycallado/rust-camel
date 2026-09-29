# Proposal: syn3

## Why

Workspace pins `syn = "2.0"` (lock: 2.0.119). syn 3.0.0 shipped 2026-07-18 with a
major API break; the Dependabot auto-bump (PR #40, syn 2.0.119 -> 3.0.4) fails
validate because three in-tree consumers use the syn AST directly. A
dependabot ignore-rule for syn semver-major landed meanwhile (cc8a5b23), so the
migration is now a deliberate task instead of an automated bump.

Cost of staying: transitive `async-trait 0.1.92` already builds on syn 3.0.4,
so the lock carries syn 2 + syn 3 in parallel — double compile cost for every
clean CI run, and our direct pin blocks all future syn 3 fixes.

bd: rc-tl6m (P3, task). Mission order: `.opencode/fleet/orders/298-syn3-mission.md`.

## What Changes

- Workspace pin `syn = "2.0"` -> `syn = "3"` in the root `Cargo.toml`
  (single-source rule: one workspace version, no dual-pin period).
- Code migration in the three direct syn consumers:
  - `crates/camel-bean-macros` (features `full`, `parsing`, `extra-traits`)
  - `crates/camel-endpoint-macros` (features `full`, `parsing`, `extra-traits`)
  - `scripts/xtask` (features `full`, `visit`)
- `Cargo.lock` updated only by this bump; diff audited for zero unrelated churn.
- No behavior change: macros must stay output-identical (pre/post pin
  byte-comparison of representative expansions + error diagnostics is the
  parity oracle); xtask lints keep their exact verdict surfaces (pre/post
  finding-tuple capture, ratchet files byte-identical).

Explicitly excluded: no transitive-dependency bumps beyond what the syn edge
move forces; no DSL surface changes; no spec deltas (`skip_specs: true` —
dependency internals); no dependabot.yml edits.

## Acceptance criteria

- The three consumers compile, pass clippy `-D warnings`, and pass their test
  batteries on syn 3.0.x.
- Downstream batteries green: `camel-dsl --lib` (854 tests), goldens corpus
  unchanged, `camel-bean`/`camel-endpoint` downstream compile.
- Pre/post pin byte-comparison passes: macro expansion baselines
  (bean-macros, endpoint-macros: outputs + error diagnostics) and xtask lint
  finding-tuple captures are byte-identical across the migration.
- Lock diff contains only the syn edge move; syn 2.x may remain in lock only as
  a transitive dep of unmigrated third-party proc-macros.
- A fresh manual bump PR for syn 3.0.4 would pass the same validate gates
  (fmt, clippy legs, tests) that PR #40 failed.

## Risk budget

DSL-core macro work — rigor required. Acceptable: mechanical API rewrites
(e.g. the `Receiver` field split) with byte-identical output proven by the
pre/post expansion baselines. Out of bounds: any change in macro-generated
tokens, lint findings delta, or unrelated lock churn. No merge in this
mission — park for human landing review.
