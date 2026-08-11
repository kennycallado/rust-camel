# Design: eliminate-devdep-cycles

## Approach

SCC analysis of the real combined normal+weak publish graph reveals the
prior scope was inflated by an **over-breaking greedy loop bug** in
`resolve_publish_order`: it snips weak edges from any unscheduled crate,
not only crates inside a cycle, fabricating ~24 phantom "broken edges".
The real topology has exactly **two non-trivial SCCs**, closed by **3
weak edges in 2 holder crates**. The fix is surgical, not a sprawl.

The fix has four ordered parts, each independently shippable:

1. **Fix the cycle detector + expose it.** Replace the over-breaking
   loop with Tarjan-SCC-gated edge removal: only break a weak edge that
   lies inside a non-trivial SCC of the remaining graph. Return the
   `broken_weak_edges` from `resolve_publish_order` (today it is a
   private side-effect). Add `cargo xtask publish --show-cycles` that
   prints the true `no_verify` set + broken edges without publishing and
   without mutating manifests. This lands FIRST so the baseline is real,
   not 16 phantom crates.
2. **Cut the 3 real edges** by remediating the two holder crates:
   `camel-core` (drop the http/ws dev-deps by StubComponent-ing the
   catalog fixture tests + relocating the real-option tests to
   `camel-test`) and `camel-endpoint-macros` (relocate the derive +
   trybuild UI tests into `camel-endpoint`, the consumer — the
   syn/serde_derive canonical pattern).
3. **Document + enforce.** ADR-0055 records the invariant; a new
   `lint-publish-cycles` gate fails on a non-empty `no_verify` set OR a
   publishable crate depending on `camel-test` (leaf guard). Trustworthy
   because of step 1's SCC fix.
4. **Delete the hack.** Only after the lint is green does
   `comment_out_camel_dev_deps`, `is_weak_dependency_section`, and the
   strip/restore publish loop get deleted.

The `test-support` feature dev-deps (e.g.
`camel-component-api = { features = ["test-support"] }`) are **not
cyclic** — `camel-component-api`'s normal deps (api, auth, language-api,
endpoint) never reach back to the holder. They are all retained
unchanged. No `*-test-support` split crate is needed; the feared "crux"
does not exist in this workspace.

`camel-test` is **already a publish-order leaf** — no publishable crate
declares it in any dep kind. The phantom `config→test`/`dsl→test` edges
in the buggy output were the holder direction decoded backwards
(actual: `camel-test --dev--> {config, dsl}`). The lint keeps a cheap
manifest-scan guard as insurance.

## Ground truth (SCC analysis, verified against source)

Two non-trivial SCCs:

- **SCC-A (5 nodes):** `camel-builder, camel-http, camel-ws, camel-core,
  camel-otel`. Normal backbone `builder→core`, `http/ws→otel`; closed by
  weak edges `otel→{core,builder}` and `core→{http,ws}`.
**SCC-B (2 nodes):** `camel-endpoint, camel-endpoint-macros`. Normal
`endpoint→endpoint-macros`; closed by weak
`endpoint-macros→endpoint` (a derive-test dev-dep). The
`endpoint-macros→api` dev-dep is a companion of the same tests, not a
cycle-closing edge.

**Minimum feedback edge set (3 cyclic edges):** cut `camel-core→{http,ws}`
(2 cyclic edges, 1 holder) and `camel-endpoint-macros→camel-endpoint` (1
cyclic edge). Verified: 0 SCCs after, pure Kahn sorts all 59 crates with
0 broken edges. The `camel-endpoint-macros→camel-api` edge is a
**companion** dev-dep (api is not in SCC-B) used only by the same derive
tests; removing it is part of the endpoint-macros relocation (it leaves
with the tests), not a cycle-closing edge.

Edge-direction note (verified in `resolve_publish_order`): the print
reads `{target} --dev--> {holder} (publish {holder})`; `no_verify` = the
holder (crate whose dev-dep is cut). The message is target-first, which
inverts the dev-dep direction relative to a human read.

## The two remediation patterns (distinct — do not unify)

### Pattern 1 — camel-core http/ws: local StubComponent (+ targeted relocation)
The http/ws dev-deps feed `#[cfg(test)]` functions in
`src/component_metadata_catalog.rs`. Split by intent:
- Tests asserting only scheme registration / count / `all_metadata().len()`
  → a private `StubComponent` (implements the `Component` trait with a
  configurable scheme + synthetic `ComponentMetadata`).
- Tests asserting the REAL http/ws option catalog shape
  (`all_phase2_schemes_have_options`, `no_duplicate_option_names`) →
  relocate those specific functions to `crates/camel-test/tests/`
  (camel-test already normal-deps http; add ws to its dev-deps,
  leaf-safe).
Either way, both `camel-component-http` and `camel-component-ws` leave
camel-core `[dev-dependencies]`.

### Pattern 2 — camel-endpoint-macros: relocate derive tests to the consumer
`endpoint-macros` is a proc-macro crate; its derive-integration + trybuild
UI tests must compile macro output referencing `camel_endpoint`/`camel_api`
types — an inherent proc-macro-testing cycle. Canonical fix (syn, darling,
serde_derive): **the derive crate's integration tests live in the consumer
crate.** Move `tests/derive_integration.rs` + `tests/ui/*` + the trybuild
harness into `camel-endpoint/tests/` (already normal-deps both). Remove
`camel-api` + `camel-endpoint` from endpoint-macros `[dev-dependencies]`.

