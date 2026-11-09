# Proposal: doc-gate-coverage

## Why

`RUSTDOCFLAGS="-D warnings" cargo doc -p camel-component-api --no-deps` exits 101. The module doc at `crates/components/camel-component-api/src/embedded_assets.rs:17` links `EMBEDDED_REF_PREFIX`, and rustdoc rejects the link (bd `rc-5ifo3`).

No gate covers this crate. The `doc-build` entry in the `## QUALITY GATES` block of `AGENTS.md` documents exactly seven crates (camel-api, camel-core, camel-builder, camel-dsl, camel-endpoint, camel-component-validator, camel-xslt), and the fixed `scripts/gates-local.sh` parser runs exactly the registry command. camel-component-api is documented by no gate, so this rustdoc defect is invisible to every agent run and surfaces only when a human runs rustdoc.

## What Changes

- Fix the broken intra-doc link in `embedded_assets.rs` with a path-qualified link. No `#[doc(hidden)]`, no `#![allow]`, no lint suppression.
- Add `-p camel-component-api` to the `doc-build` registry entry in `AGENTS.md`, keeping the continuation-line shape the gate parser expects.
- Prove the gate now catches the regression: on a scratch copy, reintroduce the bad link and show the registry command exits 101.

Excluded: runtime code, test behavior, new dependencies, `scripts/gates-local.sh` (it parses the registry; this is a registry edit, not a script edit). The sibling registry crate `camel-proto-compiler` was checked and is clean (no outer doc on its `pub mod embedded;` declaration, rustdoc exits clean) — nothing to report there.

## Acceptance criteria

- `RUSTDOCFLAGS="-D warnings" cargo doc -p camel-component-api --no-deps` exits 0.
- The full registry command (all eight crates, exactly as written in `AGENTS.md`) exits 0.
- Negative proof: with the bad link reintroduced in a disposable full-workspace copy, the same command exits 101 with a diagnostic naming the broken `EMBEDDED_REF_PREFIX` intra-doc link in `embedded_assets.rs` (command, diagnostic, and exit status recorded in the park).
- `bash scripts/gates-local.sh` exits 0 with `passed 28, failed 0, skipped 4`, and the `doc-build` gate output lists eight crates.
- `cargo fmt --check --all` exits 0.

## Risk budget

Doc-comment and registry edits only. Two accepted risks: a registry edit could break the parser's continuation-line shape (mitigated by running the full `gates-local.sh` sweep), and adding the crate to the gate could surface further rustdoc warnings (mitigated by the verification run; anything beyond a wrong path or a missing backtick stops the change and reports instead of patching).

Affected crates: camel-component-api (doc comment only), AGENTS.md (gate registry). bd issue `rc-5ifo3`.
