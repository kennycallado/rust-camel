# ADR-0055: Publish Topology — No Cyclic Dev/Build-Dependencies on Publishable Crates

**Date:** 2026-08-11
**Status:** Proposed
**Related:** ADR-0045 (camel-core architecture charter), ADR-0049 (lint+policy pairing precedent)

## Context

### Problem

`cargo publish` resolves `[dev-dependencies]` and `[build-dependencies]`
against the crates.io registry during package verification, so weak
edges participate in the topological publish order. A cycle closed only
by weak edges cannot be sorted: the holder cannot publish before its
dev-dep target, and the target cannot publish before the holder. The
workaround was an xtask hack (commit `146c28ee`, ~200 LoC) that mutated
each cyclic crate's `Cargo.toml` on disk at publish time — commenting
out the `camel-*` dev-dep lines, publishing with `--no-verify`, then
restoring the original bytes. This carried a real failure mode: a crash
between write and restore left a dirty tree, and the published manifest
silently differed from source.

### Diagnostic discovery (the over-breaking bug)

The first analysis reported "25 broken weak edges / 16 cyclic crates"
and proposed mass test relocation. Tarjan SCC analysis of the real
combined normal+weak graph revealed this was an artifact of an
**over-breaking greedy loop** in `resolve_publish_order`: the loop
snipped weak edges from *any* still-unscheduled crate, not only crates
inside a non-trivial SCC. The real topology had **two non-trivial
SCCs** closed by **four weak edges**:

- **SCC-A:** `{camel-builder, camel-component-http, camel-component-ws, camel-core, camel-otel}` — closed by `camel-core --dev--> camel-component-http`, `camel-core --dev--> camel-component-ws`, and the mutual `camel-component-http --dev--> camel-core`.
- **SCC-B:** `{camel-endpoint, camel-endpoint-macros}` — closed by `camel-endpoint-macros --dev--> camel-endpoint` (a proc-macro derive-test dev-dep).

The other ~20 "broken edges" were collateral. The `test-support`
feature dev-deps (e.g. `camel-component-api = { features = ["test-support"] }`)
are **non-cyclic** — `camel-component-api`'s normal deps never reach back
to the holder — and needed no change.

## Decision

A crate published to crates.io **MUST NOT** declare a `camel-*`
dev-dependency or build-dependency that closes a publish-order cycle.
Cycle detection is **SCC-accurate**: `resolve_publish_order` runs Tarjan
on the combined normal+weak graph and breaks only weak edges whose both
endpoints lie inside a non-trivial SCC, recomputing after each cut, with
deterministic lexicographic edge selection. This makes the
`lint-publish-cycles` gate and `publish --show-cycles` diagnostic
truthful (no phantom edges).

`camel-test` is the **publish-order leaf sink**: no publishable crate
declares it in any dependency kind, so it is topologically incapable of
joining a cycle. It stays published as the downstream-facing test
utility crate (`cargo add camel-test`).

Two remediation patterns resolve any cycle that does arise:

1. **StubComponent substitution** — when a real component is incidental
   scaffolding (registered only to assert scheme registration or a
   count), a local stub implementing the `Component` trait replaces it.
2. **Proc-macro test relocation to the consumer** — a proc-macro crate's
   derive-integration and trybuild UI tests live in the consumer crate,
   which already normal-depends on the proc-macro crate (the syn /
   serde_derive canonical pattern).

The manifest-mutation hack (`comment_out_camel_dev_deps` and the
strip/restore publish loop) is **deleted**. `publish_crates` is a plain
linear topological sort and fails closed if `no_verify` is non-empty.
(`is_weak_dependency_section` is retained — it is benign shared
edge-classification logic used by the publish-graph builder and the leaf
guard, not part of the strip/restore hack.)

## Forces

- **Publish-cycle constraint:** crates.io resolves weak edges against
  published versions; cargo cannot topologically sort a weak-only cycle.
- **Test locality vs. topology:** tests want to live near the code they
  test, but a cyclic dev-dep blocks publish. The two remediation patterns
  resolve the tension with minimal code movement.
- **Diagnostic correctness:** the over-breaking loop made the lint
  untrustworthy (it would have failed on 16 phantom crates). SCC-gating
  is the load-bearing fix that makes the invariant enforceable.
- **Rejected manifest-mutation hack:** it mutated `Cargo.toml` on disk
  during publish (dirty-tree failure mode) and produced a published
  manifest that silently differed from source.

## Alternatives considered

- **Mass relocation of ~130 files across 8 crates:** rejected — SCC
  analysis proved those crates were not in any real cycle; their
  "cyclic" edges were phantom output of the over-breaking loop. Pure
  churn.
- **`*-test-support` split crate:** rejected — `camel-component-api`
  dev-deps are non-cyclic (verified); the feared "I need test-support but
  dev-depending on it is cyclic" case does not exist in this workspace.
- **Set `camel-test publish = false`:** rejected — it is already a true
  leaf (the phantom edges were the holder direction decoded backwards)
  and is a downstream-facing public utility.
- **Keep the strip/restore hack, just add the lint:** rejected — the
  hack's dirty-tree failure mode and published-manifest drift are
  unnecessary once the lint enforces acyclicity.

## Enforcement

`cargo xtask lint-publish-cycles` (wired into `AGENTS.md ## QUALITY
GATES`) fails when `no_verify` is non-empty OR when any publishable crate
declares `camel-test` in any dependency kind. It reuses the same
SCC-accurate `resolve_publish_order` predicate as
`cargo xtask publish --show-cycles`.