## Affected crates

- `scripts/xtask`: Tarjan-SCC-gated cycle breaking in `resolve_publish_order`; return `broken_weak_edges`; add `publish --show-cycles`; add `lint-publish-cycles`; delete `comment_out_camel_dev_deps`, `is_weak_dependency_section`, strip/restore loop; simplify `publish_crates`.
- `crates/camel-core`: StubComponent + targeted relocation of real-option catalog tests; drop http/ws from `[dev-dependencies]`.
- `crates/camel-endpoint-macros`: move derive + trybuild UI tests to `camel-endpoint/tests/`; drop api + endpoint from `[dev-dependencies]`.
- `crates/camel-endpoint`: receives the relocated endpoint-macros tests.
- `crates/camel-test`: receives the relocated real-option catalog tests; add `camel-component-ws` to `[dev-dependencies]`.
- `docs/adr`: ADR-0055; `CONTEXT-MAP.md` citation.

## Architecture boundaries

A **build/publish concern**. No public API change; no runtime data-path
change. `camel-test`'s downstream-facing API is unchanged. The relocated
tests exercise the same surfaces; only their compilation unit changes.

## Phases

### Phase 0: SCC-accurate detector + diagnostic
- **Goal:** make the cycle measurement truthful and expose it.
- **Dependencies:** none.
- **Externally-visible types/interfaces:** `resolve_publish_order` returns `(sorted, no_verify, broken_weak_edges)`; new `cargo xtask publish --show-cycles`.
- **Deliverable:** Tarjan-SCC-gated edge breaking; `--show-cycles` prints the true `no_verify` set + broken edges; baseline recorded (expected: `{camel-core, camel-endpoint-macros}`-class real set, NOT 16).
- **Exit-criteria:** `cargo xtask publish --show-cycles` runs without publishing/mutating manifests and reports the true minimal `no_verify`; `cargo build -p xtask` + clippy clean; the over-breaking loop no longer fabricates phantom edges (unit test with a synthetic acyclic-but-weak graph asserts zero broken edges).

### Phase 1: Cut the 3 real edges
- **Goal:** drive the true `no_verify` set to empty.
- **Dependencies:** Phase 0 (trustworthy baseline).
- **Externally-visible types/interfaces:** none (test relocation/stubbing).
- **Deliverable:** camel-core StubComponent + targeted real-option relocation, http/ws dev-deps removed; endpoint-macros derive + trybuild UI tests relocated to camel-endpoint, api+endpoint dev-deps removed; relocation manifest.
- **Exit-criteria:** `cargo xtask publish --show-cycles` reports empty `no_verify` and zero broken edges; `cargo test -p camel-core -p camel-endpoint -p camel-endpoint-macros -p camel-test` passes; `cargo build --benches -p camel-core` passes; relocation manifest accounts for every move.

### Phase 2: Document + enforce the invariant
- **Goal:** make the topology invariant explicit and self-enforcing.
- **Dependencies:** Phase 1 (`no_verify` empty, so the gate is green on first run).
- **Externally-visible types/interfaces:** new `cargo xtask lint-publish-cycles` gate; ADR-0055.
- **Deliverable:** ADR-0055 + CONTEXT-MAP citation; `lint-publish-cycles` (fails on non-empty `no_verify` OR a publishable crate depending on `camel-test` in any kind) wired into `AGENTS.md ## QUALITY GATES`.
- **Exit-criteria:** `cargo xtask lint-publish-cycles` exits 0; gate listed in AGENTS.md; ADR-0055 exists and cited.

### Phase 3: Delete the hack
- **Goal:** remove the workaround now that the invariant is enforced.
- **Dependencies:** Phase 2 (lint green and wired).
- **Deliverable:** `comment_out_camel_dev_deps`, `is_weak_dependency_section`, strip/restore loop deleted; `publish_crates` simplified to a plain linear topo sort.
- **Exit-criteria:** deleted symbols unreferenced; `--show-cycles` 0 edges; `lint-publish-cycles` exits 0; no Cargo.toml written during publish; `cargo build --workspace` passes.

## Alternatives considered

- **Mass relocation of ~130 files across 8 crates:** rejected — SCC analysis proves those crates are not in any real cycle; their "cyclic" edges were phantom output of the over-breaking loop. Pure churn.
- **`*-test-support` split crate:** rejected — component-api dev-deps are non-cyclic (verified); the feared crux does not exist.
- **Set `camel-test publish = false`:** rejected — it is already a true leaf (the phantom edges were decoded backwards) and is a downstream-facing public utility.
- **Keep the strip/restore hack, just add the SCC fix:** rejected — the hack mutates manifests on disk during publish (dirty-tree failure mode); once the lint enforces acyclicity the hack is unnecessary.
- **Split into multiple changes:** rejected — 3 surgical edits + loop fix + lint + ADR + hack deletion are cohesive and small (~8 tasks).
