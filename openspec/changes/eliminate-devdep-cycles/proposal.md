# Proposal: eliminate-devdep-cycles

## Why

`cargo publish` to crates.io fails when a publishable crate declares a
`camel-*` dev-dependency or build-dependency that closes a publish-order
cycle (crates.io resolves dev-deps against already-published versions, so
cargo cannot topologically sort them). The workaround today is an xtask
hack (commit `146c28ee`, ~200 LoC in `scripts/xtask/src/main.rs`) that
mutates each cyclic crate's `Cargo.toml` at publish time — comments out
the `camel-*` lines, publishes with `--no-verify`, then restores the
original bytes. This carries a real failure mode: a crash between write
and restore leaves a dirty tree, and the published manifest silently
differs from source.

SCC analysis of the real dependency graph shows the earlier scope
estimate ("16 cyclic crates / 25 edges") was inflated by an over-breaking
greedy loop in `resolve_publish_order` that snips weak edges from any
unscheduled crate, not only crates inside a cycle. The true topology has
**two non-trivial SCCs** closed by **3 weak edges in 2 holder crates**.
This change fixes the detector, cuts the 3 edges, and deletes the hack.

## What Changes

**Scope correction (post-SCC analysis):** the cycle graph has exactly **two non-trivial SCCs** closed by **3 weak edges in 2 holder crates** (`camel-core→{http,ws}` and `camel-endpoint-macros→{endpoint,api}`). The earlier "16 cyclic crates / 25 edges" reading was an artifact of an over-breaking greedy loop bug in `resolve_publish_order`. The `test-support` feature dev-deps are non-cyclic and are all retained.

**Included:**
- Fix `resolve_publish_order`'s cycle-breaking loop to be **SCC-accurate** (Tarjan-gated: break only weak edges inside non-trivial SCCs); return `broken_weak_edges` from the function (today a private side-effect).
- Add `cargo xtask publish --show-cycles` diagnostic (no publish, no manifest mutation) so the baseline is the true minimal `no_verify` set.
- **camel-core:** StubComponent the catalog fixture tests; relocate the real http/ws-option catalog tests to `camel-test`; drop `camel-component-http` + `camel-component-ws` from `[dev-dependencies]`.
- **camel-endpoint-macros:** relocate `derive_integration.rs` + the trybuild UI tests into `camel-endpoint/tests/` (the consumer crate, syn/serde_derive pattern); drop `camel-api` + `camel-endpoint` from `[dev-dependencies]`.
- New `cargo xtask lint-publish-cycles` gate (SCC-accurate + camel-test leaf guard) wired into `AGENTS.md ## QUALITY GATES`; lands BEFORE the hack is deleted.
- Delete `comment_out_camel_dev_deps`, `is_weak_dependency_section`, and the strip/restore loop; `publish_crates` simplifies to a plain linear topo sort.
- ADR-0055 recording the topology invariant, cited from `CONTEXT-MAP.md`.

**Excluded (explicitly):**
- Mass relocation of ~130 files across llm/surrealdb/template/validator/exec/wasm/builder — SCC analysis proves those crates are NOT in any real cycle; their "cyclic" edges were phantom. Pure churn.
- A `*-test-support` split crate — component-api dev-deps are non-cyclic (verified); the feared crux does not exist.
- Setting `camel-test publish = false` — it is already a true leaf (phantom edges were decoded backwards) and is a downstream-facing public utility.
- Touching any `test-support` feature dev-dep — all retained unchanged.

**Affected crates:** `scripts/xtask` (SCC fix + diagnostic + lint + hack removal), `crates/camel-core` (StubComponent + 2 dev-deps removed), `crates/camel-endpoint-macros` (derive/UI tests relocated, 2 dev-deps removed), `crates/camel-endpoint` (receives endpoint-macros tests), `crates/camel-test` (receives real-option catalog tests + ws dev-dep), `docs/adr` (ADR-0055).

**bd issue:** rc-erh6 (discovered-from rc-mwb2).

## Acceptance criteria

- `cargo xtask publish --show-cycles` reports 0 broken weak edges and an empty `no_verify` set.
- `cargo xtask lint-publish-cycles` passes (exit 0) and is wired into the project gate suite.
- `comment_out_camel_dev_deps`, `is_weak_dependency_section`, and the strip/restore loop are deleted; `publish_crates` is a plain linear topo sort.
- All relocated tests still pass (moved, not deleted).
- ADR-0055 is merged into `docs/adr/` and cited from `CONTEXT-MAP.md`.

## Risk budget

**Acceptable:** touching the publish pipeline and moving integration
tests, because the cycle diagnostic gives a measurable monotone baseline
and each phase is independently shippable.

**Out of bounds:** changing any published crate's *normal* dependency
surface, changing public API, or splitting crates. If a hidden cyclic
edge in `src/` `#[cfg(test)]` or a doctest cannot be resolved by the
`test-support` pattern, escalate to the human rather than refactor
library internals.
