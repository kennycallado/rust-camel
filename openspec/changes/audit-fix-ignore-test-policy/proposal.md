# Proposal: audit-fix-ignore-test-policy

## Why

The workspace had 29 `#[ignore]` tests with zero enforcement. The
`audit-fix-wit-versioning` change (ADR-0053) broke 13 buildable WASM
integration tests by re-versioning `camel:plugin` to `1.0.0`. The merge
gate (`cargo test --workspace`) silently skipped all of them.

Separately, an audit found that 7 inline `#[ignore]` tests in component
crates duplicated existing, superior testcontainers-based coverage in
`camel-test` — coverage that simply wasn't wired into CI.

This change closes both gaps: it establishes a `#[ignore]` policy with
a closed vocabulary, enforces it with an xtask lint, wires the buildable
WASM tests into CI, wires the existing external-service integration
tests into CI, and deletes the redundant duplicates.

## What Changes

**Included:**
- ADR-0054: two-prefix `#[ignore]` classification policy
  (`requires pre-built`, `slow test:`)
- `cargo xtask lint-ignore`: source-scanning lint enforcing the
  vocabulary, rejecting `requires live` as a migration error
- Deletion of 7 duplicate inline tests (kafka 2, redis 3, opensearch 1,
  k8s 1) — superseded by existing `camel-test` testcontainers coverage
- CI `wasm-integration` job: builds WASM guest crates, runs buildable
  `#[ignore]` tests per-target (derived from allowlist)
- CI integration tests: wires existing `camel-test` testcontainers tests
  (Kafka, Redis, OpenSearch, K8s) into `full-tests-linux` job
- Normalization of remaining `#[ignore]` reasons (Ollama → `slow test:`,
  camel-file → `slow test:`)
- `lint-ignore` in `AGENTS.md` QUALITY GATES and `ci.yml` quality job

**Excluded:**
- Ollama testcontainers provisioning (4B model pull — genuinely special;
  follow-up bd issue filed for nightly workflow decision)
- Any modification to test bodies — only `#[ignore]` strings change

## Acceptance criteria

- ADR-0054 follows repo format, references ADR-0012/0049/0053
- `lint-ignore` rejects bare `#[ignore]`, unrecognized prefixes, and
  `requires live` as a migration error
- `lint-ignore` passes on all remaining `#[ignore]` tests
- CI `wasm-integration` job present and structurally correct
- CI `full-tests-linux` job runs kafka/redis/opensearch/kubernetes
  integration tests via testcontainers
- 7 duplicate inline tests deleted, no compilation errors

## Risk budget

- **Acceptable:** CI cost of one `wasm32-wasip2` guest build + per-target
  `--ignored` test runs + testcontainers integration tests per PR.
- **Out of bounds:** Any change to existing test logic.

Bd: rc-4old (epic), rc-k9b6, rc-yrml, rc-3fu1, rc-4pat
