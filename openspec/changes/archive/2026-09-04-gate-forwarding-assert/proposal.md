# Proposal: gate-forwarding-assert

## Why

`camel-bundles` exposes 8 optional component gates as Cargo features
(`http-static`, `kafka`, `mqtt`, `surrealdb`, `grpc`, `llm`, `mcp`,
`wasm`). The workspace dependency declares `default-features = false`,
so each consumer crate must re-forward every gate it wants through its
own `[features]` table (`kafka = ["dep:camel-component-kafka",
"camel-bundles/kafka"]`). Cargo does not verify this forwarding. A gate
added to `camel-bundles` without matching forwarding in a consumer
compiles clean, then that consumer silently boots without the
component. The route then fails at boot with an unknown scheme. The
failure is loud but late, and only on the code path that uses the missing component.

Two consumers exist today (`camel-cli`, `camel-integration-test`) and a
third (`camel command`) is planned on top of `camel-bundles`. The drift
surface grows with every consumer. bd rc-n8ss (inter-phase review
finding, change `integration-tier-contract`) asked for exactly this
guard.

## What Changes

- New xtask command `lint-gate-forwarding`:
  - Rule 1 (shadow-feature forwarding): a consumer feature whose name
    equals a bundles gate must transitively activate
    `camel-bundles/<gate>` through the consumer's own feature graph.
  - Rule 2 (boot-consumer completeness): a consumer marked
    `[package.metadata.camel-bundles] boot-consumer = true` must
    forward every bundles gate through some feature.
  - Consumers are discovered by scanning workspace member manifests for
    a `camel-bundles` dependency. There is no hardcoded crate list.
- `camel-cli` declares the `boot-consumer` metadata marker
  (`camel run` aims to boot every gate). `camel-integration-test` stays
  unmarked: its core-only, `default-features = false` composition is
  intentional (ADR-0069 demand gating).
- CI step + entry in the AGENTS.md QUALITY GATES registry.

Excluded: changing any feature semantics, forcing
`camel-integration-test` to forward gates, runtime code of any crate.

## Acceptance criteria

- `cargo xtask lint-gate-forwarding` exits 0 on the current tree.
- Seeded violations are reported with crate, feature, and gate names,
  and exit 1. Seeds cover a shadow feature without forwarding and a
  boot consumer missing one gate.
- A non-consumer crate with a feature named like a gate produces no
  violation.
- The lint runs in CI and is listed in AGENTS.md `## QUALITY GATES`.

## Risk budget

Tooling-only. The one manifest edit (metadata marker on `camel-cli`)
is inert to cargo. Worst case: the lint has a false positive and blocks
CI. This is acceptable. It is fixable by correcting the rule, and it cannot
touch runtime behavior. Out of bounds: any change to how features
compose at build time.

Bd: rc-n8ss
