# bump-version — canonical release bump procedure

Verified against the v0.48.0 precedent (`0f67ee5d`) and the v0.49.0 bump
(`f8eb976a`). Follow EXACTLY; do not improvise steps.

0. **GATE SWEEP FIRST** (born from the v0.49.0 rustdoc miss): a release
   push must NEVER be the first doc-build. Before the bump commit, in a
   worktree at the release candidate HEAD run:
   `RUSTC_WRAPPER= RUSTDOCFLAGS="-D warnings" cargo doc -p camel-api -p
   camel-core -p camel-builder -p camel-dsl -p camel-endpoint --no-deps`
   plus `cargo fmt --check --all`. The 7-target release matrix is
   CI-only; doc-build and fmt are cheap locally — run them.

1. **Version pins**: the workspace ROOT `Cargo.toml` carries ~69 version
   strings (the `[workspace.package]` version AND internal camel-* pins).
   Bump ALL of them: `sed -i 's/0\.X\.0/0.Y.0/g' Cargo.toml`.
   A one-line workspace-only edit is INCOMPLETE and breaks the build.
2. **Lock regen** (metadata-only, no compile): `cargo update --workspace`.
3. **Audit the lock diff** (non-negotiable): every changed line must be a
   version swap; ZERO dependency-name changes; transitive deps held.
   Verify: `git diff Cargo.lock | grep '^[-+]version' | grep -vc '<old|new>'`
   must print 0.
4. **Golden deptree fixture**: DELETED (2026-09-19, v0.50.0 bump). The
   fixture became version-agnostic in 7904cdd0 (rc-2eal2): the test
   canonicalizes `vX.Y.Z` on both comparison sides. No regen step, no
   color-pin hazard. If `default_closure_matches_golden` ever fails after
   a bump again, the regression is REAL (a crate actually entered/left
   the closure) — investigate, do not regenerate.
5. **Commit** (single, canonical shape): `chore(release): bump version to
   X.Y.Z` — body may carry the audit summary + `Bd:` footer. Create the bd
   FIRST; read its id from the create's own output (never grep lists).
6. **Tag**: lightweight, `vX.Y.Z` (precedent v0.40-v0.47). Owner decides
   the moment.
7. **Push** (main + tag) is the OWNER's exclusive action — it triggers
   `release-matrix.yml` via the `release.yml` tag wrapper. Since dea5c9c7
   (three-flavor pipeline): 12-entry matrix / 11 uploads (slim/regular/
   full per target; desktop full-only; ARM-native runner for
   full-gnu-aarch64), Docker `:slim/:regular/:full` tags, and crates.io
   via OIDC trusted publishing (all registered crates incl.
   camel-component-stream). `-rc.*` tags run the same pipeline but SKIP
   crates.io — use them to rehearse (precedent: v0.50.0-rc.1..rc.3).
   The reusable workflow is invoked by both the `release.yml` tag wrapper
   and the `release-dev.yml` dev wrapper. Agents never push.
8. Cargo in the MAIN checkout is limited to metadata-only commands
   (`cargo update`, `cargo tree`); builds/tests run in worktrees or by
   the human (pre-push tests are the human's domain).

History: the fixture regen step was born at the v0.49.0 bump (2026-09-17)
— the fixture did not exist at v0.48.0, so there was no precedent to read;
this file IS the precedent now.
