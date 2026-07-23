### Workspace version bump prompt

Bump the rust-camel workspace version. Released via tag `vX.Y.Z` →
`.github/workflows/release.yml` (7-target build matrix → GitHub Release →
`scripts/publish-crates.sh` to crates.io → Docker images ghcr + dockerhub, amd64+arm64).

The versioning mechanism is documented as a comment in `Cargo.toml` above
`[workspace.dependencies]` (lines ~38-40): exact pinning (`=X.Y.Z`) of all internal deps
ensures `cargo publish` verifies against the version being released.

**Steps (OLD=A.B.C → NEW=X.Y.Z):**

1. Bump `[workspace.package].version` in `Cargo.toml`.
2. Update all exact-pinned internal deps (the `version = "=OLD"` block, lines ~42-75+).
   Canonical command from the Cargo.toml comment:
   `sed -i 's/version = "=OLD"/version = "=NEW"/g' Cargo.toml`
3. `README.md` line 5: `> **Status:** Pre-release (`OLD`)` → `NEW`.
4. Regenerate the lockfile: `cargo generate-lockfile` (or `cargo build`).
5. Sanity check: `cargo build --workspace`.
   Optional pre-release: `./scripts/publish-crates.sh --dry-run` (topological publish order).
6. Commit (`caveman-commit`): `chore(release): bump version to X.Y.Z`
   - Body: bullet list of notable changes since last release; reference `Bd: rc-XXXX` at end.
7. Local tag: `git tag vX.Y.Z`

**Do NOT `git push`.** The human pushes the tag to trigger `release.yml`.
