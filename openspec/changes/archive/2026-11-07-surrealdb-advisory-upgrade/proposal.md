# Proposal: surrealdb-advisory-upgrade

## Why

`cargo audit` fails on `main` with two vulnerabilities in the pinned `surrealdb` /
`surrealdb-core` 3.2.4 (pulled through `camel-component-surrealdb` and `camel-test`):

- **RUSTSEC-2026-0453** — use-after-free in KV backends via a leaked
  `affinitypool::spawn_local` future; fixed in ≥ 3.3.0.
- **RUSTSEC-2026-0454** — data race and double free via unsound `Send`/`Sync` in
  `reblessive`; fixed in ≥ 4.0.0.

Every recent mission has recorded this as a pre-existing audit-gate exemption
(bd rc-yhpyz, P1). The debt now blocks the audit gate everywhere; this change
clears it at the source.

## What Changes

1. **Bump `surrealdb` 3.2.4 → 4.0.0** in the workspace manifest
   (`surrealdb = { version = "4", ... }`, same features: `protocol-ws`,
   `protocol-http`, `rustls`, `kv-mem`) and regenerate `Cargo.lock`. 4.0.0
   (released 2026-11-03) clears **both** advisories: `surrealdb-core` 4.0.0 is
   past the 0454 fix gate, and the bump moves `affinitypool` 0.7 → 0.8 past the
   0453 fix gate. Verified by probe at mission start: `cargo check` of
   `camel-component-surrealdb`, `camel-test`, `surrealdb-example`, and
   `camel-bundles` compiles with zero errors and zero code changes (the
   component itself on its default target; the other packages with
   `--all-targets`), and `cargo audit` on the probed lock reports **zero
   vulnerabilities** (only the 7 pre-allowed warnings remain). The
   feature-gated `camel-test` SurrealDB fixtures (`integration-tests`
   feature, Docker-backed, server fixture pinned `v3.1.4`) were NOT part of
   the probe — runtime compatibility is verified only by the dedicated
   runtime gate below.
2. **Transitive resolution delta** from the surrealdb subtree (e.g.
   `ammonia`, `object_store`, `diskann-*`, `html5ever`, `string_cache`) is part
   of the bump and is captured as a lock-diff evidence file. No direct
   manifest dependency outside `surrealdb` changes.
3. **CHANGELOG.md** entry under `### Changed` (dependency/security class,
   bd rc-yhpyz). Crate `CONTEXT.md` and `docs/**` change only if verification
   shows a documented behavior moved (component public surface and the
   documented URI parameters are expected to stay identical).
4. **Evidence record** (`evidence.md` in this change dir): before/after audit
   output, `cargo tree -d --target all` for `surrealdb` / `jsonwebtoken` /
   `ed25519-dalek`, exact post-bump lock versions, lock diff summary, and any
   test that had to change with the reason.

**Not in this change:** the `jsonwebtoken` 10.4.0 / `ed25519-dalek` 2.2.0
duplicates (still present under `surrealdb-core` 4.0.0 — ecosystem-blocked,
same class as the deferral recorded by mission 515); no `audit.toml` ignore is
needed because no advisory remains; no unrelated dependency bumps; no
reformatting of the SurrealDB component.

Affected crates: `camel-component-surrealdb`, `camel-test`, `camel-bundles`,
`surrealdb-example` (all compile-time consumers; expected zero code change),
root `Cargo.toml`, `Cargo.lock`, `CHANGELOG.md`.

## Acceptance criteria

- `cargo audit` exits clean: **zero vulnerabilities** (RUSTSEC-2026-0453 and
  -0454 gone; the 7 pre-allowed warnings unchanged).
- `Cargo.lock` pins `surrealdb` 4.0.0 and `surrealdb-core` 4.0.0.
- Component public surface unchanged: `cargo check --workspace --all-targets`
  green; `camel-component-surrealdb` and `camel-test` test suites green.
- `camel-test` SurrealDB Docker suite green **as executed tests**:
  `cargo test -p camel-test --features integration-tests --test
  surrealdb_test` (suite is `#![cfg(feature = "integration-tests")]` and
  asserts against the testcontainers server fixture pinned `v3.1.4`;
  default-feature checks do not compile it). Any test that must change is
  recorded with the reason.
- Workspace gates green: fmt, AGENTS.md clippy set, lint battery, doc tests.
- CHANGELOG entry present; evidence.md complete; park written to the fleet
  inbox.

## Risk budget

Low-to-medium: 4.0.0 is 3 days old, so compile-clean does not guarantee
runtime-identical behavior — mitigated by running the real Docker-backed
SurrealDB suite and the full local gate set before parking. Compile-only risk
is already discharged by the probe. Out of bounds: forcing jsonwebtoken
unification, bumping any other direct dependency, editing component code
beyond what a failing test forces, blanket audit.toml ignores.
