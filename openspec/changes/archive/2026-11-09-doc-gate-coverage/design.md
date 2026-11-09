# Design: doc-gate-coverage

## Approach

Three parts: fix the link, widen the gate, prove the gate works.

### 1. Link fix (root cause established by experiment)

`EMBEDDED_REF_PREFIX` is a `pub const` in the same module as the failing link, yet the unqualified link fails. The mechanism, reproduced A/B on a scratch crate (stable 1.99.1, edition 2024):

- `crates/components/camel-component-api/src/lib.rs:15` puts an outer doc (`/// Registry of sealed-boot embedded xslt/xsd/sql assets; …`) on the `pub mod embedded_assets;` declaration.
- With an outer doc on the declaration, rustdoc resolves the whole merged module-doc block — outer `///` plus the inner `//!` block in `embedded_assets.rs` — in the declaration's scope (the crate root), not the module scope.
- From crate-root scope, plain `[`EMBEDDED_REF_PREFIX`]` and `[`self::EMBEDDED_REF_PREFIX`]` both fail ("no item named `EMBEDDED_REF_PREFIX` in scope"); removing the outer doc makes the plain link resolve; `[`crate::embedded_assets::EMBEDDED_REF_PREFIX`]` always resolves.

Fix: qualify the link at `embedded_assets.rs:17` as `[`crate::embedded_assets::EMBEDDED_REF_PREFIX`]`. The item is public and must stay linked (it is the reserved-reference contract documented in the module); the other links in the file live in item docs, resolve in module scope, and are left alone. The private-const branch of the mission order (demote to plain code formatting) does not apply: the const is `pub`. No `#[doc(hidden)]`, no allow attributes.

Adding the crate to the gate surfaces no further warnings: the unpatched crate generates exactly one rustdoc warning (this link), verified with a warning-only doc run.

### 2. Gate coverage

Add `-p camel-component-api` to the `doc-build` entry in the `## QUALITY GATES` block of `AGENTS.md`, grouped with its sibling on the last continuation line:

```
- name: doc-build
  run: RUSTDOCFLAGS="-D warnings" cargo doc -p camel-api -p camel-core
    -p camel-builder -p camel-dsl -p camel-endpoint
    -p camel-component-api -p camel-component-validator -p camel-xslt --no-deps
```

Continuation lines keep the four-space `-p …` shape: `scripts/gates-local.sh` parses this block (it is the source of truth for local gates), and the pre-fix parser bug (folded scalar, dropped `--no-deps`) is exactly what hid this crate during mission 546. The script itself is not touched.

### 3. Negative proof

Copy the whole workspace (manifests, path dependencies, all eight packages) to a disposable scratch workspace outside the repo tree, reintroduce the unqualified bad link in its `embedded_assets.rs` copy only, and run the eight-crate registry command there: it must exit 101 with a diagnostic naming the broken `EMBEDDED_REF_PREFIX` intra-doc link in `embedded_assets.rs`. The feature worktree itself is never mutated for this proof; the command, diagnostic line, and exit status are recorded verbatim for the park. The scratch workspace is deleted after capture.

## Affected crates

- camel-component-api: one doc-comment line changed; no code, no tests, no manifest change.
- AGENTS.md: one registry line widened to eight crates.

## Architecture boundaries

No Runtime, DSL, Components, Services, Languages, or Functions code path changes. The Components boundary is touched only through its API crate's documentation. `AGENTS.md` is the project constitution and the gate registry's single source of truth; the edit widens coverage of an existing gate rather than adding a gate or a lint. `scripts/gates-local.sh` remains a pure parser of that registry.

## Spec handling

`skip_specs: true`. Justification: no spec canon covers the `doc-build` gate set — `rg -l 'doc-build' openspec/specs/` returns nothing, and the `ci-compile-gate` spec is about the integration-test compile step, not rustdoc. The gate registry lives in `AGENTS.md`, not in `openspec/specs/`; this change widens an existing registry entry and does not introduce operator-visible behavior. If the spec bless rules otherwise, the delta goes under a new `doc-gate-coverage` spec as directed by the mission order.

## Verification

Run in the worktree, foreground, under the fleet scope unit:

1. `RUSTDOCFLAGS="-D warnings" cargo doc -p camel-component-api --no-deps` → exit 0.
2. The full eight-crate registry command as written in `AGENTS.md` → exit 0.
3. Negative proof (disposable workspace copy) → exit 101, diagnostic naming the broken `EMBEDDED_REF_PREFIX` link in `embedded_assets.rs`; command, diagnostic, and exit status recorded in the park.
4. `bash scripts/gates-local.sh` → exit 0, `passed 28, failed 0, skipped 4`, `doc-build` lists eight crates.
5. `cargo fmt --check --all` → exit 0.
