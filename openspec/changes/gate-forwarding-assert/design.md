# Design: gate-forwarding-assert

## Approach

A single new xtask lint, `lint-gate-forwarding`, module
`scripts/xtask/src/lint_gate_forwarding.rs`, following the module
pattern of `lint_component_deps.rs` (manifest parsing with the `toml`
crate) and the dispatch pattern of every `lint-*` command in
`scripts/xtask/src/main.rs`. Zero violations print
`lint-gate-forwarding: OK (0 violations)` to stdout and exit 0.
Violation lines print to stdout, followed by
`lint-gate-forwarding: FAILED` on stderr, exit 1. Internal errors
print `lint-gate-forwarding error: {e}` on stderr and exit 2.

The lint is pure manifest analysis. It never compiles code.

Inputs, resolved from the workspace root:

1. Gates: parse `crates/camel-bundles/Cargo.toml` `[features]`. Gates
   are every key except `default`. Expected today: the 8 listed in the
   proposal.
2. Consumers: every workspace member manifest whose `[dependencies]`
   or `[dev-dependencies]` tables contain a `camel-bundles` key.
   Membership comes from the root manifest `[workspace] members` globs,
   the same resolution `lint_component_deps` already performs.
3. Boot-consumer marker: `[package.metadata.camel-bundles]
   boot-consumer = true` on a consumer manifest.

Rules:

- Rule 1, shadow-feature forwarding. For each consumer feature whose
  name equals a gate name, resolve the transitive closure of that
  feature through the consumer's own `[features]` (string entries
  activate sibling features. `dep:` entries are skipped). The closure
  must contain `camel-bundles/<gate>`. A same-named feature that only
  pulls `dep:camel-component-<x>` is the exact rc-n8ss failure mode:
  the component compiles, the cascade does not register it.
- Rule 2, boot-consumer completeness. For each marked consumer, for
  every gate, some feature closure must contain
  `camel-bundles/<gate>`. Unmarked consumers are exempt.
  `camel-integration-test` deliberately boots the unconditional core
  set with `default-features = false` (ADR-0069 §8 demand gating). A
  future gate-hungry scenario opts in per gate when a scenario demands
  it.

Why a metadata marker instead of a hardcoded crate list: the planned
`camel command` becomes a boot consumer by adding one inert manifest
line. The lint never edits its own source to track the fleet. Cargo
preserves `[package.metadata]` verbatim, so the marker is invisible to
the build.

Violation lines name all three of crate, feature, gate, for example
`crates/camel-cli: feature 'kafka' shadows bundles gate 'kafka' but
does not forward camel-bundles/kafka` and `crates/camel-cli: boot
consumer does not forward gate 'mqtt'`.

## Affected crates

- `scripts/xtask`: new lint module, dispatch wiring, unit tests.
- `camel-cli`: `[package.metadata.camel-bundles] boot-consumer = true`
  (one inert manifest block, no code).
- CI workflow + AGENTS.md gate registry (one step, one line).

## Architecture boundaries

The lint reads manifests only. It sits in the tooling plane next to
`lint-component-deps` and `lint-publish-cycles`. It touches no runtime
crate, respects the data/control plane split (pure analysis, no side
effects), and enforces an ADR-0069 §8 contract: demand-gated activation
must stay explicit and must not silently diverge between a consumer's
compile-time features and the `camel_bundles::boot` cascade it runs.
It is the tripwire that keeps the rc-n8ss seam sealed before a third
consumer exists.
